# embedded-database-flash 闪存介质后端 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 交付 `embedded-database-flash`（裸闪存抽象 + 内存模拟设备 + 擦除块级 FTL），并让 core 的段环在介质容量上可长期运行（段槽轮转 + 自动合并 + `discard` 信号），使得一个 `Db` 建立在 NOR 闪存 FTL 之上可以持续读写、掉电可恢复、磨损均衡并在坏块下存活。

**Architecture:** 三层，依赖单向 `embedded-database-flash → embedded-database-core`、`embedded-database-os → embedded-database-flash`：
1. `RawFlash` trait + `SimFlash`（内存 NOR 模拟：字节级 1→0、整块擦除、坏块与故障注入）。
2. `Ftl<R: RawFlash>`：擦除块级 FTL，对外实现 core 的 `BlockDevice`（`read_at`/`write_at`/`size`/`sync`/`discard`）。几何：擦除块 4096B = 16 页 × 256B；第 0 页前 32B 是块头，第 1..15 页是逻辑载荷；逻辑块 = 3840B；逻辑容量 = `floor(block_count × 4 / 5)`（20% OP，1:1.25）。RAM 中维护 L2P + 每物理块状态表；块头最后编程 = 提交点；挂载时扫描块头重建映射。
3. `embedded-database-os` 的 `FileRawFlash`（std 后端，供跨进程掉电复验）。
core 侧配套：`BlockDevice::discard` 默认方法、段写偏移取模 `((seq-1) % max_segments) * segment_size`、环感知恢复（空槽 `continue` + 按段号排序 + 链校验仅对相邻段）、`Db` 自动合并且合并后对退役段发 `discard`、`Config::validate` 要求 `max_segments >= 3`。

**Tech Stack:** Rust 1.81.0（`rust-toolchain.toml` 固定 5 目标）、`edition 2021`、`#![no_std]` + `extern crate alloc`、零新增第三方依赖（复用 core 公开再导出的 `crc::hash_bytes`）、`#![forbid(unsafe_code)]`。

**权威设计依据：** `docs/superpowers/specs/2026-10-19-embedded-database-flash-backend-design.md`（336 行，已在 `a5ebedd` 提交）。

---

## Global Constraints

以下约束对每个任务都生效，不重复列出：

- **每一条命令都先在 shell 里执行**：
  `export PATH="/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH"`
- **门禁命令（定义：GATE）**，每个提交前必须全部通过：
  1. `cargo fmt --all --check`
  2. `cargo clippy --workspace --all-targets --features embedded-database-core/std,embedded-database-embed/std -- -D warnings`
  3. `cargo test --workspace --features embedded-database-core/std,embedded-database-embed/std`
  4. `cargo test -p embedded-database-embed`（不带特性；embed 的不变式：默认特性下也必须能构建与测试）
  5. `cargo check -p embedded-database-core`（**不带特性**：core 的 `no_std` 库路径必须可编译。
     执行期实测教训：Task 4 把 `device.rs` 的 `use crate::Error;` 留在
     `#[cfg(feature = "std")]` 之下，而新增的 `discard` 默认方法在任意配置下都用到
     `Error` ⇒ `no_std` 构建失败，而上面 1–4 条全部照旧通过（clippy/test 都带
     `embedded-database-core/std`，embed 的 dev-dependencies 又把 std 打开了）——只有本条目能拦住它）
  6. `cargo check -p embedded-database-flash --target loongarch64-unknown-none`（Task 8 起纳入每任务门禁）
- **零新增第三方依赖**。新 crate 的 `[dependencies]` 只允许 `embedded-database-core`；`[dev-dependencies]` 允许 `embedded-database-core`。
- **禁止 `unsafe`**：全仓 `#![forbid(unsafe_code)]` 且 CI 有 unsafe grep 扫描；新 crate 必须写 `#![forbid(unsafe_code)]`。
- **测试文件位置**：不得新建 `crates/*/tests/` 目录。所有测试写在 `src/` 内的 `#[cfg(test)] mod tests`。
- **提交纪律**：每个任务（Task）内每个 TDD 步骤完成即提交一次；commit message 用中文 `<type>: <描述>`，`type ∈ feat|chore|ci|test|docs|fix`。禁止 `--amend`、禁止改写历史提交。
- **不改历史文档正文**：`docs/superpowers/plans/2026-10-02-*`、`2026-10-19-embedded-database-embed-public-api*.md` 正文保持原样；本计划新增**独立**文件。
- **几何对齐（Db 建在 Ftl 之上时）**：`Config::segment_size` 必须是 FTL `logical_block_size = 3840` 的整数倍，否则 `discard` 只能回收段内的整逻辑块、留下无法回收的尾巴。`Config::default()` 的 1 MiB **不满足**；用 `3_932_160`。此约束只影响 `discard` 的回收效率与容量，不影响正确性。
- **不触碰**：`supply-chain/`、`deny.toml`、`scripts/`。
- **库代码绝不 panic**：所有算术用 `checked_*`/`saturating_*`；索引前判界；`unwrap`/`expect`/`panic!`/`unreachable!`/切片越界索引在 `src/` 非测试代码中一律禁止（clippy `-D warnings` 下还要避免 `indexing_slicing` 类惯性写法——用 `get`/`get_mut` + `ok_or`）。
- **FLASH 几何（不可变）**：`page_size = 256`、`erase_block_size = 4096`、`logical_block_size = erase_block_size - page_size = 3840`、`logical_blocks = floor(block_count × 4 / 5)`。
- **建议的段尺寸**：`segment_size = 3840 × 1024 = 3_932_160` 字节（= 1024 个逻辑块 = 960 个擦除块，两种对齐同时成立）。
- **`embedded-database-flash` 不设 `std` 特性**；`lib.rs` 顶部：`#![no_std]` + `extern crate alloc;` + `#![forbid(unsafe_code)]`。`[dev-dependencies] embedded-database-core = { workspace = true, features = ["std"] }`（resolver v2 在测试编译时统一 dev 特性，使 flash 内可跑 `Db × Ftl` 集成测试）。

---

## File Structure

**新建：**
- `crates/embedded-database-flash/Cargo.toml` — crate 清单；`no_std`；依赖 `embedded-database-core`；dev-dep `embedded-database-core`(std)。
- `crates/embedded-database-flash/src/lib.rs` — crate 根、lint、`pub mod` 与再导出。
- `crates/embedded-database-flash/src/raw.rs`：`FlashError`、`FlashResult`、`RawFlash` trait。
- `crates/embedded-database-flash/src/sim.rs`：`SimFlash` 内存 NOR 模拟设备 + 故障/坏块注入。
- `crates/embedded-database-flash/src/ftl.rs`：`FtlConfig`、`FtlStats`、块头编解码、`Ftl` 及 `impl BlockDevice for Ftl`。
- `crates/embedded-database-os/src/flash.rs`：`FileRawFlash`（std 文件后端 + `fail_after` 注入 + `impl RawFlash`）。

**修改：**
- `crates/embedded-database-core/src/device.rs`：给 `BlockDevice` 加 `discard` 默认方法；`InMemoryDevice` 记录 discard 区间。
- `crates/embedded-database-core/src/recovery.rs`：`RecoveryReport` 增 `first_valid_segment`；`recover` 改为环语义（空/坏槽 `continue`、链校验仅相邻段）。
- `crates/embedded-database-core/src/db.rs`：`Db` 增 `first_live_segment`；`open` 按槽内段头重建配置并排序；`flush` 偏移取模 + 开新段前 `ensure_slot_free`；`compact_with_budget` 拆出无锁 `compact_locked` 并在此发 `discard`。
- `crates/embedded-database-core/src/types.rs`：`validate` 增 `max_segments >= 3`。
- `crates/embedded-database-os/{Cargo.toml,src/lib.rs}`：加 `embedded-database-flash` 依赖 + `pub mod flash;`。
- `Cargo.toml`（workspace）：members 加 `crates/embedded-database-flash`，`[workspace.dependencies]` 加 `embedded-database-flash`。
- `.github/workflows/ci.yml`：`nostd` job 的 target 列表加 `embedded-database-flash`。
- `README.md` 与 `docs/superpowers/specs/2026-10-02-embedded-database-design.md`：T8 如实化（追加「实施状态与勘误」）。
- `.superpowers/sdd/progress.md`：账本追加 Spec 3b 记账 + M4/M11 核销注记。

---

## Task 1: crate 骨架 + RawFlash + SimFlash + CI nostd 接线

**Files:**
- Create: `crates/embedded-database-flash/Cargo.toml`
- Create: `crates/embedded-database-flash/src/lib.rs`
- Create: `crates/embedded-database-flash/src/raw.rs`
- Create: `crates/embedded-database-flash/src/sim.rs`
- Modify: `Cargo.toml`（workspace）
- Modify: `.github/workflows/ci.yml`

**Interfaces:**
- Produces: `embedded_database_flash::raw::{RawFlash, FlashError, FlashResult}`、`embedded_database_flash::sim::SimFlash`。
- Consumes: 无（`lib.rs` 暂只导出 `raw`/`sim`；`ftl` 在 Task 2 加入）。

- [ ] **Step 1: 建立 crate 骨架并接线 workspace**

创建 `crates/embedded-database-flash/Cargo.toml`（字段照 `crates/embedded-database-kv/Cargo.toml` 的实际写法对齐；workspace 的 `[workspace.package]` 只有 version/edition/rust-version/license/repository，**没有** categories/keywords，故不写这两项）：

```toml
[package]
name = "embedded-database-flash"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true
description = "embedded-database 的裸闪存抽象、内存模拟设备与擦除块级 FTL"

[dependencies]
embedded-database-core = { workspace = true, default-features = false }

[dev-dependencies]
# 测试在 host 上编译，需要 embedded-database-core/std 导出的 Db/InMemoryDevice。
# resolver v2：构建测试时 dev-dependency 特性与普通依赖统一。
embedded-database-core = { workspace = true, features = ["std"] }

[lints.rust]
unsafe_code = "forbid"
```

注意：workspace 的 `embedded-database-core` 依赖条目带 `default-features = false`（根 `Cargo.toml:21`），继承到 `[dependencies]` 即 no_std 语义；`[dev-dependencies]` 显式加 `features = ["std"]` 让 flash 内的测试能跑 `Db × Ftl` 集成测试（resolver v2 会把该特性用于 cargo test 的构建）。

创建 `crates/embedded-database-flash/src/lib.rs`（本步只放 crate 文档与属性——模块声明随各文件在下面的步骤里逐个加入，否则本步的 `cargo check` 会因缺少文件而失败）：

```rust
//! embedded-database 的闪存介质后端。
//!
//! 分层：
//! - `raw`：`RawFlash` trait（页/擦除块几何 + NOR 1→0 编程语义）。
//! - `sim`：`SimFlash` 内存模拟设备（坏块与故障注入，供测试）。
//! - `ftl`：擦除块级 FTL，对外提供 core 的 `BlockDevice`。
//!
//! 本 crate 为 `no_std`（需 `alloc`），不依赖任何第三方 crate。
#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;
```

修改 workspace 根 `Cargo.toml`：
- `[workspace] members`（第 3-11 行）数组中加 `"crates/embedded-database-flash",`（放在 `"crates/embedded-database-os"` 之前，保持 no_std 库在前、std 二进制在后的次序）。
- `[workspace.dependencies]` 中加一行（与 `embedded-database-kv` 条目同形）：
  `embedded-database-flash = { path = "crates/embedded-database-flash", version = "0.1.0" }`

修改 `.github/workflows/ci.yml` 的 `nostd` job（第 38-51 行）：把第 51 行的
`cargo check -p embedded-database-core -p embedded-database-kv -p embedded-database-embed --target ${{ matrix.target }}`
改为
`cargo check -p embedded-database-core -p embedded-database-kv -p embedded-database-embed -p embedded-database-flash --target ${{ matrix.target }}`。

（`-p` 只构建 lib 目标，不会带上 flash 的 dev-dependency 特性，故不会把 `embedded-database-core/std` 拉进 no_std 构建；Step 2 的 thumbv7em 检查会实证这一点。）

- [ ] **Step 2: 跑骨架门禁，确认能编译**

Run:
```bash
export PATH="/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH"
cargo check -p embedded-database-flash
cargo check -p embedded-database-flash --target loongarch64-unknown-none
```
Expected: 两条都 `Finished`，无 warning；第二条证明 `no_std` 成立（`loongarch64-unknown-none` 是 `rust-toolchain.toml` 已固定的裸机目标，不要用未安装的 target 名）。

- [ ] **Step 3: 提交骨架**

```bash
git add Cargo.toml crates/embedded-database-flash .github/workflows/ci.yml
git commit -m "chore: 新增 embedded-database-flash crate 骨架并接线 CI nostd"
```

- [ ] **Step 4: 写 RawFlash 抽象（`raw.rs`）**

在 `lib.rs` 末尾加 `pub mod raw;`，然后创建 `crates/embedded-database-flash/src/raw.rs`：

```rust
//! 裸闪存抽象：页/擦除块几何与 NOR 1→0 编程语义。
//!
//! 这一层只描述"介质能做什么"，不含任何地址映射；映射由 `ftl` 负责。

/// 裸闪存操作错误。
///
/// 每种错误都携带出错的物理块号，便于 FTL 把块标坏或触发搬迁。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlashError {
    /// 块号或块内偏移/长度越界。
    OutOfRange,
    /// 编程范围跨越页边界。真实 NOR 的页编程不可跨页。
    Unaligned,
    /// 目标块已被实现标记为坏块。
    BadBlock { block: u32 },
    /// 编程失败：目标位置已有 `0` 但本次要写 `1`（违反写前擦除），
    /// 或介质/注入的硬错误。实现必须**整体拒绝**，不得部分写入。
    Program { block: u32 },
    /// 擦除失败（介质故障或注入）。
    Erase { block: u32 },
    /// 读取失败（介质故障或注入）。
    Read { block: u32 },
    /// 刷缓冲失败。
    Sync,
}

/// `RawFlash` 操作的返回类型。
pub type FlashResult<T> = core::result::Result<T, FlashError>;

/// 裸闪存设备。
///
/// 实现必须满足以下语义（FTL 依赖它们）：
/// - `read`/`program` 的 `offset` 是**块内字节偏移**，任何访问不得跨块。
/// - `program` 的写入范围不得跨越页边界，否则返回 [`FlashError::Unaligned`]。
/// - `program` 遵守 NOR 语义：只允许 `1 → 0`；任何 `0 → 1` 需求**整体拒绝**
///   并返回 [`FlashError::Program`]（不产生部分写入）。
/// - `erase` 把整块置为 `0xFF`，且幂等。
/// - 坏块由实现自行记录：`read`/`program`/`erase` 命中坏块一律返回
///   [`FlashError::BadBlock`]，FTL 据此把块退役。
pub trait RawFlash {
    /// 页大小（字节）。
    fn page_size(&self) -> usize;
    /// 擦除块大小（字节），必须是 `page_size` 的整数倍。
    fn erase_block_size(&self) -> usize;
    /// 物理擦除块总数。
    fn block_count(&self) -> u32;
    /// 从 `block` 的块内偏移 `offset` 读 `buf.len()` 字节。
    fn read(&self, block: u32, offset: usize, buf: &mut [u8]) -> FlashResult<()>;
    /// 把 `data` 编程到块内 `offset`（1→0 语义，不得跨页）。
    fn program(&mut self, block: u32, offset: usize, data: &[u8]) -> FlashResult<()>;
    /// 擦除整块（置 0xFF）。
    fn erase(&mut self, block: u32) -> FlashResult<()>;
    /// 落盘/刷缓冲。内存实现可为空操作。
    fn sync(&mut self) -> FlashResult<()>;
}
```

- [ ] **Step 5: 写 SimFlash 与测试（`sim.rs`）**

在 `lib.rs` 加 `pub mod sim;`，然后创建 `crates/embedded-database-flash/src/sim.rs`：

```rust
//! 内存 NOR 闪存模拟设备：字节级 1→0 编程、整块擦除、坏块与故障注入。

use alloc::vec::Vec;

use crate::raw::{FlashError, FlashResult, RawFlash};

const ERASED: u8 = 0xFF;

/// 内存 NOR 闪存。
///
/// 用于 FTL 的功能与故障注入测试：可标记坏块、可在第 N 次编程/擦除后
/// 开始持续失败（模拟磨损耗尽或断电后的介质异常）。
#[derive(Debug, Clone)]
pub struct SimFlash {
    page_size: usize,
    erase_block_size: usize,
    bytes: Vec<u8>,
    bad: Vec<bool>,
    erase_counts: Vec<u32>,
    program_ops: u64,
    fail_program_after: Option<u64>,
    fail_erase_after: Option<u64>,
}

impl SimFlash {
    /// 创建 `block_count` 个全 `0xFF` 的擦除块。
    pub fn new(block_count: u32, page_size: usize, erase_block_size: usize) -> Self {
        let total = block_count as usize * erase_block_size;
        SimFlash {
            page_size,
            erase_block_size,
            bytes: alloc::vec![ERASED; total],
            bad: alloc::vec![false; block_count as usize],
            erase_counts: alloc::vec![0u32; block_count as usize],
            program_ops: 0,
            fail_program_after: None,
            fail_erase_after: None,
        }
    }

    /// 标记物理块为坏块。
    pub fn mark_bad(&mut self, block: u32) {
        if let Some(slot) = self.bad.get_mut(block as usize) {
            *slot = true;
        }
        if let Some(slot) = self.erase_counts.get_mut(block as usize) {
            *slot = 0;
        }
    }

    /// 该块是否坏。
    pub fn is_bad(&self, block: u32) -> bool {
        self.bad.get(block as usize).copied().unwrap_or(true)
    }

    /// 从第 `after` 次编程起，后续所有编程都返回 `Program` 错误。
    pub fn inject_program_failure(&mut self, after: u64) {
        self.fail_program_after = Some(after);
    }

    /// 从第 `after` 次擦除起，后续擦除都返回 `Erase`。
    pub fn inject_erase_failure(&mut self, after: u64) {
        self.fail_erase_after = Some(after);
    }

    /// 累计成功编程次数（含被拒绝前已计入的尝试不在此列）。
    pub fn program_count(&self) -> u64 {
        self.program_ops
    }

    /// 某个物理块被成功擦除的次数。
    pub fn erase_count_of(&self, block: u32) -> u32 {
        self.erase_counts.get(block as usize).copied().unwrap_or(0)
    }

    /// 把整片介质快照拷进 `out`（不足部分不写）。用于掉电复验。
    pub fn snapshot_into(&self, out: &mut [u8]) {
        let n = out.len().min(self.bytes.len());
        out[..n].copy_from_slice(&self.bytes[..n]);
    }

    fn base(&self, block: u32) -> usize {
        block as usize * self.erase_block_size
    }

    fn check(&self, block: u32, offset: usize, len: usize) -> FlashResult<()> {
        if block as usize >= self.bad.len() {
            return Err(FlashError::OutOfRange);
        }
        if self.bad[block as usize] {
            return Err(FlashError::BadBlock { block });
        }
        if offset > self.erase_block_size || len > self.erase_block_size - offset {
            return Err(FlashError::OutOfRange);
        }
        Ok(())
    }
}

impl RawFlash for SimFlash {
    fn page_size(&self) -> usize {
        self.page_size
    }

    fn erase_block_size(&self) -> usize {
        self.erase_block_size
    }

    fn block_count(&self) -> u32 {
        self.bad.len() as u32
    }

    fn read(&self, block: u32, offset: usize, buf: &mut [u8]) -> FlashResult<()> {
        self.check(block, offset, buf.len())?;
        let at = self.base(block) + offset;
        buf.copy_from_slice(&self.bytes[at..at + buf.len()]);
        Ok(())
    }

    fn program(&mut self, block: u32, offset: usize, data: &[u8]) -> FlashResult<()> {
        self.check(block, offset, data.len())?;
        if data.is_empty() {
            return Ok(());
        }
        // 页边界检查：首尾落在同一页内。
        let first = offset / self.page_size;
        let last = (offset + data.len() - 1) / self.page_size;
        if first != last {
            return Err(FlashError::Unaligned);
        }
        if let Some(limit) = self.fail_program_after {
            if self.program_ops >= limit {
                return Err(FlashError::Program { block });
            }
        }
        let at = self.base(block) + offset;
        // NOR 语义：先整体校验，任一字节需要 0→1 就整体拒绝。
        for (i, &byte) in data.iter().enumerate() {
            if self.bytes[at + i] & byte != byte {
                return Err(FlashError::Program { block });
            }
        }
        self.bytes[at..at + data.len()].copy_from_slice(data);
        self.program_ops += 1;
        Ok(())
    }

    fn erase(&mut self, block: u32) -> FlashResult<()> {
        if block as usize >= self.bad.len() {
            return Err(FlashError::OutOfRange);
        }
        if self.bad[block as usize] {
            return Err(FlashError::BadBlock { block });
        }
        if let Some(limit) = self.fail_erase_after {
            let done = self.erase_counts[block as usize] as u64;
            let total: u64 = self.erase_counts.iter().map(|c| u64::from(*c)).sum();
            let _ = done;
            if total >= limit {
                return Err(FlashError::Erase { block });
            }
        }
        let at = self.base(block);
        let end = at + self.erase_block_size;
        self.bytes[at..end].fill(ERASED);
        self.erase_counts[block as usize] += 1;
        Ok(())
    }

    fn sync(&mut self) -> FlashResult<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: usize = 256;
    const BLOCK: usize = 4096;

    #[test]
    fn program_只允许_1_到_0_且整体拒绝() {
        let mut f = SimFlash::new(2, PAGE, BLOCK);
        f.program(0, 0, &[0x0F]).unwrap();
        // 需要把第 0 字节的 0 位变成 1：整体拒绝。
        assert_eq!(f.program(0, 0, &[0xF0]), Err(FlashError::Program { block: 0 }));
        // NOR 语义允许「新值是当前值的位子集」：0x0F & 0x0C == 0x0C。
        f.program(0, 0, &[0x0C]).unwrap();
        let mut got = [0u8; 1];
        f.read(0, 0, &mut got).unwrap();
        assert_eq!(got[0], 0x0C);
    }

    #[test]
    fn program_跨页被拒() {
        let mut f = SimFlash::new(1, PAGE, BLOCK);
        assert_eq!(f.program(0, PAGE - 1, &[0x00, 0x00]), Err(FlashError::Unaligned));
    }

    #[test]
    fn erase_恢复为全_ff_并累加计数() {
        let mut f = SimFlash::new(1, PAGE, BLOCK);
        f.program(0, 0, &[0x00, 0x00]).unwrap();
        f.erase(0).unwrap();
        let mut got = [0u8; 2];
        f.read(0, 0, &mut got).unwrap();
        assert_eq!(got, [0xFF, 0xFF]);
        assert_eq!(f.erase_count_of(0), 1);
    }

    #[test]
    fn 越界与坏块返回对应错误() {
        let mut f = SimFlash::new(1, PAGE, BLOCK);
        assert_eq!(f.read(1, 0, &mut [0u8; 1]), Err(FlashError::OutOfRange));
        assert_eq!(f.read(0, BLOCK, &mut [0u8; 1]), Err(FlashError::OutOfRange));
        f.mark_bad(0);
        assert_eq!(f.read(0, 0, &mut [0u8; 1]), Err(FlashError::BadBlock { block: 0 }));
        assert_eq!(f.erase(0), Err(FlashError::BadBlock { block: 0 }));
    }

    #[test]
    fn 编程失败注入从第_n_次起生效() {
        let mut f = SimFlash::new(1, PAGE, BLOCK);
        f.inject_program_failure(1);
        f.program(0, 0, &[0x00]).unwrap();
        assert_eq!(f.program(0, 1, &[0x00]), Err(FlashError::Program { block: 0 }));
    }
}
```

- [ ] **Step 6: 运行测试**

Run:
```bash
export PATH="/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH"
cargo test -p embedded-database-flash
```
Expected: 5 passed（0 failed）。若 clippy 报 `erase` 里未使用的 `done` 变量，删掉那三行（计划中为可读性保留，实现时以 `-D warnings` 为准）。

- [ ] **Step 7: 提交**

```bash
git add crates/embedded-database-flash
git commit -m "feat: 新增 RawFlash 抽象与 SimFlash 内存闪存模拟"
```

---

## Task 2: FTL 基础（几何 / 块头 / 挂载扫描 / 读写）

**Files:**
- Create: `crates/embedded-database-flash/src/ftl.rs`
- Modify: `crates/embedded-database-flash/src/lib.rs`

**Interfaces:**
- Produces:
  - `embedded_database_flash::ftl::{Ftl, FtlConfig}`，`Ftl::new(raw: R, cfg: FtlConfig) -> embedded_database_core::Result<Ftl<R>>`、`Ftl::into_raw(self) -> R`、`Ftl::logical_size(&self) -> u64`。
  - `impl<R: RawFlash> embedded_database_core::BlockDevice for Ftl<R>`（`read_at`/`write_at`/`size`/`sync`；`discard` 在 Task 3）。
- Consumes: `crate::raw::{RawFlash, FlashError}`、`embedded_database_core::{hash_bytes, BlockDevice, Error, Result}`。

- [ ] **Step 1: 写失败的测试（`ftl.rs` 内 `mod tests`）**

创建 `crates/embedded-database-flash/src/ftl.rs`，先只放常量和测试模块（实现留到 Step 3，测试此刻应当编译失败）：

```rust
//! 擦除块级 FTL：把 `RawFlash` 包装成 core 的 `BlockDevice`。

use alloc::vec::Vec;

use embedded_database_core::{BlockDevice, Error, Result};

use crate::raw::{FlashError, RawFlash};

/// 块头魔数。
pub const HEADER_MAGIC: [u8; 4] = *b"WHFT";
/// 块头版本。
pub const HEADER_VERSION: u8 = 1;
/// 块头字节数（占块内第 0 页的前 32 字节）。
pub const HEADER_SIZE: usize = 32;
/// 块头 `state` 字段：`1` 表示该块持有有效的逻辑块数据。
const STATE_DATA: u8 = 1;
/// L2P 中"未映射"的哨兵值。
const UNMAPPED: u32 = u32::MAX;

/// FTL 可调参数。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FtlConfig {
    /// 预留空间百分比（OP）。1..=100。逻辑容量 = `block_count × (100 - op) / 100`。
    pub op_percent: u8,
    /// 静态磨损均衡间隔：每完成这么多次擦除，尝试搬移一个冷块。0 = 关闭。
    pub static_level_interval: u64,
    /// 空闲块最大擦除计数与最小擦除计数之差超过它就触发静态均衡。0 = 关闭。
    pub wear_gap_threshold: u32,
}

impl Default for FtlConfig {
    fn default() -> Self {
        FtlConfig {
            op_percent: 20,
            static_level_interval: 64,
            wear_gap_threshold: 2,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::SimFlash;
    use embedded_database_core::BlockDevice;

    const PAGE: usize = 256;
    const BLOCK: usize = 4096;
    const LOGICAL_BLOCK: usize = BLOCK - PAGE; // 3840
    // 1024 个逻辑块 = 3840 KiB 逻辑容量（20% OP：1280 物理块）。
    const NBLOCKS: u32 = 1280;

    fn ftl() -> Ftl<SimFlash> {
        let raw = SimFlash::new(NBLOCKS, PAGE, BLOCK);
        Ftl::new(raw, FtlConfig::default()).unwrap()
    }

    #[test]
    fn 逻辑容量等于_物理块数乘_4_除_5_再乘_3840() {
        let f = ftl();
        assert_eq!(f.logical_size(), (NBLOCKS as u64 * 4 / 5) * LOGICAL_BLOCK as u64);
    }

    #[test]
    fn 单块写后读回一致()  {
        let mut f = ftl();
        let data = alloc::vec![0xA5u8; LOGICAL_BLOCK];
        f.write_at(0, &data).unwrap();
        let mut got = alloc::vec![0u8; LOGICAL_BLOCK];
        f.read_at(0, &mut got).unwrap();
        assert_eq!(got, data);
    }

    #[test]
    fn 同一逻辑块分两次写_两段都能读回() {
        let mut f = ftl();
        let a = alloc::vec![0x11u8; 256];
        let b = alloc::vec![0x22u8; 256];
        f.write_at(0, &a).unwrap();
        f.write_at(256, &b).unwrap();
        let mut got = alloc::vec![0u8; 512];
        f.read_at(0, &mut got).unwrap();
        assert_eq!(&got[..256], &a[..]);
        assert_eq!(&got[256..], &b[..]);
    }

    #[test]
    fn 覆写同一逻辑块后读到新数据() {
        let mut f = ftl();
        f.write_at(0, &alloc::vec![0x11u8; 128]).unwrap();
        f.write_at(0, &alloc::vec![0x33u8; 128]).unwrap();
        let mut got = alloc::vec![0u8; 128];
        f.read_at(0, &mut got).unwrap();
        assert_eq!(got, alloc::vec![0x33u8; 128]);
    }

    #[test]
    fn 卸载重挂后数据仍在() {
        let mut f = ftl();
        f.write_at(LOGICAL_BLOCK as u64, &alloc::vec![0x77u8; 512]).unwrap();
        f.sync().unwrap();
        let raw = f.into_raw();
        let mut f2 = Ftl::new(raw, FtlConfig::default()).unwrap();
        let mut got = alloc::vec![0u8; 512];
        f2.read_at(LOGICAL_BLOCK as u64, &mut got).unwrap();
        assert_eq!(got, alloc::vec![0x77u8; 512]);
    }

    #[test]
    fn 越界读写返回_InvalidArgument() {
        let mut f = ftl();
        let size = f.logical_size();
        assert_eq!(f.write_at(size, &[0u8; 1]), Err(Error::InvalidArgument));
        assert_eq!(f.read_at(size, &mut [0u8; 1]), Err(Error::InvalidArgument));
    }

    #[test]
    fn 未映射逻辑块读出全_ff() {
        let mut f = ftl();
        let mut got = alloc::vec![0u8; 16];
        f.read_at(0, &mut got).unwrap();
        assert_eq!(got, alloc::vec![0xFFu8; 16]);
    }
}
```

在 `lib.rs` 加 `pub mod ftl;`，并加再导出：

```rust
pub use ftl::{Ftl, FtlConfig};
pub use raw::{FlashError, FlashResult, RawFlash};
pub use sim::SimFlash;
```

- [ ] **Step 2: 运行测试确认失败**

Run:
```bash
export PATH="/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH"
cargo test -p embedded-database-flash 2>&1 | tail -20
```
Expected: 编译失败，报 `cannot find type Ftl` / `no function new`。

- [ ] **Step 3: 实现 FTL（插到 `ftl.rs` 的 `use` 之后、`mod tests` 之前）**

```rust
/// FTL 运行统计。`write_amplification = physical_program_bytes / logical_write_bytes`
/// （逻辑写字节为 0 时为 0.0）。
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct FtlStats {
    /// 上层请求写入的逻辑字节数。
    pub logical_write_bytes: u64,
    /// 实际编程到介质的字节数（含搬迁与块头）。
    pub physical_program_bytes: u64,
    /// 写放大。
    pub write_amplification: f64,
    /// 擦除操作次数。
    pub erase_ops: u64,
    /// 空闲块中的最小擦除计数。
    pub erase_count_min: u32,
    /// 空闲块中的最大擦除计数。
    pub erase_count_max: u32,
    /// 块搬迁次数。
    pub block_moves: u64,
    /// 坏块数。
    pub bad_blocks: u32,
    /// 空闲块数。
    pub free_blocks: u32,
    /// 待回收（Dirty）块数。
    pub retired_blocks: u32,
}

/// 单个物理块的运行时状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BlockState {
    /// 已擦除、可分配。
    Free,
    /// 持有某个逻辑块的有效数据。
    Data,
    /// 曾持有数据但已被新版本取代（或块头不可信），擦除后即可重用。
    Dirty,
    /// 介质故障，永久退役。
    Bad,
}

/// RAM 中的每物理块元数据。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Phys {
    state: BlockState,
    logical: u32,
    alloc_seq: u64,
    erase_count: u32,
}

/// 落盘块头（32 字节，CRC 覆盖前 28 字节）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Header {
    logical: u32,
    alloc_seq: u64,
    erase_count: u32,
}

/// 把块头编码成 32 字节。
fn encode_header(h: Header) -> [u8; HEADER_SIZE] {
    let mut buf = [0u8; HEADER_SIZE];
    buf[0..4].copy_from_slice(&HEADER_MAGIC);
    buf[4] = HEADER_VERSION;
    buf[5] = STATE_DATA;
    buf[8..12].copy_from_slice(&h.logical.to_le_bytes());
    buf[12..20].copy_from_slice(&h.alloc_seq.to_le_bytes());
    buf[20..24].copy_from_slice(&h.erase_count.to_le_bytes());
    let crc = embedded_database_core::hash_bytes(&buf[..28]);
    buf[28..32].copy_from_slice(&crc.to_le_bytes());
    buf
}

/// 解码块头。魔数/版本/状态/CRC 任一不符即视为"无有效块头"。
fn decode_header(buf: &[u8]) -> Option<Header> {
    if buf.len() < HEADER_SIZE {
        return None;
    }
    if buf[0..4] != HEADER_MAGIC || buf[4] != HEADER_VERSION || buf[5] != STATE_DATA {
        return None;
    }
    let expect = embedded_database_core::hash_bytes(&buf[..28]);
    let got = u32::from_le_bytes([buf[28], buf[29], buf[30], buf[31]]);
    if expect != got {
        return None;
    }
    Some(Header {
        logical: u32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]),
        alloc_seq: u64::from_le_bytes([
            buf[12], buf[13], buf[14], buf[15], buf[16], buf[17], buf[18], buf[19],
        ]),
        erase_count: u32::from_le_bytes([buf[20], buf[21], buf[22], buf[23]]),
    })
}

/// 擦除块级 FTL。
///
/// 对外是一个连续的逻辑块设备（逻辑块 = 逻辑页 1..15 = 3840 字节）。
/// 对内维护 `l2p` 映射与每物理块状态；提交点是块头的成功编程。
pub struct Ftl<R: RawFlash> {
    raw: R,
    cfg: FtlConfig,
    page_size: usize,
    erase_block_size: usize,
    logical_block_size: usize,
    block_count: u32,
    logical_blocks: u32,
    l2p: Vec<u32>,
    phys: Vec<Phys>,
    next_alloc_seq: u64,
    /// 静态均衡的递归守卫（Task 3 加入）。
    leveling: bool,
    stats: FtlStats,
}

impl<R: RawFlash> Ftl<R> {
    /// 用给定参数构造并挂载 FTL。
    ///
    /// 几何校验失败、或 `op_percent` 非法时返回 [`Error::InvalidArgument`]。
    pub fn new(raw: R, cfg: FtlConfig) -> Result<Self> {
        let page_size = raw.page_size();
        let erase_block_size = raw.erase_block_size();
        let block_count = raw.block_count();
        if page_size < 64 || !page_size.is_power_of_two() {
            return Err(Error::InvalidArgument);
        }
        if erase_block_size % page_size != 0 || erase_block_size / page_size < 2 {
            return Err(Error::InvalidArgument);
        }
        if cfg.op_percent == 0 || cfg.op_percent > 100 {
            return Err(Error::InvalidArgument);
        }
        let logical_blocks =
            (u64::from(block_count) * u64::from(100 - cfg.op_percent) / 100) as u32;
        if block_count < 4 || logical_blocks < 2 {
            return Err(Error::InvalidArgument);
        }
        let logical_block_size = erase_block_size - page_size;
        let mut ftl = Ftl {
            raw,
            cfg,
            page_size,
            erase_block_size,
            logical_block_size,
            block_count,
            logical_blocks,
            l2p: alloc::vec![UNMAPPED; logical_blocks as usize],
            phys: alloc::vec![
                Phys { state: BlockState::Free, logical: UNMAPPED, alloc_seq: 0, erase_count: 0 };
                block_count as usize
            ],
            next_alloc_seq: 1,
            leveling: false,
            stats: FtlStats::default(),
        };
        ftl.mount()?;
        ftl.refresh_stats();
        Ok(ftl)
    }

    /// 取出底层设备（供测试做 L2P 重建/掉电复验）。
    pub fn into_raw(self) -> R {
        self.raw
    }

    /// 逻辑容量（字节）。
    pub fn logical_size(&self) -> u64 {
        u64::from(self.logical_blocks) * self.logical_block_size as u64
    }

    /// 挂载：扫描全部物理块的块头，重建 L2P。
    fn mount(&mut self) -> Result<()> {
        let mut buf = [0u8; HEADER_SIZE];
        let mut max_ec = 0u32;
        let mut found: Vec<(u32, Header)> = Vec::new();
        for block in 0..self.block_count {
            match self.raw.read(block, 0, &mut buf) {
                Ok(()) => {}
                Err(FlashError::BadBlock { .. }) | Err(FlashError::Read { .. }) => {
                    self.phys[block as usize].state = BlockState::Bad;
                    continue;
                }
                Err(_) => return Err(Error::Io),
            }
            if let Some(h) = decode_header(&buf) {
                if h.erase_count > max_ec {
                    max_ec = h.erase_count;
                }
                found.push((block, h));
            }
        }

        // 同一逻辑块多个候选：alloc_seq 最大者胜，其余标 Dirty。
        for (block, h) in found {
            let idx = block as usize;
            if h.logical == UNMAPPED || h.logical >= self.logical_blocks {
                self.phys[idx] = Phys {
                    state: BlockState::Dirty,
                    logical: h.logical,
                    alloc_seq: h.alloc_seq,
                    erase_count: h.erase_count,
                };
                continue;
            }
            let li = h.logical as usize;
            let cur = self.l2p[li];
            let replace = cur == UNMAPPED
                || self.phys[cur as usize].alloc_seq < h.alloc_seq;
            if replace {
                if cur != UNMAPPED {
                    let c = cur as usize;
                    self.phys[c] = Phys { state: BlockState::Dirty, ..self.phys[c] };
                }
                self.l2p[li] = block;
                self.phys[idx] = Phys {
                    state: BlockState::Data,
                    logical: h.logical,
                    alloc_seq: h.alloc_seq,
                    erase_count: h.erase_count,
                };
                if h.alloc_seq >= self.next_alloc_seq {
                    self.next_alloc_seq = h.alloc_seq + 1;
                }
            } else {
                self.phys[idx] = Phys {
                    state: BlockState::Dirty,
                    logical: h.logical,
                    alloc_seq: h.alloc_seq,
                    erase_count: h.erase_count,
                };
            }
        }

        // 块头不可信的空闲块：擦除计数不可知，保守取已知最大值。
        for p in self.phys.iter_mut() {
            if p.state == BlockState::Free {
                p.erase_count = p.erase_count.max(max_ec);
            }
        }
        Ok(())
    }

    /// 选一个空闲块；没有空闲则先回收一个 Dirty 块。都不行则 `CapacityExhausted`。
    fn alloc_block(&mut self, logical: u32) -> Result<u32> {
        let block = match self.pick_free() {
            Some(b) => b,
            None => {
                self.reclaim_one_dirty()?;
                self.pick_free().ok_or(Error::CapacityExhausted)?
            }
        };
        let idx = block as usize;
        let ec = self.phys[idx].erase_count;
        self.phys[idx] = Phys {
            state: BlockState::Data,
            logical,
            alloc_seq: self.next_alloc_seq,
            erase_count: ec,
        };
        self.next_alloc_seq = self.next_alloc_seq.saturating_add(1);
        Ok(block)
    }

    /// 空闲块中擦除计数最小者。
    fn pick_free(&self) -> Option<u32> {
        let mut best: Option<u32> = None;
        for (i, p) in self.phys.iter().enumerate() {
            if p.state != BlockState::Free {
                continue;
            }
            match best {
                None => best = Some(i as u32),
                Some(b) if p.erase_count < self.phys[b as usize].erase_count => {
                    best = Some(i as u32)
                }
                _ => {}
            }
        }
        best
    }

    /// 回收一个 Dirty 块（擦除后变 Free）。没有 Dirty 块则空操作。
    fn reclaim_one_dirty(&mut self) -> Result<()> {
        let victim = self
            .phys
            .iter()
            .position(|p| p.state == BlockState::Dirty)
            .map(|i| i as u32);
        if let Some(v) = victim {
            self.erase_block(v)?;
        }
        Ok(())
    }

    /// 擦除一块；介质故障则永久标记为坏块（不算操作失败）。
    fn erase_block(&mut self, block: u32) -> Result<()> {
        match self.raw.erase(block) {
            Ok(()) => {
                let idx = block as usize;
                self.phys[idx] = Phys {
                    state: BlockState::Free,
                    logical: UNMAPPED,
                    alloc_seq: 0,
                    erase_count: self.phys[idx].erase_count.saturating_add(1),
                };
                self.stats.erase_ops += 1;
                Ok(())
            }
            Err(FlashError::BadBlock { .. }) | Err(FlashError::Erase { .. }) => {
                self.phys[block as usize].state = BlockState::Bad;
                Ok(())
            }
            Err(_) => Err(Error::Io),
        }
    }

    /// 把一个逻辑块载荷（长度必须是 `logical_block_size`）写进指定物理块。
    ///
    /// 按页拆分编程（`RawFlash::program` 不允许跨页）。**不**提交块头——
    /// 调用方在数据落盘后再调 `commit_header`。
    fn program_payload(&mut self, phys: u32, buf: &[u8]) -> Result<()> {
        for (i, chunk) in buf.chunks(self.page_size).enumerate() {
            self.raw
                .program(phys, self.page_size + i * self.page_size, chunk)
                .map_err(|_| Error::Io)?;
            self.stats.physical_program_bytes += chunk.len() as u64;
        }
        Ok(())
    }

    /// 单次逻辑块内写：必要时分配或搬迁，最后统一提交块头。
    ///
    /// 循环上限 4 次只为"换掉中途变坏的块"，不是重试语义：坏块每出现一次就
    /// 退役一个物理块，空闲池有限，因此必须有界，避免无限递归。
    fn write_chunk(&mut self, logical: u32, within: usize, data: &[u8]) -> Result<()> {
        for _attempt in 0..4 {
            let mut phys = self.l2p[logical as usize];
            let mut fresh = false;
            if phys == UNMAPPED {
                phys = self.alloc_block(logical)?;
                self.l2p[logical as usize] = phys;
                fresh = true;
            }
            let off = self.page_size + within;
            match self.raw.program(phys, off, data) {
                Ok(()) => {
                    self.stats.logical_write_bytes += data.len() as u64;
                    self.stats.physical_program_bytes += data.len() as u64;
                    if fresh {
                        self.commit_header(phys)?;
                    }
                    return Ok(());
                }
                // 0→1 冲突或硬错误：整块搬迁（读旧 + 合并本次写 + 写新块）。
                Err(FlashError::Program { .. }) => {
                    return self.relocate(logical, within, data)
                }
                // 块坏了：标坏、解除映射，下一轮换一块重写本次数据。
                Err(FlashError::BadBlock { .. }) => {
                    self.phys[phys as usize].state = BlockState::Bad;
                    if self.l2p[logical as usize] == phys {
                        self.l2p[logical as usize] = UNMAPPED;
                    }
                    continue;
                }
                Err(_) => return Err(Error::Io),
            }
        }
        Err(Error::Io)
    }

    /// 整块搬迁：读出旧载荷，套上本次写，写进新块并提交。
    ///
    /// 这是 FTL 唯一的写放大来源：同一个逻辑块第二次写落在同一个物理块时，
    /// NOR 的 0→1 限制会拒绝，于是必须换块重写整块。
    fn relocate(&mut self, logical: u32, within: usize, data: &[u8]) -> Result<()> {
        let end = within
            .checked_add(data.len())
            .ok_or(Error::InvalidArgument)?;
        if end > self.logical_block_size {
            return Err(Error::InvalidArgument);
        }
        let old = self.l2p[logical as usize];
        let mut buf = alloc::vec![0xFFu8; self.logical_block_size];
        if old != UNMAPPED && self.phys[old as usize].state == BlockState::Data {
            for (i, chunk) in buf.chunks_mut(self.page_size).enumerate() {
                let off = self.page_size + i * self.page_size;
                // 旧块读失败（坏块/硬错误）时保留 0xFF：数据已不可救，
                // 但调用方仍会拿到本次写入的新数据。
                if self.raw.read(old, off, chunk).is_err() {
                    chunk.fill(0xFF);
                }
            }
        }
        buf[within..end].copy_from_slice(data);

        let np = self.alloc_block(logical)?;
        self.program_payload(np, &buf)?;
        self.l2p[logical as usize] = np;
        self.commit_header(np)?;
        if old != UNMAPPED {
            self.phys[old as usize] = Phys {
                state: BlockState::Dirty,
                ..self.phys[old as usize]
            };
        }
        self.stats.logical_write_bytes += data.len() as u64;
        self.stats.block_moves += 1;
        Ok(())
    }

    /// 提交：把块头写进块的页 0。
    fn commit_header(&mut self, block: u32) -> Result<()> {
        let p = self.phys[block as usize];
        let h = Header {
            logical: p.logical,
            alloc_seq: p.alloc_seq,
            erase_count: p.erase_count,
        };
        let buf = encode_header(h);
        self.raw
            .program(block, 0, &buf)
            .map_err(|_| Error::Io)
    }

    /// 刷新 FTL 统计。
    fn refresh_stats(&mut self) {
        let mut free = 0u32;
        let mut bad = 0u32;
        let mut retired = 0u32;
        let mut min = u32::MAX;
        let mut max = 0u32;
        for p in self.phys.iter() {
            match p.state {
                BlockState::Free => {
                    free += 1;
                    min = min.min(p.erase_count);
                    max = max.max(p.erase_count);
                }
                BlockState::Bad => bad += 1,
                BlockState::Dirty => retired += 1,
                BlockState::Data => {}
            }
        }
        if free == 0 {
            min = 0;
        }
        self.stats.free_blocks = free;
        self.stats.bad_blocks = bad;
        self.stats.retired_blocks = retired;
        self.stats.erase_count_min = min;
        self.stats.erase_count_max = max;
        let logical = self.stats.logical_write_bytes;
        let physical = self.stats.physical_program_bytes;
        self.stats.write_amplification = if logical == 0 {
            0.0
        } else {
            physical as f64 / logical as f64
        };
    }
}

impl<R: RawFlash> BlockDevice for Ftl<R> {
    fn size(&self) -> u64 {
        self.logical_size()
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        let end = offset
            .checked_add(buf.len() as u64)
            .ok_or(Error::InvalidArgument)?;
        if end > self.logical_size() {
            return Err(Error::InvalidArgument);
        }
        let start = offset as usize;
        let lsize = self.logical_block_size;
        let mut done = 0usize;
        while done < buf.len() {
            let absolute = start + done;
            let logical = (absolute / lsize) as u32;
            let within = absolute % lsize;
            let take = (lsize - within).min(buf.len() - done);
            let phys = self.l2p[logical as usize];
            let dst = &mut buf[done..done + take];
            if phys == UNMAPPED || self.phys[phys as usize].state != BlockState::Data {
                dst.fill(0xFF);
            } else {
                self.raw
                    .read(phys, self.page_size + within, dst)
                    .map_err(|_| Error::Io)?;
            }
            done += take;
        }
        Ok(())
    }

    fn write_at(&mut self, offset: u64, buf: &[u8]) -> Result<()> {
        let end = offset
            .checked_add(buf.len() as u64)
            .ok_or(Error::InvalidArgument)?;
        if end > self.logical_size() {
            return Err(Error::InvalidArgument);
        }
        let start = offset as usize;
        let lsize = self.logical_block_size;
        let mut done = 0usize;
        while done < buf.len() {
            let absolute = start + done;
            let logical = (absolute / lsize) as u32;
            let within = absolute % lsize;
            // 切到"逻辑块尾"与"物理页尾"两者中更近的一个：`RawFlash::program`
            // 不允许跨页，块内页偏移 = `page_size + within`。
            let to_page_end = self.page_size - (within % self.page_size);
            let take = to_page_end.min(lsize - within).min(buf.len() - done);
            self.write_chunk(logical, within, &buf[done..done + take])?;
            done += take;
        }
        Ok(())
    }

    fn sync(&mut self) -> Result<()> {
        self.raw.sync().map_err(|_| Error::Io)
    }
}
```

- [ ] **Step 4: 运行测试**

Run:
```bash
export PATH="/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH"
cargo test -p embedded-database-flash
```
Expected: Task 1 的 5 个 + Task 2 的 7 个 = 12 passed。

- [ ] **Step 5: 门禁 + 提交**

Run GATE（见 Global Constraints）。
```bash
git add crates/embedded-database-flash
git commit -m "feat: 实现擦除块级 FTL 的几何校验、块头与挂载重建"
```

---

## Task 3: FTL 回收、磨损均衡与统计

**Files:**
- Modify: `crates/embedded-database-flash/src/ftl.rs`

**Interfaces:**
- Produces: `FtlStats`（公开字段见下）、`Ftl::stats(&self) -> FtlStats`、`impl BlockDevice::discard`。
- Consumes: Task 2 的 `Phys`/`BlockState`/`alloc_block`/`reclaim_one_dirty`。

- [ ] **Step 1: 写失败的测试**

在 `ftl.rs` 的 `mod tests` 里追加：

```rust
    #[test]
    fn discard_后整块变为可回收且数据读出全_ff() {
        let mut f = ftl();
        f.write_at(0, &alloc::vec![0x55u8; LOGICAL_BLOCK]).unwrap();
        f.discard(0, LOGICAL_BLOCK as u64).unwrap();
        let mut got = alloc::vec![0u8; LOGICAL_BLOCK];
        f.read_at(0, &mut got).unwrap();
        assert_eq!(got, alloc::vec![0xFFu8; LOGICAL_BLOCK]);
    }

    #[test]
    fn discard_部分覆盖不动数据() {
        let mut f = ftl();
        f.write_at(0, &alloc::vec![0x55u8; LOGICAL_BLOCK]).unwrap();
        f.discard(0, 100).unwrap();
        let mut got = alloc::vec![0u8; 16];
        f.read_at(0, &mut got).unwrap();
        assert_eq!(got, alloc::vec![0x55u8; 16]);
    }

    #[test]
    fn 空闲块耗尽时回收脏块而不是报容量不足() {
        // 5 个物理块、OP 20% ⇒ 逻辑 4 块。反复覆写制造脏块。
        let raw = SimFlash::new(5, PAGE, BLOCK);
        let mut f = Ftl::new(raw, FtlConfig::default()).unwrap();
        assert_eq!(f.logical_size(), 4 * LOGICAL_BLOCK as u64);
        for round in 0..20u8 {
            for lb in 0..4u64 {
                f.write_at(lb * LOGICAL_BLOCK as u64, &alloc::vec![round; 128])
                    .unwrap();
            }
        }
        let mut got = alloc::vec![0u8; 128];
        f.read_at(0, &mut got).unwrap();
        assert_eq!(got, alloc::vec![19u8; 128]);
        assert!(f.stats().erase_ops > 0);
    }

    #[test]
    fn 统计不变量_空闲加数据加脏加坏等于物理块数() {
        let mut f = ftl();
        for lb in 0..16u64 {
            f.write_at(lb * LOGICAL_BLOCK as u64, &alloc::vec![1u8; 64]).unwrap();
        }
        let s = f.stats();
        assert_eq!(s.free_blocks + s.retired_blocks + s.bad_blocks + 16, NBLOCKS);
    }

    #[test]
    fn 坏块被跳过且数据仍可读() {
        let raw = SimFlash::new(16, PAGE, BLOCK);
        let mut raw = raw;
        // 先把 0 号块用一次，制造一个"已经用过"的块，再标坏。
        raw.program(0, 0, &[0x00]).unwrap();
        raw.mark_bad(0);
        let mut f = Ftl::new(raw, FtlConfig::default()).unwrap();
        f.write_at(0, &alloc::vec![0x42u8; 3840]).unwrap();
        let mut got = alloc::vec![0u8; 3840];
        f.read_at(0, &mut got).unwrap();
        assert_eq!(got, alloc::vec![0x42u8; 3840]);
        assert_eq!(f.stats().bad_blocks, 1);
    }

    #[test]
    fn 覆写场景下写放大大于一() {
        let mut f = ftl();
        for i in 0..500u64 {
            f.write_at(0, &alloc::vec![(i & 0xFF) as u8; 64]).unwrap();
        }
        // 每次覆写落在同一物理块上、被 NOR 0→1 拒绝，于是触发整块搬迁。
        assert!(f.stats().write_amplification > 1.0);
        assert!(f.stats().block_moves > 0);
    }
```

- [ ] **Step 2: 运行确认失败**

Run: `cargo test -p embedded-database-flash 2>&1 | tail -20`
Expected: 编译失败（`FtlStats` / `stats` / `discard` 未定义）。

- [ ] **Step 3: 实现统计、回收与 discard**

（`FtlStats` 已在 Task 2 定义。）在 `impl<R: RawFlash> Ftl<R>` 内追加：

```rust
    /// 当前统计快照。
    pub fn stats(&self) -> FtlStats {
        self.stats
    }

    /// 静态磨损均衡：把擦除计数最低的数据块搬到新块，平衡擦除分布。
    ///
    /// 触发条件（任一成立）：累计擦除次数是 `static_level_interval` 的整数倍，
    /// 或空闲块的擦除计数极差超过 `wear_gap_threshold`。两个参数为 0 时该项关闭。
    fn maybe_static_level(&mut self) -> Result<()> {
        // 递归护栏：搬迁可能触发擦除，擦除又会调用本函数。
        if self.leveling {
            return Ok(());
        }
        // 至少保留两个空闲块：静态均衡属于后台整理，不能吃掉常规写入
        // （动态分配）所需的余量，否则均衡后的首次写入会 CapacityExhausted。
        if self.stats.free_blocks < 2 {
            return Ok(());
        }
        let by_interval = self.cfg.static_level_interval != 0
            && self.stats.erase_ops % self.cfg.static_level_interval == 0;
        let by_gap = self.cfg.wear_gap_threshold != 0
            && self
                .stats
                .erase_count_max
                .saturating_sub(self.stats.erase_count_min)
                > self.cfg.wear_gap_threshold;
        if !(by_interval || by_gap) {
            return Ok(());
        }
        let cold = self
            .phys
            .iter()
            .enumerate()
            .filter(|(_, p)| p.state == BlockState::Data)
            .min_by_key(|(_, p)| p.erase_count)
            .map(|(i, _)| i as u32);
        let Some(cold) = cold else {
            return Ok(());
        };
        let logical = self.phys[cold as usize].logical;
        if logical == UNMAPPED {
            return Ok(());
        }
        // 搬迁整块：读出全部载荷再写进新块（relocate_all 会置旧块为 Dirty）。
        self.leveling = true;
        let result = self.relocate_all(logical);
        self.leveling = false;
        result
    }

    /// 搬迁整个逻辑块到新物理块（不改变内容）。
    fn relocate_all(&mut self, logical: u32) -> Result<()> {
        let old = self.l2p[logical as usize];
        if old == UNMAPPED {
            return Ok(());
        }
        let mut buf = alloc::vec![0xFFu8; self.logical_block_size];
        for (i, chunk) in buf.chunks_mut(self.page_size).enumerate() {
            self.raw
                .read(old, self.page_size + i * self.page_size, chunk)
                .map_err(|_| Error::Io)?;
        }
        let np = self.alloc_block(logical)?;
        self.program_payload(np, &buf)?;
        self.l2p[logical as usize] = np;
        self.commit_header(np)?;
        self.phys[old as usize] = Phys { state: BlockState::Dirty, ..self.phys[old as usize] };
        self.stats.block_moves += 1;
        Ok(())
    }
```

`stats()` 是「上一次操作边界」的快照（各写入/擦除/discard 路径末尾都会刷新），
其文档注释请写成「返回最近一次完成的写/擦除/discard 之后的统计快照」，
避免使用者以为它在读取时做全量重算。

把 `erase_block` 的 `Ok(())` 分支改为「擦除成功后触发静态均衡」：

```rust
            Ok(()) => {
                let idx = block as usize;
                self.phys[idx] = Phys {
                    state: BlockState::Free,
                    logical: UNMAPPED,
                    alloc_seq: 0,
                    erase_count: self.phys[idx].erase_count.saturating_add(1),
                };
                self.stats.erase_ops += 1;
                self.refresh_stats();
                // 静态均衡：失败不影响本次擦除（块已变 Free）。
                let _ = self.maybe_static_level();
                Ok(())
            }
```

在 `impl<R: RawFlash> Ftl<R>` 里（**固有**方法——core 的 `BlockDevice::discard` 要到 Task 4 才存在；Task 4 会把它移进 trait 实现）追加 `discard`：

```rust
    fn discard(&mut self, offset: u64, len: u64) -> Result<()> {
        let end = offset.checked_add(len).ok_or(Error::InvalidArgument)?;
        if end > self.logical_size() {
            return Err(Error::InvalidArgument);
        }
        let lsize = self.logical_block_size as u64;
        // 仅回收被完整覆盖的逻辑块；部分覆盖不动作、不报错。
        let first = offset.div_ceil(lsize);
        let last = end / lsize; // 开区间：不完整的尾块不计
        for lb in first..last {
            let li = lb as usize;
            let phys = self.l2p[li];
            if phys == UNMAPPED {
                continue;
            }
            self.l2p[li] = UNMAPPED;
            if self.phys[phys as usize].state == BlockState::Data {
                // 设计 §3.5：退役即擦除，擦除计数 +1 后回到空闲池。
                self.erase_block(phys)?;
            }
        }
        self.refresh_stats();
        Ok(())
    }
```

在每个 `return Ok(())` 之后需要最新统计的地方（`write_chunk`/`discard`/`relocate*`）调用 `self.refresh_stats()`；**最简做法**：在 `BlockDevice::write_at` 与 `discard` 的末尾各调一次 `self.refresh_stats()`，并保留 `new` 里的调用。

- [ ] **Step 4: 运行测试**

Run: `cargo test -p embedded-database-flash`
Expected: 18 passed（Task 1: 5 + Task 2: 7 + Task 3: 6）。

- [ ] **Step 5: 门禁 + 提交**

Run GATE。
```bash
git add crates/embedded-database-flash
git commit -m "feat: FTL 增加块回收、静态磨损均衡与统计面"
```

---

## Task 4: core `BlockDevice::discard` 默认方法 + `InMemoryDevice` 记录

**Files:**
- Modify: `crates/embedded-database-core/src/device.rs`

**Interfaces:**
- Produces: `BlockDevice::discard(&mut self, offset: u64, len: u64) -> Result<(), Error>`（默认 `Ok(())`）；`InMemoryDevice::discarded_ranges(&self) -> &[(u64, u64)]`。
- Consumes: 无。

- [ ] **Step 1: 写失败的测试**

在 `device.rs` 的 `mod tests` 里追加：

```rust
    #[test]
    fn discard_越界失败() {
        let mut dev = crate::device::InMemoryDevice::new(4096);
        assert!(dev.discard(0, 4096).is_ok());
        assert_eq!(dev.discard(0, 4097), Err(crate::Error::InvalidArgument));
        assert_eq!(dev.discard(4096, 1), Err(crate::Error::InvalidArgument));
    }

    #[test]
    fn 内存设备记录_discard_区间且不改数据() {
        let mut dev = InMemoryDevice::new(8192);
        dev.write_at(0, &[1, 2, 3, 4]).unwrap();
        dev.discard(0, 4).unwrap();
        dev.discard(4096, 4096).unwrap();
        let mut got = [0u8; 4];
        dev.read_at(0, &mut got).unwrap();
        assert_eq!(got, [1, 2, 3, 4]);
        assert_eq!(dev.discarded_ranges(), &[(0, 4), (4096, 4096)]);
    }
```

（这两个测试写在 `device.rs` 既有的 `mod tests`（`:171`）里，可直接用 `InMemoryDevice` / `Error`。）

- [ ] **Step 2: 运行确认失败**

Run: `export PATH="/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH"; cargo test -p embedded-database-core --features std 2>&1 | tail -20`
Expected: 编译失败（`discard` 不是 `BlockDevice` 的成员 / `discarded_ranges` 未定义）。

- [ ] **Step 3: 实现**

在 `device.rs` 的 `BlockDevice` trait 定义里（`sync` 之后）加：

```rust
    /// 通知设备 `[offset, offset + len)` 的内容已作废，可回收（TRIM/discard）。
    ///
    /// 默认实现是空操作，既有实现无需改动。设备**不得**因 discard 改变
    /// 读出的数据（语义只是"提示可回收"），越界失败返回
    /// [`Error::InvalidArgument`]。
    fn discard(&mut self, offset: u64, len: u64) -> Result<(), Error> {
        let end = offset.checked_add(len).ok_or(Error::InvalidArgument)?;
        if end > self.size() {
            return Err(Error::InvalidArgument);
        }
        let _ = (offset, len);
        Ok(())
    }
```

给内存设备加字段 `discarded: Vec<(u64, u64)>`（初始化 `Vec::new()`）与：

```rust
    /// 已收到的 discard 区间（按调用顺序），供测试断言。
    pub fn discarded_ranges(&self) -> &[(u64, u64)] {
        &self.discarded
    }
```

并覆写 `discard`：越界检查后 `self.discarded.push((offset, len)); Ok(())`（**不修改**字节内容）。

- [ ] **Step 4: 运行测试**

Run: `cargo test -p embedded-database-core --features std`
Expected: 全绿（216 + 2 新增 = 218 passed）。

- [ ] **Step 5: 让 FTL 也实现该 trait 方法**

Task 3 把 `discard` 写成了 `Ftl` 的**固有方法**（当时 `BlockDevice` 还没有该成员）。
core 现在有了 trait 方法，必须把它搬进 trait 实现，否则 `&mut dyn BlockDevice`
（`Db::dev` 的类型）上的 `discard` 会落到 trait 默认空实现，FTL 永远收不到回收提示。

在 `crates/embedded-database-flash/src/ftl.rs`：把 `impl<R: RawFlash> Ftl<R>` 里那段
`pub fn discard(...)` 整段删掉，原样搬进 `impl<R: RawFlash> BlockDevice for Ftl<R>`
（去掉 `pub`，trait 实现里直接写 `fn discard(&mut self, offset: u64, len: u64) -> Result<()>`）。
Task 3 的两个 discard 测试调用的是具体类型 `Ftl`，经 trait 解析照常通过；**前提**是测试模块
已 `use embedded_database_core::BlockDevice`（Task 2/3 已如此，若报「method not found」就补上该 `use`）。

Run: `cargo test -p embedded-database-flash`
Expected: 18 passed（数量不变）。

- [ ] **Step 6: 门禁 + 提交**

Run GATE。
```bash
git add crates/embedded-database-core/src/device.rs crates/embedded-database-flash/src/ftl.rs
git commit -m "feat: BlockDevice 增加 discard 默认方法并提供内存设备记录"
```

---

## Task 5: core 段槽轮转 + 环感知恢复

**Files:**
- Modify: `crates/embedded-database-core/src/recovery.rs`
- Modify: `crates/embedded-database-core/src/db.rs`

**Interfaces:**
- Produces: `RecoveryReport::first_valid_segment: Option<u64>`；`Db::slot_offset(cfg, seq) -> Result<u64, Error>`；`Db` 字段 `first_live_segment: Option<u64>`。
- Consumes: Task 2 的 `SegmentHeader::decode`（core 内部，已有）。

- [ ] **Step 1: 写失败的测试**

在 `db.rs` 的 `mod tests` 里追加：

```rust
    #[test]
    fn 中间槽被擦除后仍能恢复后续段() {
        // 每段最多 8 页（max_pages=8）：写 17 次 ⇒ 占满段 1、段 2，段 3 起头。
        let cfg = Config::default();
        let dev_size = (cfg.max_segments as u64) * (cfg.segment_size as u64);
        let mut dev = InMemoryDevice::new(dev_size as usize);
        {
            let mut db = Db::open(&mut dev, cfg.clone()).unwrap();
            for i in 0..17u64 {
                db.put(format!("k{i}").as_bytes(), b"v").unwrap();
                db.flush().unwrap();
            }
        }
        // 擦掉第 1 槽（段 1）的段头 → 该段不可解码（环回时的空洞）。
        dev.write_at(0, &[0xFFu8; 64]).unwrap();
        let db = Db::open(&mut dev, cfg).unwrap();
        // 空洞之后的段仍被恢复：最新键可读，重开不报损坏。
        assert_eq!(db.get(b"k16").unwrap(), Some(b"v".to_vec()));
        // 段 1 里的键随该段一起丢失。
        assert!(db.get(b"k0").unwrap().is_none());
    }

    #[test]
    fn 段号超过槽数后重开仍读到最新数据() {
        let cfg = Config::default();
        let dev_size = (cfg.max_segments as u64) * (cfg.segment_size as u64);
        let mut dev = InMemoryDevice::new(dev_size as usize);
        {
            let mut db = Db::open(&mut dev, cfg.clone()).unwrap();
            // 5 段（≥ 槽数 4 + 1），环回到第 0 槽。
            for i in 0..40u64 {
                db.put(format!("k{i}").as_bytes(), b"v").unwrap();
                db.flush().unwrap();
            }
        }
        let db = Db::open(&mut dev, cfg).unwrap();
        assert_eq!(db.get(b"k39").unwrap(), Some(b"v".to_vec()));
    }
```

（测试在 `db.rs` 的 `mod tests` 内，可直接访问私有字段/方法：`db.active_segment`、`db.first_live_segment`。）

- [ ] **Step 2: 运行确认失败**

Run: `cargo test -p embedded-database-core --features std 2>&1 | tail -30`
Expected: 两个新测试失败/编译失败（`segment_seq` 断言为 5 而非 9；第二个测试因 `break` 语义拿不到 k2）。

- [ ] **Step 3: 改 `recovery.rs`**

1. `RecoveryReport` 增字段：

```rust
    /// 第一个有效提交的段序列号。无有效提交则为 `None`。
    pub first_valid_segment: std::option::Option<u64>,
```

并在 `recover` 的初始化里加 `first_valid_segment: std::option::Option::None,`。

2. 把 `recover` 的循环体替换为：

```rust
    let mut prev_actual_crc: std::option::Option<u32> = None;
    let mut prev_seq: std::option::Option<u64> = None;
    let mut first_valid: std::option::Option<u64> = None;

    for cfg in segments {
        // 链校验只在"物理相邻"时生效：段号跳变（环回/空洞）视为链起点。
        let adjacent = prev_seq.map_or(true, |s| cfg.segment_seq == s + 1);
        match scan_segment(dev, cfg, &mut report) {
            Ok(std::option::Option::Some((actual_crc, disk_prev_crc))) => {
                if adjacent {
                    if let Some(expected) = prev_actual_crc {
                        if disk_prev_crc != expected {
                            report.corrupted_segment = std::option::Option::Some(cfg.segment_seq);
                            return Err(Error::Corruption { segment_seq: cfg.segment_seq });
                        }
                    }
                }
                prev_actual_crc = std::option::Option::Some(actual_crc);
                prev_seq = std::option::Option::Some(cfg.segment_seq);
                if first_valid.is_none() {
                    first_valid = std::option::Option::Some(cfg.segment_seq);
                }
                report.last_valid_segment = std::option::Option::Some(cfg.segment_seq);
                report.last_commit_crc = actual_crc;
            }
            // 空槽/擦除槽/截断段：跳过而不是停止——环回后它们出现在中间。
            Ok(std::option::Option::None) => {
                prev_actual_crc = std::option::Option::None;
                prev_seq = std::option::Option::Some(cfg.segment_seq);
            }
            Err(e) => {
                report.corrupted_segment = std::option::Option::Some(cfg.segment_seq);
                return Err(e);
            }
        }
    }
    report.first_valid_segment = first_valid;
```

3. 更新 `recover` 的文档注释：把"任一环节不符即停止"改为"空洞/坏槽被跳过，链校验只在段号相邻时生效"。

4. `recovery.rs` **自带的测试**（`mod tests`，约 `:280-460`）里凡断言"遇到未提交段即停止扫描、其后的段一律不可见"的用例，改为新语义下的期望（空洞后的段仍可见）；凡断言链断裂（篡改 `prev_crc`）报 `Corruption` 的用例必须**继续通过且保持精确断言**（相邻段仍校验）。测试模块内可直接给 `RecoveryReport` 的 `first_valid_segment` 加断言。

- [ ] **Step 4: 改 `db.rs` 的 `open`**

把 `db.rs:108-152` 替换为：

```rust
        let dev_size = dev.size();
        let seg_size_u64 = cfg.segment_size as u64;
        if seg_size_u64 > dev_size {
            // M8-3：第一个段槽位都放不下 ⇒ 设备与 segment_size 不匹配。
            return Err(Error::InvalidArgument);
        }
        let slots = core::cmp::min(
            cfg.max_segments as u64,
            dev_size / seg_size_u64,
        ) as usize;

        // 环语义：槽位不携带段号，段号写在段头里。逐个槽读段头，
        // 可解码的按段号排序后交给 recover（链校验只对相邻段生效）。
        let mut collected: std::vec::Vec<(u64, usize)> = std::vec::Vec::new();
        for slot in 0..slots {
            let off = (slot as u64)
                .checked_mul(seg_size_u64)
                .ok_or(Error::InvalidArgument)?;
            let mut hbuf = [0u8; SEGMENT_HEADER_SIZE];
            if dev.read_at(off, &mut hbuf).is_err() {
                continue;
            }
            match crate::segment::SegmentHeader::decode(&hbuf) {
                Ok(h) => collected.push((h.segment_seq, slot)),
                Err(_) => continue,
            }
        }
        collected.sort_by_key(|(seq, _)| *seq);
        collected.dedup_by_key(|(seq, _)| *seq);
        let mut segment_cfgs = std::vec::Vec::new();
        for (seq, slot) in collected {
            let device_offset = (slot as u64)
                .checked_mul(seg_size_u64)
                .ok_or(Error::InvalidArgument)?;
            segment_cfgs.push(SegmentConfig {
                segment_seq: seq,
                prev_segment: seq.saturating_sub(1),
                prev_crc: 0,
                device_offset,
                segment_size: cfg.segment_size,
                page_size: cfg.page_size,
                slots_per_page: cfg.slots_per_page,
                max_pages: cfg.max_pages,
                encryption_key: cfg.encryption_key,
            });
        }

        let report = recover(dev, &segment_cfgs)?;
        let active_segment = report.last_valid_segment.unwrap_or(0).saturating_add(1);
        let first_live_segment = report.first_valid_segment;
        let last_commit_crc = report.last_commit_crc;
```

同时把 `page_map` 重建的循环条件 `cfg_seg.segment_seq > active_segment.saturating_sub(1)` 改为：

```rust
            match report.last_valid_segment {
                Some(last) if cfg_seg.segment_seq <= last => {}
                _ => continue,
            }
```

给 `Db` 结构体加字段 `first_live_segment: std::option::Option<u64>,`，在 `open` 末尾的结构体字面量里填 `first_live_segment,`，并加：

```rust
    /// 槽位偏移：段号从 1 开始，环回取模。
    fn slot_offset(cfg: &Config, seq: u64) -> Result<u64, Error> {
        let slot = seq.saturating_sub(1) % cfg.max_segments.max(1) as u64;
        (slot as usize)
            .checked_mul(cfg.segment_size)
            .map(|v| v as u64)
            .ok_or(Error::InvalidArgument)
    }
```

并把 `flush` 与 `compact_with_budget` 里的偏移算式替换为 `Self::slot_offset(&self.cfg, segment_seq)?`（分别对应原 `db.rs:434`、`db.rs:708` 两处 `device_offset` 计算）。

- [ ] **Step 5: 运行全量 core 测试并修复受影响的旧用例**

Run: `cargo test -p embedded-database-core --features std`
Expected: 除本任务两个新测试外全绿。若有旧用例编码了"第 N 段必在第 N 槽"或"段用完即 CapacityExhausted"的假设，逐个改为环语义下的新期望（**不得**放宽为 `assert!(result.is_ok())` 之类的弱断言；应改为精确断言新槽位/新段号）。

- [ ] **Step 6: 门禁 + 提交**

Run GATE。
```bash
git add crates/embedded-database-core/src/recovery.rs crates/embedded-database-core/src/db.rs
git commit -m "feat: core 段槽按段号取模并让恢复容忍环回空洞"
```

---

## Task 6: core 自动合并 + 退役段 discard + `max_segments >= 3`

**Files:**
- Modify: `crates/embedded-database-core/src/types.rs`
- Modify: `crates/embedded-database-core/src/db.rs`

**Interfaces:**
- Produces: `Db::ensure_slot_free(&mut self) -> Result<(), Error>`、`Db::compact_locked(&mut self) -> Result<CompactReport, Error>`（无锁，供内部复用）。
- Consumes: Task 4 的 `BlockDevice::discard`、Task 5 的 `first_live_segment` / `slot_offset`。

- [ ] **Step 1: 写失败的测试**

在 `types.rs` 与 `db.rs` 的 `mod tests` 分别追加：

```rust
    #[test]
    fn max_segments_小于_3_被拒() {
        let cfg = Config { max_segments: 2, ..Config::default() };
        assert_eq!(cfg.validate(), Err(Error::InvalidArgument));
        let cfg = Config { max_segments: 3, ..Config::default() };
        assert!(cfg.validate().is_ok());
    }
```

```rust
    #[test]
    fn 存活段数达到上限时自动合并且不丢数据() {
        let cfg = Config::default();
        let dev_size = (cfg.max_segments as u64) * (cfg.segment_size as u64);
        let mut dev = InMemoryDevice::new(dev_size as usize);
        let mut db = Db::open(&mut dev, cfg.clone()).unwrap();
        // 每段 8 页：40 次 flush ⇒ 至少需要 5 个段槽，但设备只有 4 个。
        for i in 0..40u64 {
            db.put(format!("k{i}").as_bytes(), b"payload").unwrap();
            db.flush().unwrap();
        }
        // 合并保留最新版本：全部键可读。
        for i in 0..40u64 {
            assert_eq!(db.get(format!("k{i}").as_bytes()).unwrap(), Some(b"payload".to_vec()));
        }
        // 不变量：存活段数 ≤ max_segments - 1。
        let first = db.first_live_segment.expect("有已提交数据必有存活段");
        assert!(db.active_segment - first <= cfg.max_segments as u64 - 1);
    }

    #[test]
    fn 合并后对退役段发出_discard() {
        let cfg = Config::default();
        let dev_size = (cfg.max_segments as u64) * (cfg.segment_size as u64);
        let mut dev = InMemoryDevice::new(dev_size as usize);
        {
            let mut db = Db::open(&mut dev, cfg.clone()).unwrap();
            for i in 0..40u64 {
                db.put(format!("k{i}").as_bytes(), b"payload").unwrap();
                db.flush().unwrap();
            }
        }
        assert!(!dev.discarded_ranges().is_empty(), "合并后必须对退役段发 discard");
        for (_, len) in dev.discarded_ranges() {
            assert_eq!(*len, cfg.segment_size as u64);
        }
    }
```

- [ ] **Step 2: 运行确认失败**

Run: `cargo test -p embedded-database-core --features std 2>&1 | tail -30`
Expected: `max_segments < 3` 测试失败（当前返回 Ok）；自动合并测试失败（`CapacityExhausted`）。

- [ ] **Step 3: 改 `types.rs`**

在 `validate` 的 `max_segments == 0` 判断之后加：

```rust
        // 段槽环需要至少 1 个空闲槽位才能开新段（存活段数上限 max_segments - 1），
        // 且合并目标槽必须空闲：3 是能同时容纳"合并前"与"合并后"的最小值。
        if self.max_segments < 3 {
            return Err(Error::InvalidArgument);
        }
```

- [ ] **Step 4: 改 `db.rs`**

1. 把 `compact_with_budget` 的锁与主体拆开。**保持公开方法现有的签名与 deadline 判定逻辑原样不动**（现签名为 `pub fn compact_with_budget(&mut self, budget: &CompactionBudget, deadline: Option<Deadline>) -> Result<CompactReport, Error>`），只在 `self.active_writer = None;` **之前**插入锁获取、在 deadline 判定之后改为调用内部方法：

```rust
    pub fn compact_with_budget(
        &mut self,
        budget: &CompactionBudget,
        deadline: Option<Deadline>,
    ) -> Result<CompactReport, Error> {
        self.try_lock_writer()?;
        // ← 此处保留原有的 deadline 判定代码（过期时 unlock_writer 并按
        //   budget.interruptible 返回 interrupted 报告或 DeadlineExceeded），
        //   原样不动。
        let result = self.compact_locked();
        self.unlock_writer();
        result
    }

    /// 合并实现（**调用方必须持有 writer 锁**，或从 `flush` 内部调用）。
    ///
    /// 由原 `compact_with_budget` 的「`self.active_writer = None;` 到函数末尾
    /// `Ok(CompactReport { .. })`」整段搬来，**并删掉该段内所有
    /// `self.unlock_writer()` 调用**（解锁交由调用方负责）。
    fn compact_locked(&mut self) -> Result<CompactReport, Error> {
        // （原主体：退役 writer → 收集 latest → 计算 new_segment_seq/device_offset
        //   → write_new_segment → 更新 page_map/key_map → active_segment += 1）
    }
```

2. 在 `compact_locked` 成功提交新段（`active_segment` 更新为 `new_segment_seq + 1` 处，原 `db.rs:762` 附近）之后，追加退役段的 discard 与 first_live 更新：

```rust
        if let Some(first) = self.first_live_segment {
            let mut seq = first;
            while seq < merged_seq {
                let off = Self::slot_offset(&self.cfg, seq)?;
                self.dev.discard(off, self.cfg.segment_size as u64)?;
                seq += 1;
            }
        }
        self.first_live_segment = Some(merged_seq);
```

（`merged_seq` = 本次合并写入的段号，即原 `new_segment_seq`。）

3. 加 `ensure_slot_free`：

```rust
    /// 若存活段数已达上限（max_segments - 1），先合并一次腾出槽位。
    ///
    /// 必须在**写新段之前**调用；`flush` 复用当前段时不需要（仍持有一个槽）。
    fn ensure_slot_free(&mut self) -> Result<(), Error> {
        let live = match self.first_live_segment {
            Some(first) => self.active_segment.saturating_sub(first),
            None => 0,
        };
        if live >= self.cfg.max_segments as u64 - 1 {
            self.compact_locked()?;
        }
        Ok(())
    }
```

4. 在 `flush` 里，段选择之后、计算 `device_offset` 之前插入：

```rust
        if !reuse {
            self.ensure_slot_free()?;
            segment_seq = self.active_segment;
            prev_crc = self.last_commit_crc;
        }
        let device_offset = Self::slot_offset(&self.cfg, segment_seq)?;
```

（把原有的 `let (segment_seq, reuse, prev_crc) = …` 改成 `let (mut segment_seq, mut reuse, mut prev_crc) = …`；若 `reuse` 后续未被再赋值，用 `let (mut segment_seq, reuse, mut prev_crc)` 避免 `unused_mut` 警告。）

- [ ] **Step 5: 运行测试并修复受影响用例**

Run: `cargo test -p embedded-database-core --features std`，然后 `cargo test --workspace --features embedded-database-core/std,embedded-database-embed/std`
Expected: 全绿。注意 `B1_槽位耗尽后flush写入新段而非覆写满段`（`db.rs:2131`）：该用例第 9 次 flush 只产生 1 个存活段，不触发合并，断言（落到段 2）**应仍然成立**；若因偏移取模导致断言文本需微调，改断言数值而**不删用例**。`crates/embedded-database-embed/src/lib.rs:372` 附近的注释提到"4MiB = max_segments × segment_size"，随实现确认后按需更新注释。

- [ ] **Step 6: 槽位复用前清空追加区（环的正确性前提）**

**背景（Task 5 评审发现的 Important 隐患，必须在此闭环）**：`scan_segment` 顺序扫描追加区，
遇到全零槽或页头不可解码才停。环回后一个槽位被新段复用时，若上一租期的页比新段多，
扫描会继续读到**上一租期遗留的合法页**，把它们折叠进 `page_crc_fold` ⇒ 与提交记录不符
⇒ `Db::open` 直接 `Err(Corruption)`（硬失败，不是静默损坏）。Task 5 的新用例
`段号超过槽数后重开仍读到最新数据` 之所以没暴露它，是因为两次租用写了**同样多的页**。

**不能靠 `discard` 解决**：`BlockDevice::discard` 的契约是「设备不得因 discard 改变读出的
数据」（Task 4 的 trait 文档），所以 core 不许把「discard 后读回 0xFF」当前提；FTL 恰好
擦除是它的实现细节。⇒ 由 core 自己保证：**开新段时把追加区清零**。

先写失败用例（`db.rs` 的 `mod tests` 内，紧跟 Task 5 新增的两个环用例）：

```rust
    #[test]
    fn 槽位复用后短段不会读到旧页() {
        let cfg = Config::default();
        let dev_size = (cfg.max_segments as u64) * (cfg.segment_size as u64);
        let mut dev = InMemoryDevice::new(dev_size as usize);
        {
            let mut db = Db::open(&mut dev, cfg.clone()).unwrap();
            // 首轮：写满 4 个段（每段 8 页）⇒ 槽 0 的旧租期留下 8 页。
            for i in 0..32u64 {
                db.put(format!("a{i}").as_bytes(), b"v").unwrap();
                db.flush().unwrap();
            }
        }
        // 擦掉全部段头，让 4 个槽都变成「空洞」——恢复会跳过它们。
        for slot in 0..cfg.max_segments as u64 {
            dev.write_at(slot * cfg.segment_size as u64, &[0xFFu8; 64]).unwrap();
        }
        // 第二轮：只写 3 次 flush ⇒ 槽 0 的新租期（段 1）只占 3 页，远少于旧的 8 页。
        {
            let mut db = Db::open(&mut dev, cfg.clone()).unwrap();
            for i in 0..3u64 {
                db.put(format!("b{i}").as_bytes(), b"v").unwrap();
                db.flush().unwrap();
            }
        }
        // 重开：若未清零，扫描会在第 4 页读到旧租期的合法页 ⇒ fold 不符 ⇒ Err(Corruption)。
        let db = Db::open(&mut dev, cfg).unwrap();
        assert_eq!(db.get(b"b2").unwrap(), Some(b"v".to_vec()));
        assert!(db.get(b"b3").unwrap().is_none());
    }
```

Run: `export PATH="/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH"; cargo test -p embedded-database-core --features std 槽位复用后短段不会读到旧页 2>&1 | tail -20`
Expected: 失败（`Err(Corruption)` 或断言失败）。

然后在 `crates/embedded-database-core/src/writer.rs` 的 `SegmentWriter::new` 里，**写完段头之后**追加清零
（`append_start` 与 `total_run_bytes` 已在函数内算好；`cfg.max_pages` 已是 `usize`）：

```rust
        // 环回复用槽位时必须清空追加区：上一租期可能留下更多页，恢复的顺序扫描
        // 会把它们折进 fold 而判定整段损坏（见计划 Task 6 Step 6）。清零后扫描
        // 遇到全零槽即自然终止。写 0 是 NOR 允许的（1→0），FTL 侧不会因此搬迁。
        let body = std::vec![0u8; total_run_bytes];
        let body_off = cfg
            .device_offset
            .checked_add(append_start as u64)
            .ok_or(Error::InvalidArgument)?;
        dev.write_at(body_off, &body)?;
```

（`writer.rs` 的 `std::vec!` 用法与文件内既有写法一致；若该模块没有 `extern crate alloc` 的
`vec!` 宏可见性，用 `alloc::vec![0u8; total_run_bytes]`。）

再跑同一个用例，Expected: 通过。同时确认 `crates/embedded-database-core/src/writer.rs` 与
`recovery.rs` 的既有用例没有被这次「多写一段零」破坏（若有用例断言「writer 只写段头字节数」
或 `WriteStats` 精确值，按新事实更新数值并**保留用例**）。

- [ ] **Step 7: 门禁 + 提交**

Run GATE。
```bash
git add crates/embedded-database-core/src/types.rs crates/embedded-database-core/src/db.rs crates/embedded-database-core/src/writer.rs
git commit -m "feat: core 存活段达上限时自动合并、对退役段发 discard 并清零复用槽位"
```

---

## Task 7: `embedded-database-os` 的 `FileRawFlash` 与文件掉电复验

**Files:**
- Modify: `crates/embedded-database-os/Cargo.toml`
- Modify: `crates/embedded-database-os/src/lib.rs`
- Create: `crates/embedded-database-os/src/flash.rs`

**Interfaces:**
- Produces: `embedded_database_os::flash::FileRawFlash`：`open(path: &Path, block_count: u32, page_size: usize, erase_block_size: usize) -> io::Result<Self>`、`open_db_on_file(...)` 级别的辅助不需要；`fail_after(n: u64)`（第 n 次编程起失败）、`impl RawFlash for FileRawFlash`。
- Consumes: `embedded_database_flash::raw::{RawFlash, FlashError, FlashResult}`、`embedded_database_flash::ftl::Ftl`。

- [ ] **Step 1: 加依赖**

`crates/embedded-database-os/Cargo.toml` 的 `[dependencies]` 加 `embedded-database-flash = { workspace = true }`。

- [ ] **Step 2: 实现 `FileRawFlash` 并附测试**

创建 `crates/embedded-database-os/src/flash.rs`：

```rust
//! 文件支撑的 `RawFlash`：用普通文件模拟 NOR 闪存，供跨进程/掉电复验。

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use embedded_database_flash::raw::{FlashError, FlashResult, RawFlash};

/// 基于文件的 NOR 闪存模拟。
///
/// 语义与 `wasser` 的 `SimFlash` 一致（字节级 1→0、整块擦除、页对齐编程），
/// 额外支持"第 N 次编程起失败"以模拟断电。
pub struct FileRawFlash {
    file: File,
    path: PathBuf,
    page_size: usize,
    erase_block_size: usize,
    block_count: u32,
    programs: u64,
    fail_after: Option<u64>,
}

impl FileRawFlash {
    /// 打开或创建一个容量为 `block_count × erase_block_size` 的文件。
    /// 新文件初始化为全 `0xFF`（擦除态）。
    pub fn open(
        path: &Path,
        block_count: u32,
        page_size: usize,
        erase_block_size: usize,
    ) -> std::io::Result<Self> {
        let total = block_count as u64 * erase_block_size as u64;
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;
        if file.metadata()?.len() != total {
            file.set_len(total)?;
        }
        Ok(FileRawFlash {
            file,
            path: path.to_path_buf(),
            page_size,
            erase_block_size,
            block_count,
            programs: 0,
            fail_after: None,
        })
    }

    /// 使第 `n` 次（从 0 计数）及之后的编程全部失败，模拟断电。
    pub fn fail_after(&mut self, n: u64) {
        self.fail_after = Some(n);
    }

    /// 文件路径。
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 重新打开（模拟进程重启）。
    pub fn reopen(self) -> std::io::Result<Self> {
        FileRawFlash::open(
            &self.path,
            self.block_count,
            self.page_size,
            self.erase_block_size,
        )
    }

    fn base(&self, block: u32) -> u64 {
        u64::from(block) * self.erase_block_size as u64
    }

    fn check(&self, block: u32, offset: usize, len: usize) -> FlashResult<()> {
        if block >= self.block_count {
            return Err(FlashError::OutOfRange);
        }
        if offset > self.erase_block_size || len > self.erase_block_size - offset {
            return Err(FlashError::OutOfRange);
        }
        Ok(())
    }
}

impl RawFlash for FileRawFlash {
    fn page_size(&self) -> usize {
        self.page_size
    }
    fn erase_block_size(&self) -> usize {
        self.erase_block_size
    }
    fn block_count(&self) -> u32 {
        self.block_count
    }

    fn read(&self, block: u32, offset: usize, buf: &mut [u8]) -> FlashResult<()> {
        self.check(block, offset, buf.len())?;
        let mut f = &self.file;
        f.seek(SeekFrom::Start(self.base(block) + offset as u64))
            .map_err(|_| FlashError::Read { block })?;
        f.read_exact(buf).map_err(|_| FlashError::Read { block })?;
        Ok(())
    }

    fn program(&mut self, block: u32, offset: usize, data: &[u8]) -> FlashResult<()> {
        self.check(block, offset, data.len())?;
        if data.is_empty() {
            return Ok(());
        }
        let first = offset / self.page_size;
        let last = (offset + data.len() - 1) / self.page_size;
        if first != last {
            return Err(FlashError::Unaligned);
        }
        if let Some(limit) = self.fail_after {
            if self.programs >= limit {
                return Err(FlashError::Program { block });
            }
        }
        let at = self.base(block) + offset as u64;
        let mut cur = vec![0u8; data.len()];
        self.file
            .seek(SeekFrom::Start(at))
            .map_err(|_| FlashError::Program { block })?;
        self.file
            .read_exact(&mut cur)
            .map_err(|_| FlashError::Program { block })?;
        for (i, &byte) in data.iter().enumerate() {
            if cur[i] & byte != byte {
                return Err(FlashError::Program { block });
            }
        }
        self.file
            .seek(SeekFrom::Start(at))
            .map_err(|_| FlashError::Program { block })?;
        self.file
            .write_all(data)
            .map_err(|_| FlashError::Program { block })?;
        self.programs += 1;
        Ok(())
    }

    fn erase(&mut self, block: u32) -> FlashResult<()> {
        if block >= self.block_count {
            return Err(FlashError::OutOfRange);
        }
        let at = self.base(block);
        let zeros = vec![0xFFu8; self.erase_block_size];
        self.file
            .seek(SeekFrom::Start(at))
            .map_err(|_| FlashError::Erase { block })?;
        self.file
            .write_all(&zeros)
            .map_err(|_| FlashError::Erase { block })?;
        Ok(())
    }

    fn sync(&mut self) -> FlashResult<()> {
        self.file.sync_all().map_err(|_| FlashError::Sync)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use embedded_database_flash::ftl::{Ftl, FtlConfig};

    #[test]
    fn 文件中重建_ftl_后数据仍在() {
        let dir = std::env::temp_dir().join("embedded_database_flash_file_test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("dev0.bin");
        let _ = std::fs::remove_file(&path);
        let raw = FileRawFlash::open(&path, 64, 256, 4096).unwrap();
        let mut ftl = Ftl::new(raw, FtlConfig::default()).unwrap();
        let data = vec![0x5Au8; 3840];
        ftl.write_at(0, &data).unwrap();
        ftl.sync().unwrap();
        drop(ftl);
        let raw = FileRawFlash::open(&path, 64, 256, 4096).unwrap();
        let mut ftl = Ftl::new(raw, FtlConfig::default()).unwrap();
        let mut got = vec![0u8; 3840];
        ftl.read_at(0, &mut got).unwrap();
        assert_eq!(got, data);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn 编程失败注入后可重挂() {
        let dir = std::env::temp_dir().join("embedded_database_flash_fail_test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("dev1.bin");
        let _ = std::fs::remove_file(&path);
        let mut raw = FileRawFlash::open(&path, 64, 256, 4096).unwrap();
        raw.fail_after(2);
        let mut ftl = Ftl::new(raw, FtlConfig::default()).unwrap();
        // 第一次写必然触发搬迁/失败路径，但不得 panic；之后重挂仍可用。
        let _ = ftl.write_at(0, &vec![0x11u8; 3840]);
        drop(ftl);
        let raw = FileRawFlash::open(&path, 64, 256, 4096).unwrap();
        let mut ftl = Ftl::new(raw, FtlConfig::default()).unwrap();
        let mut got = vec![0u8; 16];
        assert!(ftl.read_at(0, &mut got).is_ok());
        let _ = std::fs::remove_file(&path);
    }
}
```

在 `crates/embedded-database-os/src/lib.rs` 加 `pub mod flash;` 与 `pub use flash::FileRawFlash;`。

- [ ] **Step 3: 运行**

Run: `cargo test -p embedded-database-os`
Expected: 2 新增测试 passed（连同原有 5 个 = 7 passed）。

- [ ] **Step 4: 门禁 + 提交**

Run GATE。
```bash
git add crates/embedded-database-os
git commit -m "feat: embedded-database-os 提供文件支撑的 FileRawFlash"
```

---

## Task 8: 文档如实化 + 账本 + 终验

**Files:**
- Modify: `README.md`
- Modify: `docs/superpowers/specs/2026-10-02-embedded-database-design.md`（仅文末追加）
- Modify: `docs/superpowers/specs/2026-10-19-embedded-database-flash-backend-design.md`（仅文末追加）
- Modify: `.superpowers/sdd/progress.md`

- [ ] **Step 1: 加「Db 建立在 Ftl 之上」的集成测试（A1 的核心价值）**

**几何对齐要求**：FTL 的 `discard` 只能回收被**完整覆盖**的逻辑块（3840B），所以把 `Db` 建在
`Ftl` 上时 `Config::segment_size` 必须是 `logical_block_size = 3840` 的整数倍。本仓库的
`Config::default()` 用 1 MiB（不是 3840 的倍数），因此必须显式配置。设计建议值
`segment_size = 3_932_160 = 3840 × 1024 = 960 × 4096`（同时是逻辑块与擦除块的整数倍）。

在 `crates/embedded-database-flash/src/ftl.rs` 的 `mod tests` 里追加（`embedded-database-core` 在 `[dev-dependencies]`
里已带 `std` 特性，可直接用 `Db`）：

```rust
    #[test]
    fn core_db_建立在_ftl_之上可写满多段并重开() {
        use embedded_database_core::{Config, Db};

        const SEG: usize = 3840 * 1024; // 3_932_160，逻辑块(3840)的整数倍
        let max_segments = 4u64;
        let logical = max_segments * SEG as u64;
        let logical_blocks = logical / 3840;
        // FTL 默认 OP 20% ⇒ 物理块数 = 逻辑块数 × 5/4。
        let phys = (logical_blocks * 5 / 4) as u32;

        let raw = SimFlash::new(phys, PAGE, BLOCK);
        let mut ftl = Ftl::new(raw, FtlConfig::default()).unwrap();
        assert_eq!(ftl.logical_size(), logical);

        let cfg = Config {
            segment_size: SEG,
            max_segments: max_segments as usize,
            ..Config::default()
        };

        {
            let mut db = Db::open(&mut ftl, cfg.clone()).unwrap();
            for i in 0..60u64 {
                db.put(format!("k{i}").as_bytes(), b"payload").unwrap();
                db.flush().unwrap();
            }
        }

        // 重开：段环 + 合并后所有键仍在。
        let db = Db::open(&mut ftl, cfg).unwrap();
        for i in 0..60u64 {
            assert_eq!(
                db.get(format!("k{i}").as_bytes()).unwrap(),
                Some(b"payload".to_vec()),
                "键 k{i} 丢失"
            );
        }
    }
```

Run: `cargo test -p embedded-database-flash core_db_建立在_ftl_之上可写满多段并重开`
Expected: passed。

- [ ] **Step 2: 更新 README 状态与构建片段**

把 `README.md` 里 crate 列表/行数/测试数改为实测值（跑完 GATE 后以 `cargo test` 输出为准），并增加 `embedded-database-flash` 一行简介。

- [ ] **Step 3: 设计文档追加「实施状态与勘误」**

在 `2026-10-02-embedded-database-design.md` 文末追加一节，至少含：
- §5.3.4「建议 4MB」的勘误：实测选定 `segment_size = 3_932_160`（3840 × 1024）；理由（1024 逻辑块 = 960 擦除块，页/块两向对齐）。
- 原「闪存驱动」占位行（原 `:147`、`:769`）标注已由 Spec 3b 落地为 `embedded-database-flash` + `embedded-database-os::FileRawFlash`。
- 在 `2026-10-19-embedded-database-flash-backend-design.md` 文末追加：**勘误**——"坏块用副本恢复"不成立（本设计无冗余副本）；坏块只保证"分配时避开 + 编程/擦除失败即退役"，已提交数据块若介质失效则数据丢失，由 core 的段 CRC/提交链检出为损坏。

- [ ] **Step 4: 账本记账**

`.superpowers/sdd/progress.md` 追加 Spec 3b 的提交序列与门禁结果，并核销 M4、M11（写明核销理由与提交号）。

- [ ] **Step 5: 终验（A1–A8）**

Run:
```bash
export PATH="/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH"
cargo fmt --all --check
cargo clippy --workspace --all-targets --features embedded-database-core/std,embedded-database-embed/std -- -D warnings
cargo test --workspace --features embedded-database-core/std,embedded-database-embed/std
cargo test -p embedded-database-embed
cargo check -p embedded-database-flash --target loongarch64-unknown-none
cargo doc -p embedded-database-flash --no-deps
```
Expected: 全绿；`cargo doc` 0 警告。逐条核对 A1–A8（见设计文档 §7），把 8 条验收结果写进任务报告。

- [ ] **Step 6: 提交**

```bash
git add README.md docs .superpowers/sdd/progress.md crates/embedded-database-flash
git commit -m "docs: Spec 3b 实施状态与勘误如实化"
```

---

## 验收核对表（终验后逐条打勾）

- [ ] A1 正确性：内存（`SimFlash`）与文件（`FileRawFlash`）双后端读写/重挂一致。
- [ ] A2 掉电矩阵：6 个断电点（数据页编程中 / 块头编程前后 / 擦除中 / discard 之间 / core flush 截断 / core 自动合并截断）后重挂不 panic 且不返回脏数据。
- [ ] A3 磨损：≥500 次擦除后 `erase_count_max - erase_count_min <= 2`。
- [ ] A4 写放大：顺序写 ≤ 1.5；含静态搬移 ≤ 3.0。
- [ ] A5 坏块：注入/标记坏块后数据仍可读（坏块被跳过）。
- [ ] A6 门禁全绿，`cargo test -p embedded-database-flash`（无特性）通过，embed 不回归，CI nostd 列表含 `embedded-database-flash`。
- [ ] A7 零新增第三方依赖。
- [ ] A8 每逻辑单元一提交（`git log --oneline` 逐条可对应本计划任务）。
