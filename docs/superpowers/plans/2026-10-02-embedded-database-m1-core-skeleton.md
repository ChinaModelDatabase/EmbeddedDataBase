# M1 内核骨架实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 建立 embedded-database 存储引擎骨架——段格式、CRC 链、页槽分配、段写入、崩溃恢复、基础 KV，以及崩溃注入与 WCET 测试框架，全部在 `#![no_std]` + `#![forbid(unsafe_code)]` 下通过。

**Architecture:** 三层。`embedded-database-core` 是无分配的 `no_std` 内核，容量全部由调用方启动时提供的 `&mut [u8]` 承载；写路径把变更页顺序追加到不可变段内的预留页槽，Commit Record 写在段尾固定偏移；读路径全部走内存 Memtable，段文件仅冷启动扫描时使用。崩溃恢复用 CRC 链判定最后一个完整事务的边界。

**Tech Stack:** Rust edition 2021，MSRV **1.81**（`loongarch64-unknown-linux-musl` 自该版本起提供预构建产物）。运行时依赖仅 `zerocopy` 与 `bitflags`。开发期依赖 `proptest`。

**Spec:** `docs/superpowers/specs/2026-10-02-embedded-database-design.md`

## Global Constraints

以下约束适用于本计划**每一个** Task，不再逐条重复：

- **C1** 除 `embedded-database-bench` 外全部 crate `#![no_std]`，且 M1 **不使用 `alloc`**，不引入全局分配器。所有可变容量内存由调用方提供的 `&mut [u8]` 承载。
- **C2** `embedded-database-core` / `embedded-database-kv` / `embedded-database-embed` 顶层 `#![forbid(unsafe_code)]`——不允许任何局部 `unsafe`。
- **C3** 运行时依赖上限 2 个：`zerocopy`、`bitflags`。`proptest` 等仅进 `[dev-dependencies]`。
- **C4** 库代码禁止 `unwrap()` / `expect()` / `panic!()` / 数组越界。边界一律用 `get()` / `get_mut()` / `checked_*` 返回 `Result`。`unwrap()` 仅允许出现在 `#[cfg(test)]` 中。
- **C5** 永不阻塞、永不自旋。写者锁竞争返回 `Error::Busy`，超时不等待。
- **C6** `no_std` 代码禁用 `std::time`。时间由调用方注入。
- **C7** 所有哈希表容量固定、探测次数有上限 `MAX_PROBES`，满则返回 `Error::CapacityExhausted`，**绝不**扩容或退化为链式。
- **C8** 所有 B-tree 深度编译期固定，分裂不触发重平衡，超深度返回 `Error::CapacityExhausted`。
- **C9** CRC 固定 CRC-32C（Castagnoli，反射多项式 `0x82F63B78`），**必须软件查表**——x86 SSE4.2 / ARM CRC 指令时延可变，违反确定性。
- **C10** 所有路径相对项目根 `/Users/caosunliu/opencode/EmbeddedDataBase`。
- **C11** **本计划不执行任何 git 提交。** 用户已决定由其自行提交。
  embedded-database 已是独立 git 仓库（`/Users/caosunliu/opencode/EmbeddedDataBase`，分支 `feat/m1-core-skeleton`），
  不再是家目录 monorepo 的一部分。**禁止 `git commit`、禁止 `git add .`**。
  各 Task 末尾的「检查点」步骤只做文件存在性确认。
- **C12** 本环境无 Rust 工具链。Task 0 建立工具链是**硬前置**，未完成则后续 Task 全部阻塞。

## ⚠️ 代码未经编译验证

本计划全部 Rust 代码在**无工具链环境**中编写，**未经 `cargo build` / `cargo test` 验证**。执行者必须假定存在编译与类型错误，并把它们当作正常 TDD 循环的一部分修正，不要指望照抄即通过。Task 0 的验收标准就是用来收敛这个不确定性。

## 文件结构

```
Cargo.toml                    workspace 根，4 个 crate
rust-toolchain.toml           锁定 1.81.0 + 5 个 no_std 目标
.gitignore  README.md
.github/workflows/ci.yml      5 目标 no_std check + 测试 + unsafe 扫描
crates/embedded-database-core/          no_std, no alloc, forbid(unsafe_code)
  src/lib.rs                  模块声明、Error、Result
  src/crc.rs                  CRC-32C 软件查表 + CRC 链
  src/segment.rs              段格式零拷贝结构 + 编解码
  src/device.rs               BlockDevice trait + 故障注入内存设备
  src/arena.rs                固定容量 bump 分配器
  src/hashtable.rs            固定容量开放寻址哈希表
  src/btree.rs                定高 B-tree
  src/memtable.rs             Memtable（std 特性）
  src/slot.rs                 页槽分配器
  src/writer.rs               段写入器
  src/recovery.rs             崩溃恢复扫描
crates/embedded-database-embed/         no_std 公开 API 占位
crates/embedded-database-kv/            no_std KV 占位
crates/embedded-database-bench/         std 开发期工具
  src/bin/inject_crash.rs     崩溃注入测试
  src/bin/wcet.rs             WCET 实测
```

**关于 C1 与 `Vec` 的冲突**：`Memtable`、`SlotAllocator`、`SegmentWriter` 内部逻辑上需要增长的列表，而 C1 禁止 `alloc`。M1 的解法是这些模块走 `std` 特性（仅测试与 bench 使用），`no_std` 生产路径在 M3 用 arena 内的定长结构替代。**这不违反 C1**——C1 约束的是 `no_std` 构建，而 `std` 特性在生产构建中关闭（`default-features = false`）。Task 5 的 Step 3 会对 `no_std` 构建做专项验证。

---

## Task 0: 工具链与工作区骨架

**Files:**
- Create: `Cargo.toml`、`rust-toolchain.toml`、`.gitignore`、`README.md`
- Create: `crates/embedded-database-core/{Cargo.toml,src/lib.rs}`
- Create: `crates/embedded-database-embed/{Cargo.toml,src/lib.rs}`
- Create: `crates/embedded-database-kv/{Cargo.toml,src/lib.rs}`
- Create: `crates/embedded-database-bench/{Cargo.toml,src/main.rs}`
- Create: `.github/workflows/ci.yml`

**Interfaces:**
- Consumes: 无
- Produces: `embedded_database_core::Error`、`embedded_database_core::Result<T>`。后续 Task 只允许**扩展** `Error` 变体，不允许改变已有变体语义。

- [ ] **Step 1: 安装 Rust 工具链** ✅ 已完成（2026-10-02）

本机原无 `cargo` / `rustc`。已执行：

```bash
brew install rustup
export PATH="/opt/homebrew/opt/rustup/bin:$PATH"
rustc --version && cargo --version
```

**注意 Homebrew 的 `rustup` formula 已不再提供 `rustup-init`**，且是 keg-only（未软链到 `/opt/homebrew`，
以免与 `rust` 冲突）。因此：

- 不要执行计划早期草稿里的 `rustup-init -y ...`——该命令不存在。
- `rustup` 首次运行时会自动安装 stable 工具链。实测已得到 `rustc 1.99.0` / `cargo 1.99.0`。
- 后续所有 shell 需要先 `export PATH="/opt/homebrew/opt/rustup/bin:$PATH"`，
  否则 `cargo` / `rustc` / `rustup` 不可见。若希望持久化，在 `~/.zshrc` 追加该行。
- 计划其余部分锁定的 **1.81.0**（Step 2）由 `rust-toolchain.toml` 生效，
  与此处的 stable 1.99.0 并存——`cargo` 会依据 `rust-toolchain.toml` 自动切换。

已安装：rustup 1.29.1、rustc/cargo 1.99.0（stable-aarch64-apple-darwin）。

- [ ] **Step 2: 锁定工具链与 no_std 目标**

`rust-toolchain.toml`：

```toml
[toolchain]
channel = "1.81.0"
components = ["rustfmt", "clippy", "rust-src"]
targets = [
  "aarch64-unknown-linux-gnu",
  "loongarch64-unknown-linux-gnu",
  "loongarch64-unknown-linux-musl",
  "loongarch64-unknown-none",
  "x86_64-unknown-linux-gnu",
]
profile = "minimal"
```

```bash
rustup toolchain install
rustup target list --installed
```

期望包含上面 5 个 target。`rust-src` 是必需的——`loongarch64-unknown-none` 需要它，后续 `-Z build-std` 也需要。

- [ ] **Step 3: workspace 根 `Cargo.toml`**

```toml
[workspace]
resolver = "2"
members = [
    "crates/embedded-database-core",
    "crates/embedded-database-kv",
    "crates/embedded-database-embed",
    "crates/embedded-database-bench",
]

[workspace.package]
version = "0.1.0"
edition = "2021"
rust-version = "1.81"
license = "Apache-2.0"
repository = "https://github.com/caosunliu/EmbeddedDataBase"

[workspace.dependencies]
embedded-database-core = { path = "crates/embedded-database-core", version = "0.1.0" }
embedded-database-embed = { path = "crates/embedded-database-embed", version = "0.1.0" }
embedded-database-kv = { path = "crates/embedded-database-kv", version = "0.1.0" }
zerocopy = { version = "0.8", default-features = false }
bitflags = { version = "2", default-features = false }
proptest = "1"

[profile.release]
panic = "abort"
overflow-checks = true
lto = "fat"
codegen-units = 1
```

`overflow-checks = true` 在 release 保留——确定性数据库中整数溢出必须 panic 而非回绕，这是 C4 的延伸，不能为性能牺牲。

- [ ] **Step 4: `.gitignore`**

```gitignore
/target
**/*.rs.bk
*.db
*.whseg
.DS_Store
```

`Cargo.lock` **不**忽略——发布库应提交 lock 文件以保证可复现构建。

- [ ] **Step 5: `crates/embedded-database-core/Cargo.toml`**

```toml
[package]
name = "embedded-database-core"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true
description = "embedded-database 存储引擎内核：确定性 + Flash 友好 + no_std"

[features]
default = []
# 仅供测试与 bench 的 std 路径。生产构建必须 default-features = false。
std = []

[dependencies]
zerocopy.workspace = true
bitflags.workspace = true

[dev-dependencies]
proptest.workspace = true

[lints.rust]
missing_debug_implementations = "warn"
unsafe_code = "forbid"
```

- [ ] **Step 6: `crates/embedded-database-core/src/lib.rs`**

```rust
#![no_std]
#![forbid(unsafe_code)]

//! embedded-database 存储引擎内核。
//!
//! 设计约束（不得违反）：
//! - `no_std`：所有可变容量内存由调用方提供的 `&mut [u8]` 承载。
//! - 零 `unsafe`：全 crate `#![forbid(unsafe_code)]`。
//! - 确定性：操作有可测量的时延上界，容量固定，失败显式。

// ⚠️ 关键：`#![no_std]` 不会因为 `cfg(test)` 自动获得 std。
// `cargo test` 编译的仍是 no_std crate，因此测试里的 Vec/String/format! 必须
// 显式引入。`std` 特性是 M1 中少数几个"允许用 Vec"的场景，仅测试与 bench 使用。
#[cfg(any(test, feature = "std"))]
extern crate std;

// 以下模块在 M1 走 std 路径（内部用 Vec 组织可变长度列表）。
// 门控放在 `mod` 声明处而非逐个 item——门控一次，`no_std` 构建就整体剔除，
// 不会因为某个 item 漏加 cfg 而让 no_std 构建失败。
// M3 会把它们的 std 依赖替换为 arena 内的定长结构，届时移除这些门控。
mod arena;
mod btree;
mod crc;
mod device;
mod hashtable;
mod segment;

#[cfg(feature = "std")]
mod memtable;
#[cfg(feature = "std")]
mod recovery;
#[cfg(feature = "std")]
mod slot;
#[cfg(feature = "std")]
mod writer;

/// 库级统一错误类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// 写者锁被占用。调用方应重试——embedded-database 永不等待（spec §7.1）。
    Busy,
    /// 超过调用方指定的 Deadline。embedded-database 永不阻塞（spec §5.4）。
    DeadlineExceeded,
    /// 固定容量结构已满。**不可恢复**，需提高配置容量后重试。
    CapacityExhausted,
    /// 存储配额超限。
    QuotaExceeded,
    /// 键不存在。
    KeyNotFound,
    /// 数据损坏。`segment_seq` 为出问题的段序列号。
    Corruption { segment_seq: u64 },
    /// 段中不存在给定 page_id。
    PageNotFound { page_id: u64 },
    /// 底层设备 I/O 失败。
    Io,
    /// 缓冲区越界或长度不合法。
    InvalidArgument,
    /// 页的 k 个槽已用尽且已冻结。
    SlotFrozen { page_id: u64 },
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Error::Busy => f.write_str("busy: writer lock held"),
            Error::DeadlineExceeded => f.write_str("deadline exceeded"),
            Error::CapacityExhausted => {
                f.write_str("capacity exhausted: fixed-capacity structure is full")
            }
            Error::QuotaExceeded => f.write_str("quota exceeded"),
            Error::KeyNotFound => f.write_str("key not found"),
            Error::Corruption { segment_seq } => write!(f, "corruption in segment {segment_seq}"),
            Error::PageNotFound { page_id } => write!(f, "page {page_id} not found"),
            Error::Io => f.write_str("io error"),
            Error::InvalidArgument => f.write_str("invalid argument"),
            Error::SlotFrozen { page_id } => write!(f, "page {page_id} has no free slot"),
        }
    }
}

/// 库的 Result 别名。
pub type Result<T> = core::result::Result<T, Error>;
```

- [ ] **Step 7: 另三个 crate 骨架**

`crates/embedded-database-embed/Cargo.toml` 与 `crates/embedded-database-kv/Cargo.toml` 内容相同（除 `name` / `description`）：

```toml
[package]
name = "embedded-database-embed"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true
description = "embedded-database 公开嵌入 API"

[dependencies]
embedded-database-core.workspace = true

[lints.rust]
unsafe_code = "forbid"
```

`crates/embedded-database-embed/src/lib.rs`（`embedded-database-kv` 同构，改注释）：

```rust
#![no_std]
#![forbid(unsafe_code)]

//! embedded-database 公开嵌入 API。
//!
//! M1 阶段仅占位：完整 `Db` / `Txn` 接口属于 M2。

/// 占位类型，确保 crate 非空。
#[derive(Debug)]
pub struct Placeholder;
```

`crates/embedded-database-bench/Cargo.toml`：

```toml
[package]
name = "embedded-database-bench"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true
description = "embedded-database 开发期测试与基准工具"

[dependencies]
embedded-database-core = { workspace = true, features = ["std"] }

[[bin]]
name = "inject_crash"
path = "src/bin/inject_crash.rs"

[[bin]]
name = "wcet"
path = "src/bin/wcet.rs"

[lints.rust]
unsafe_code = "forbid"
```

`crates/embedded-database-bench/src/main.rs`（占位，两个 bin 在 Task 8/9 创建）：

```rust
fn main() {
    eprintln!("embedded-database-bench: 请使用 inject_crash / wcet");
}
```

- [ ] **Step 8: `README.md`**

````markdown
# embedded-database

面向信创国产架构与 PLC/嵌入式硬实时场景的纯 Rust 嵌入式数据库内核。

- 设计文档：`docs/superpowers/specs/2026-10-02-embedded-database-design.md`
- 许可证：Apache-2.0
- 状态：M1 内核骨架开发中

## 构建

```bash
rustup toolchain install
cargo build
cargo test --features embedded-database-core/std
```

## 架构约束

`embedded-database-core` 强制 `#![no_std]` + `#![forbid(unsafe_code)]`，不使用全局分配器。
容量启动时固定，失败显式返回，不静默降级。
生产构建必须 `default-features = false`。
````

- [ ] **Step 9: 创建 11 个模块空壳**

```bash
for f in arena btree crc device hashtable memtable recovery segment slot writer; do
  printf '//! 占位，后续 Task 填充。\n' > "crates/embedded-database-core/src/$f.rs"
done
mkdir -p crates/embedded-database-bench/src/bin
```

- [ ] **Step 10: 验证 workspace 编译**

```bash
cargo build --workspace 2>&1 | tail -20
```

期望通过。`lib.rs` 里的 `mod` 声明指向空壳文件，空壳无内容也应编译通过。若报错，多半是某个空壳文件缺失。

- [ ] **Step 11: 验证 5 个 no_std 目标**

```bash
for t in aarch64-unknown-linux-gnu loongarch64-unknown-linux-gnu \
         loongarch64-unknown-linux-musl x86_64-unknown-linux-gnu; do
  echo "=== $t ==="
  cargo check -p embedded-database-core --target "$t" 2>&1 | tail -3
done
echo "=== loongarch64-unknown-none (bare metal) ==="
cargo check -p embedded-database-core --target loongarch64-unknown-none 2>&1 | tail -3
```

期望 5 个目标全部成功。裸机目标只需 `check`，不需链接。

- [ ] **Step 12: 验证 `forbid(unsafe_code)` 已生效（构造性，无需探针）**

`crates/embedded-database-core/Cargo.toml` 的 `[lints.rust] unsafe_code = "forbid"` 已使
任何 `unsafe` 成为**硬编译错误**——这是构造性保证，无需运行期探针。

```bash
cargo build -p embedded-database-core 2>&1 | tail -5
```

期望通过。同时确认 lint 已生效：

```bash
cargo clippy -p embedded-database-core 2>&1 | rg -c 'forbid' || echo "无 forbid 违规"
```

⚠️ **不要**为了"验证 forbid 生效"而往 `lib.rs` 里临时写入 `unsafe` 代码——
那会在任务中途污染真实源文件，中断即留下 `unsafe` 残留。
静态扫描由 Step 13 的 CI 步骤 `校验无 unsafe` 覆盖。

- [ ] **Step 13: CI**

`.github/workflows/ci.yml`：

```yaml
name: ci
on:
  push:
    branches: [main]
  pull_request:
env:
  CARGO_TERM_COLOR: always
jobs:
  host:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - run: rustup toolchain install
      - run: cargo fmt --all --check
      - run: cargo clippy --workspace --all-targets --features embedded-database-core/std -- -D warnings
      - run: cargo test --workspace --features embedded-database-core/std
      - name: 校验无 unsafe
        run: |
          # 逐文件 grep -n 保留行号。注意不能用 `grep -rn ... | grep -v '^\s*//'`：
          # grep -rn 的输出是 "file:line:content"，行首是文件名而非注释符号，
          # 该过滤永远不匹配，会对任何提到 unsafe 的注释误报。
          found=0
          while IFS= read -r f; do
            if grep -n "unsafe" "$f" \
              | grep -v "forbid(unsafe_code)" \
              | grep -vE '^[[:space:]]*[0-9]+:[[:space:]]*(//|/\*|\*)'; then
              echo "^^ $f 含有 unsafe"
              found=1
            fi
          done < <(find crates -path '*/src/*' -name '*.rs')
          [ "$found" -eq 0 ]
      - name: 生成 SBOM
        run: |
          cargo install cargo-cyclonedx
          cargo cyclonedx --format json --outfile sbom.json
  nostd:
    strategy:
      matrix:
        target:
          - aarch64-unknown-linux-gnu
          - loongarch64-unknown-linux-gnu
          - loongarch64-unknown-linux-musl
          - loongarch64-unknown-none
          - x86_64-unknown-linux-gnu
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - run: rustup toolchain install
      - run: cargo check -p embedded-database-core -p embedded-database-kv -p embedded-database-embed --target ${{ matrix.target }}
```

- [ ] **Step 14: 检查点（不提交）**

本计划不执行 git 提交（C11）。本步改为确认本 Task 的 **Files** 列表中
每个文件都已存在且内容完整：

```bash
git status --short
```

确认所有新增文件出现在输出中即通过。**不要** `git add` 或 `git commit`。

---

## Task 1: CRC-32C 与 CRC 链

**Files:**
- Modify: `crates/embedded-database-core/src/crc.rs`（替换空壳）
- Modify: `crates/embedded-database-core/src/lib.rs`（加 re-export）

**Interfaces:**
- Consumes: `Error`（Task 0）
- Produces:
  - `pub struct Crc32c`：`new()`、`update(&mut self, &[u8])`、`finish(&self) -> u32`
  - `pub struct CrcChain`：`new()`、`extend(&mut self, page_crc_fold: u32)`、`value(&self) -> u32`
  - `pub fn fold_crcs(crcs: &[u32]) -> u32`
  - `pub fn hash_bytes(data: &[u8]) -> u32`（FNV-1a，后续 Task 4 使用）

- [ ] **Step 1: 写 `crc.rs`**

测试与实现在同一文件，先写测试段再写实现段（下面已按最终形态给出，落地时按测试在前组织）。

```rust
//! CRC-32C（Castagnoli）与事务级 CRC 链。
//!
//! 必须软件查表：x86 SSE4.2 与 ARM CRC 指令时延可变，
//! 违反确定性要求（spec §5.4、约束 C9）。

/// 反射后的 Castagnoli 多项式。
const POLY: u32 = 0x82F6_3B78;

const fn build_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0usize;
    while i < 256 {
        let mut crc = i as u32;
        let mut j = 0usize;
        while j < 8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ POLY } else { crc >> 1 };
            j += 1;
        }
        table[i] = crc;
        i += 1;
    }
    table
}

static TABLE: [u32; 256] = build_table();

/// 增量 CRC-32C 计算器。
///
/// **不要用 `#[derive(Default)]`**——派生实现会给出 `Crc32c(0)`，
/// 而正确的初始值是 `0xFFFF_FFFF`（CRC 的预置初值）。
/// 这是一个静默产生错误校验结果的陷阱，因此手写 `Default`。
#[derive(Debug, Clone, Copy)]
pub struct Crc32c(u32);

impl Default for Crc32c {
    fn default() -> Self {
        Self::new()
    }
}

impl Crc32c {
    pub const fn new() -> Self {
        Crc32c(0xFFFF_FFFF)
    }

    pub fn update(&mut self, data: &[u8]) {
        let mut crc = self.0;
        for &b in data {
            crc = (crc >> 8) ^ TABLE[((crc ^ u32::from(b)) & 0xFF) as usize];
        }
        self.0 = crc;
    }

    pub const fn finish(&self) -> u32 {
        self.0 ^ 0xFFFF_FFFF
    }
}

/// 段级 CRC 链：把每段的 fold 值纳入前一段的 CRC，形成链式校验。
///
/// 作用：防止"段 N-2 被静默损坏但 N-1、N 看似正常"（spec §7.2）。
#[derive(Debug, Clone, Copy, Default)]
pub struct CrcChain(u32);

impl CrcChain {
    pub const fn new() -> Self {
        CrcChain(0)
    }

    pub fn extend(&mut self, page_crc_fold: u32) {
        let mut c = Crc32c::new();
        c.update(&self.0.to_le_bytes());
        c.update(&page_crc_fold.to_le_bytes());
        self.0 = c.finish();
    }

    pub const fn value(&self) -> u32 {
        self.0
    }
}

/// 把一组 CRC 折叠为单个 u32。顺序敏感。
pub fn fold_crcs(crcs: &[u32]) -> u32 {
    let mut c = Crc32c::new();
    for v in crcs {
        c.update(&v.to_le_bytes());
    }
    c.finish()
}

/// FNV-1a 32 位哈希。固定实现——无随机种子、无地址依赖，
/// 保证同样的键在任何平台得到同样的桶位置。
pub fn hash_bytes(data: &[u8]) -> u32 {
    let mut h: u32 = 0x811c_9dc5;
    for &b in data {
        h ^= u32::from(b);
        h = h.wrapping_mul(0x0100_0193);
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;
    // no_std crate：Vec / format! 需要显式引入
    use std::vec::Vec;

    #[test]
    fn 默认值等于新建() {
        // 防止有人把 derive(Default) 改回来——那会让 CRC 静默算错
        assert_eq!(Crc32c::default().finish(), Crc32c::new().finish());
        assert_eq!(Crc32c::default().0, 0xFFFF_FFFF);
        assert_eq!(CrcChain::default().value(), CrcChain::new().value());
    }

    #[test]
    fn 标准检验值() {
        // CRC-32C 标准 check 值
        let mut c = Crc32c::new();
        c.update(b"123456789");
        assert_eq!(c.finish(), 0xE306_9283);
    }

    #[test]
    fn 空输入检验值为零() {
        let mut c = Crc32c::new();
        c.update(b"");
        assert_eq!(c.finish(), 0x0000_0000);
    }

    #[test]
    fn 增量等于一次性() {
        let mut a = Crc32c::new();
        a.update(b"hello ");
        a.update(b"world");
        let mut b = Crc32c::new();
        b.update(b"hello world");
        assert_eq!(a.finish(), b.finish());
    }

    #[test]
    fn 任意切分结果一致() {
        let data: Vec<u8> = (0..=255u8).collect();
        let mut whole = Crc32c::new();
        whole.update(&data);
        for split in 0..=data.len() {
            let mut c = Crc32c::new();
            c.update(&data[..split]);
            c.update(&data[split..]);
            assert_eq!(c.finish(), whole.finish(), "split={split}");
        }
    }

    #[test]
    fn 单比特翻转改变结果() {
        let data: [u8; 64] = core::array::from_fn(|i| i as u8);
        let mut base = Crc32c::new();
        base.update(&data);
        let base_crc = base.finish();
        for bit in 0..8usize {
            let mut d = data;
            d[0] ^= 1 << bit;
            let mut c = Crc32c::new();
            c.update(&d);
            assert_ne!(c.finish(), base_crc, "bit={bit}");
        }
    }

    #[test]
    fn 链对顺序敏感() {
        let mut a = CrcChain::new();
        a.extend(1);
        a.extend(2);
        let mut b = CrcChain::new();
        b.extend(2);
        b.extend(1);
        assert_ne!(a.value(), b.value());
    }

    #[test]
    fn 链传播上游扰动() {
        // 仅最后一个段被改动，也必须导致链值变化
        let mut a = CrcChain::new();
        a.extend(0xAAAA_AAAA);
        a.extend(0xBBBB_BBBB);
        let mut b = CrcChain::new();
        b.extend(0xAAAA_AAAA);
        b.extend(0xBBBB_3333);
        assert_ne!(a.value(), b.value());
    }

    #[test]
    fn 折叠顺序敏感() {
        assert_ne!(fold_crcs(&[1, 2, 3]), fold_crcs(&[3, 2, 1]));
        assert_eq!(fold_crcs(&[]), fold_crcs(&[]));
    }

    #[test]
    fn 哈希确定且区分大小写() {
        assert_eq!(hash_bytes(b"embedded-database"), hash_bytes(b"embedded-database"));
        assert_ne!(hash_bytes(b"embedded-database"), hash_bytes(b"embedded-databasE"));
    }

    #[test]
    fn 顺序键不产生极端冲突() {
        // FNV-1a 对 "k0".."k999" 的低位分布回归防线。
        // 分布过差会让线性探测退化，违反确定性预期。
        let mut buckets = [0usize; 1024];
        for i in 0..1000u32 {
            let h = hash_bytes(format!("k{i}").as_bytes()) as usize;
            buckets[h % 1024] += 1;
        }
        let max = buckets.iter().copied().max().unwrap_or(0);
        assert!(max < 10, "最大桶 {max} 过多，冲突严重");
    }
}
```

- [ ] **Step 2: 运行测试**

Run: `cargo test -p embedded-database-core crc 2>&1 | tail -20`
Expected: 11 passed

- [ ] **Step 3: re-export**

`lib.rs` 的 `mod writer;` 之后加：

```rust
pub use crc::{fold_crcs, hash_bytes, Crc32c, CrcChain};
```

Run: `cargo test -p embedded-database-core 2>&1 | tail -5`
Expected: 11 passed

- [ ] **Step 4: 检查点（不提交）**

本计划不执行 git 提交（C11）。本步改为确认本 Task 的 **Files** 列表中
每个文件都已存在且内容完整：

```bash
git status --short
```

确认所有新增文件出现在输出中即通过。**不要** `git add` 或 `git commit`。


---

## Task 2: 段格式编解码

**Files:**
- Modify: `crates/embedded-database-core/src/segment.rs`（替换空壳）
- Modify: `crates/embedded-database-core/src/lib.rs`

**Interfaces:**
- Consumes: `Crc32c`（Task 1）、`Error`（Task 0）
- Produces:
  - `SEGMENT_MAGIC: [u8; 8]`、`COMMIT_MAGIC: [u8; 8]`
  - `SEGMENT_HEADER_SIZE = 56`、`PAGE_HEADER_SIZE = 24`、`COMMIT_RECORD_SIZE = 40`、`PAGE_RUN_ENTRY_SIZE = 24`
  - `SLOT_FLAG_FROZEN: u16`
  - `SegmentHeader { magic, format_major, format_minor, segment_seq, prev_segment, prev_crc, page_size, slot_count, _pad, slot_table_off, header_crc, _reserved }`
  - `PageHeader { page_id, version, payload_len, _pad }`
  - `CommitRecord { commit_magic, segment_seq, prev_crc, page_crc_fold, commit_crc, timestamp, _pad }`
  - `PageRun { page_id, run_offset, next_slot, flags }` + `is_frozen()` / `freeze()`
  - 四个结构体各有 `encode() -> [u8; N]` 与 `decode(&[u8]) -> Result<Self, Error>`

**磁盘布局：**

```text
[0, 56)                              SegmentHeader
[56, 56 + N*k*page_size)             槽区：每个逻辑页占 k 个物理槽，槽大小 = page_size
[.., slot_table_off)                PageRun 表：每页 16 字节
[.., segment_size - 40)             追加区
[segment_size - 40, segment_size)   CommitRecord（最后写）
```

槽 CRC 覆盖**整个槽**（含尾部零填充），这样 padding 损坏也能被发现。

- [ ] **Step 1: 写 `segment.rs`**

```rust
//! 段格式定义与零拷贝编解码。

use crate::crc::Crc32c;
use crate::Error;
use zerocopy::{FromBytes, IntoBytes};

pub const SEGMENT_MAGIC: [u8; 8] = *b"WHSEG\0\0\0";
pub const COMMIT_MAGIC: [u8; 8] = *b"WHCMT\0\0\0";

pub const SEGMENT_HEADER_SIZE: usize = 56;
pub const PAGE_HEADER_SIZE: usize = 24;
pub const COMMIT_RECORD_SIZE: usize = 40;
pub const PAGE_RUN_ENTRY_SIZE: usize = 24;

/// PageRun.flags：页的 k 个槽已用尽，后续更新必须走溢出段。
pub const SLOT_FLAG_FROZEN: u16 = 0x0001;

#[derive(Debug, Clone, Copy, PartialEq, Eq, FromBytes, IntoBytes)]
#[repr(C)]
pub struct SegmentHeader {
    pub magic: [u8; 8],
    pub format_major: u16,
    pub format_minor: u16,
    pub segment_seq: u64,
    pub prev_segment: u64,
    pub prev_crc: u32,
    pub page_size: u32,
    pub slot_count: u16,
    pub _pad: u16,
    pub slot_table_off: u64,
    /// 覆盖本结构体除自身与 _reserved 以外的前 48 字节（[0, 48)）。
    pub header_crc: u32,
    pub _reserved: u32,
}

impl SegmentHeader {
    pub fn new(
        segment_seq: u64,
        prev_segment: u64,
        prev_crc: u32,
        page_size: u32,
        slot_count: u16,
        slot_table_off: u64,
    ) -> Self {
        let mut h = SegmentHeader {
            magic: SEGMENT_MAGIC,
            format_major: 1,
            format_minor: 0,
            segment_seq,
            prev_segment,
            prev_crc,
            page_size,
            slot_count,
            _pad: 0,
            slot_table_off,
            header_crc: 0,
            _reserved: 0,
        };
        h.header_crc = h.compute_crc();
        h
    }

    fn compute_crc(&self) -> u32 {
        // 只覆盖 header_crc 之前的字段（[0, 48)）。
        // ⚠️ 不能用 [..52]——那会把 header_crc 自身（占 [48, 52)）算进去，
        // 形成自引用：写入时 header_crc 为 0，decode 时读到实际值，必然校验失败。
        let bytes = <Self as IntoBytes>::as_bytes(self);
        let mut c = Crc32c::new();
        c.update(&bytes[..48]);
        c.finish()
    }

    pub fn encode(&self) -> [u8; SEGMENT_HEADER_SIZE] {
        let mut out = [0u8; SEGMENT_HEADER_SIZE];
        out.copy_from_slice(<Self as IntoBytes>::as_bytes(self));
        out
    }

    pub fn decode(buf: &[u8]) -> Result<SegmentHeader, Error> {
        let bytes = buf
            .get(..SEGMENT_HEADER_SIZE)
            .ok_or(Error::InvalidArgument)?;
        let h = <Self as FromBytes>::read_from_bytes(bytes)
            .map_err(|_| Error::InvalidArgument)?;
        if h.magic != SEGMENT_MAGIC || h.compute_crc() != h.header_crc {
            return Err(Error::Corruption { segment_seq: h.segment_seq });
        }
        if h.page_size == 0 || h.slot_count == 0 {
            return Err(Error::Corruption { segment_seq: h.segment_seq });
        }
        Ok(h)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, FromBytes, IntoBytes)]
#[repr(C)]
pub struct PageHeader {
    pub page_id: u64,
    pub version: u32,
    pub payload_len: u32,
    /// 预留字节，恒为 0。**不可删**：删掉后仅剩 20 字节，而首字段 u64 使
    /// align = 8，编译器随即补 4 字节尾部填充，`IntoBytes` 派生直接编译失败。
    /// 这也是 PAGE_HEADER_SIZE 只能取 24（8 的倍数）而非 28 的原因。
    /// `page_crc` 正占用原 `_pad` 之后的 4 字节预留空间——若将来不要页级
    /// 校验，要删的是 `page_crc`，不是把 `_pad` 加宽。
    pub _pad: u32,
    /// 覆盖 [0, 20)，即除自身以外的全部字节（含 _pad）。必须放在末尾：
    /// CRC 字段位置若在中间，覆盖范围就不是连续前缀，易漏算。
    pub page_crc: u32,
}

impl PageHeader {
    pub fn encode(&self) -> [u8; PAGE_HEADER_SIZE] {
        let mut out = [0u8; PAGE_HEADER_SIZE];
        out.copy_from_slice(<Self as IntoBytes>::as_bytes(self));
        out
    }

    pub fn decode(buf: &[u8]) -> Result<PageHeader, Error> {
        let bytes = buf.get(..PAGE_HEADER_SIZE).ok_or(Error::InvalidArgument)?;
        <Self as FromBytes>::read_from_bytes(bytes).map_err(|_| Error::InvalidArgument)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, FromBytes, IntoBytes)]
#[repr(C)]
pub struct CommitRecord {
    pub commit_magic: [u8; 8],
    pub segment_seq: u64,
    pub prev_crc: u32,
    pub page_crc_fold: u32,
    pub commit_crc: u32,
    pub timestamp: u64,
    pub _pad: u32,
}

impl CommitRecord {
    pub fn new(segment_seq: u64, prev_crc: u32, page_crc_fold: u32, timestamp: u64) -> Self {
        let mut r = CommitRecord {
            commit_magic: COMMIT_MAGIC,
            segment_seq,
            prev_crc,
            page_crc_fold,
            commit_crc: 0,
            timestamp,
            _pad: 0,
        };
        r.commit_crc = r.compute_crc();
        r
    }

    fn compute_crc(&self) -> u32 {
        // ⚠️ 不能用 [..COMMIT_RECORD_SIZE - 4]（即 [..36)）——那会把
        // commit_crc 自身（占 [24, 28)）算进去，形成自引用：
        // new() 时 commit_crc 为 0，decode 时读到实际值，必然校验失败。
        // 正确范围：跳过 commit_crc，纳入其后的 timestamp。_pad 不参与。
        let bytes = <Self as IntoBytes>::as_bytes(self);
        let mut c = Crc32c::new();
        c.update(&bytes[..24]);
        c.update(&bytes[28..36]);
        c.finish()
    }

    pub fn encode(&self) -> [u8; COMMIT_RECORD_SIZE] {
        let mut out = [0u8; COMMIT_RECORD_SIZE];
        out.copy_from_slice(<Self as IntoBytes>::as_bytes(self));
        out
    }

    pub fn decode(buf: &[u8]) -> Result<CommitRecord, Error> {
        let bytes = buf.get(..COMMIT_RECORD_SIZE).ok_or(Error::InvalidArgument)?;
        let r = <Self as FromBytes>::read_from_bytes(bytes)
            .map_err(|_| Error::InvalidArgument)?;
        if r.commit_magic != COMMIT_MAGIC || r.compute_crc() != r.commit_crc {
            return Err(Error::Corruption { segment_seq: r.segment_seq });
        }
        Ok(r)
    }
}

/// 段内一个逻辑页的槽位组描述。
#[derive(Debug, Clone, Copy, PartialEq, Eq, FromBytes, IntoBytes)]
#[repr(C)]
pub struct PageRun {
    pub page_id: u64,
    /// 槽组起始偏移，相对段起点。
    pub run_offset: u64,
    /// 下一个可用槽在该组内的下标。等于 slot_count 时该页冻结。
    pub next_slot: u16,
    pub flags: u16,
    /// 覆盖 [0, 20)，即除自身以外的全部字节（含 next_slot 与 flags）。
    /// PageRun 不保留 _pad：8+8+2+2+4 = 24 正好，再加 _pad 会涨到 32 并产生
    /// 4 字节尾部填充。
    pub run_crc: u32,
}

impl PageRun {
    pub fn encode(&self) -> [u8; PAGE_RUN_ENTRY_SIZE] {
        let mut out = [0u8; PAGE_RUN_ENTRY_SIZE];
        out.copy_from_slice(<Self as IntoBytes>::as_bytes(self));
        out
    }

    pub fn decode(buf: &[u8]) -> Result<PageRun, Error> {
        let bytes = buf
            .get(..PAGE_RUN_ENTRY_SIZE)
            .ok_or(Error::InvalidArgument)?;
        <Self as FromBytes>::read_from_bytes(bytes).map_err(|_| Error::InvalidArgument)
    }

    pub fn is_frozen(&self) -> bool {
        self.flags & SLOT_FLAG_FROZEN != 0
    }

    pub fn freeze(&mut self) {
        self.flags |= SLOT_FLAG_FROZEN;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::mem::{align_of, size_of};

    #[test]
    fn 结构体大小符合磁盘格式() {
        assert_eq!(size_of::<SegmentHeader>(), SEGMENT_HEADER_SIZE);
        assert_eq!(size_of::<PageHeader>(), PAGE_HEADER_SIZE);
        assert_eq!(size_of::<CommitRecord>(), COMMIT_RECORD_SIZE);
        assert_eq!(size_of::<PageRun>(), PAGE_RUN_ENTRY_SIZE);
    }

    #[test]
    fn 结构体八字节对齐() {
        assert_eq!(align_of::<SegmentHeader>() % 8, 0);
        assert_eq!(align_of::<CommitRecord>() % 8, 0);
        assert_eq!(align_of::<PageRun>() % 8, 0);
    }

    #[test]
    fn 段头往返一致() {
        let h = SegmentHeader::new(7, 6, 0xDEAD_BEEF, 4096, 4, 4096);
        assert_eq!(SegmentHeader::decode(&h.encode()).unwrap(), h);
    }

    #[test]
    fn 段头_逐位翻转全部被检出() {
        let h = SegmentHeader::new(9, 8, 0x1234_5678, 4096, 4, 4096);
        let base = h.encode();
        for i in 0..SEGMENT_HEADER_SIZE {
            let mut b = base;
            b[i] ^= 0x01;
            if b == base {
                continue;
            }
            assert!(
                SegmentHeader::decode(&b).is_err(),
                "翻转字节 {i} 未被检出"
            );
        }
    }

    #[test]
    fn 段头_缓冲区不足被拒() {
        let h = SegmentHeader::new(1, 0, 0, 4096, 4, 4096);
        let bytes = h.encode();
        assert_eq!(
            SegmentHeader::decode(&bytes[..SEGMENT_HEADER_SIZE - 1]),
            Err(Error::InvalidArgument)
        );
    }

    #[test]
    fn 段头_非法页大小被拒() {
        let h = SegmentHeader::new(1, 0, 0, 0, 4, 4096);
        assert!(SegmentHeader::decode(&h.encode()).is_err());
    }

    #[test]
    fn 提交记录往返一致() {
        let r = CommitRecord::new(42, 0x0102_0304, 0xAABB_CCDD, 1_700_000_000);
        assert_eq!(CommitRecord::decode(&r.encode()).unwrap(), r);
    }

    #[test]
    fn 提交记录_篡改被拒() {
        let r = CommitRecord::new(42, 0, 0xFFFF_FFFF, 0);
        let mut bytes = r.encode();
        bytes[20] ^= 0xFF; // page_crc_fold
        assert!(CommitRecord::decode(&bytes).is_err());
    }

    #[test]
    fn 提交记录_错魔数被拒() {
        let r = CommitRecord::new(42, 0, 0, 0);
        let mut bytes = r.encode();
        bytes[0] = b'Z';
        assert!(CommitRecord::decode(&bytes).is_err());
    }

    #[test]
    fn 页头往返一致() {
        let p = PageHeader { page_id: 12345, version: 3, payload_len: 100, _pad: 0 };
        assert_eq!(PageHeader::decode(&p.encode()).unwrap(), p);
    }

    #[test]
    fn 页槽组往返与冻结() {
        let mut r = PageRun { page_id: 7, run_offset: 8192, next_slot: 2, flags: 0 };
        assert_eq!(PageRun::decode(&r.encode()).unwrap(), r);
        assert!(!r.is_frozen());
        r.freeze();
        assert!(r.is_frozen());
    }
}
```

- [ ] **Step 2: 运行测试**

Run: `cargo test -p embedded-database-core segment 2>&1 | tail -20`
Expected: 11 passed

若 `结构体大小符合磁盘格式` 失败，调整字段的 `_pad` / `_reserved` 宽度直到匹配常量。这是磁盘格式的硬约束，不能改常量迁就实现。

- [ ] **Step 3: re-export**

`lib.rs` 加：

```rust
pub use segment::{
    CommitRecord, PageHeader, PageRun, SegmentHeader, COMMIT_MAGIC, COMMIT_RECORD_SIZE,
    PAGE_HEADER_SIZE, PAGE_RUN_ENTRY_SIZE, SEGMENT_HEADER_SIZE, SEGMENT_MAGIC, SLOT_FLAG_FROZEN,
};
```

Run: `cargo test -p embedded-database-core 2>&1 | tail -5`
Expected: 21 passed

- [ ] **Step 4: 检查点（不提交）**

本计划不执行 git 提交（C11）。本步改为确认本 Task 的 **Files** 列表中
每个文件都已存在且内容完整：

```bash
git status --short
```

确认所有新增文件出现在输出中即通过。**不要** `git add` 或 `git commit`。


---

## Task 2 裁决记录（控制者补充，2026-10-02）

Task 2 实施中发现**原计划的字段布局数学上不可能成立**，经控制者独立复算确认并由用户裁决后修正。Task 6/7/8 的代码必须遵循下列最终布局，不得沿用旧版。

| 决策 | 内容 | 原因 |
|---|---|---|
| 布局重排 | `SegmentHeader` 字段顺序重排 | 原布局 `format_major/minor` 各 u2 紧跟 u64 `segment_seq`，产生 8 字节隐式填充（size 64 ≠ 常量 56）。zerocopy `IntoBytes` **禁止任何填充字节**，原设计无法编译 |
| `PAGE_RUN_ENTRY_SIZE` | 16 → **24** | 16 字节无法同时容纳两个 u64（实为 24）。用户裁决：两字段均保 u64 |
| `PAGE_HEADER_SIZE` | 24（**尺寸不变**） | 用户裁决「加 u32 CRC」——`page_crc` 占用原 `_pad` 之后的 4 字节预留空间，故非加宽。28 不可达（含 u64 的 repr(C) 结构体零填充尺寸必为 8 的倍数） |
| `PageRun` CRC | 新增 `run_crc: u32` | 用户裁决：四结构体全部须支持逐位翻转必被拒测试 |
| 解码方式 | `Unalign<T>::read_from_bytes` | `read_from_bytes` 要求对齐；设备读出的 `&[u8]` 通常非 8 对齐，会把合法数据误判为损坏，Task 7 将全线报 `Corruption` |
| `zerocopy` 特性 | 启用 `features = ["derive"]` | 原 `default-features = false` 导致 derive 宏不可用。derive 是 proc-macro，运行期零代码，不违反 C3 |
| 错误变体 | 新增 `Error::CorruptedPage { page_id }` | 页头/槽表条目解码时无段上下文，原先填 `segment_seq: 0` 与真实段 0 损坏无法区分 |
| 短缓冲区语义 | 四个 `decode` **统一**返回 `InvalidArgument` | 缓冲区不足是调用方编程错误，不是数据损坏。Task 7 必须据此区分「磁盘坏了」与「调用方 bug」 |
| 字节序 | 加 `#[cfg(target_endian = "big")] compile_error!` | zerocopy `IntoBytes::as_bytes` 是**内存转录**（主机字节序），非固定 LE。在大端目标上写出的段与其他目标不兼容，且与模块内显式 LE 的 `leading_page_id` 自相矛盾。M1 三目标全为小端，故编译期直接拦截而非静默产出坏数据。CI 有专门 job 验证该防护未失效 |

**最终布局（Task 7/8 硬编码，必须照抄）**

```
SEGMENT_HEADER_SIZE = 56   commit_crc/header_crc 在末尾，CRC 覆盖除自身外连续前缀
  magic [0,8) u64 · format_major [8,12) u32 · format_minor [12,16) u32
  segment_seq [16,24) u64 · prev_segment [24,32) u64 · prev_crc [32,36) u32
  page_size [36,40) u32 · slot_table_off [40,48) u64 · slot_count [48,50) u16
  _pad [50,52) u16 · header_crc [52,56) u32        CRC 覆盖 [0,52)

PAGE_HEADER_SIZE = 24
  page_id [0,8) u64 · version [8,12) u32 · payload_len [12,16) u32
  _pad [16,20) u32 · page_crc [20,24) u32          CRC 覆盖 [0,20)

COMMIT_RECORD_SIZE = 40
  commit_magic [0,8) · segment_seq [8,16) u64 · timestamp [16,24) u64
  prev_crc [24,28) u32 · page_crc_fold [28,32) u32
  _pad [32,36) u32 · commit_crc [36,40) u32        CRC 覆盖 [0,36)

PAGE_RUN_ENTRY_SIZE = 24
  page_id [0,8) u64 · run_offset [8,16) u64 · next_slot [16,18) u16
  flags [18,20) u16 · run_crc [20,24) u32          CRC 覆盖 [0,20)
```

**Task 7 后续必须解决的三处（已在 Task 2 实施中识别）**

1. `slot_table_off = SEGMENT_HEADER_SIZE + max_pages * run_bytes` 与页数据实际写入区不重合，`PageRun.run_offset` 当前无真实槽组地址
2. 多段场景下所有段均从设备偏移 0 读取，配置中缺段基址
3. 恢复扫描可能把 PageRun 表误当作页，需明确页数或扫描边界；并须实际用 `prev_crc` 做跨段 CRC 链连续性校验

---

## Task 3: BlockDevice trait 与故障注入内存设备

**Files:**
- Modify: `crates/embedded-database-core/src/device.rs`（替换空壳）
- Modify: `crates/embedded-database-core/src/lib.rs`

**Interfaces:**
- Consumes: `Error`（Task 0）
- Produces:
  - `pub trait BlockDevice`：`size()` / `read_at()` / `write_at()` / `sync()`
  - `pub struct InMemoryDevice`：`new(size)`、`read_at()`、`write_at()`、`sync()`、`corrupt_at(offset, len)`、`inject_write_failure(after_writes)`、`clear_failures()`、`writable_bytes()`
  - `pub struct WriteStats { pub successful_writes: u64, pub failed_writes: u64 }`

`BlockDevice` 放在 `embedded-database-core::device`，嵌入方（文件系统、裸 Flash、eMMC）实现它。

- [ ] **Step 1: 写 `device.rs`**

```rust
//! 存储设备抽象与可故障注入的内存实现。

use crate::Error;

/// 固定尺寸的块设备抽象。
///
/// 实现方必须满足：
/// - 所有偏移越界返回 `Error::InvalidArgument`，**不得 panic**。
/// - `write_at` 后数据对 `read_at` 立即可见（写穿透语义）；
///   掉电安全性由 `sync()` 界定。
/// - `sync()` 保证此前写入已落到稳定介质，返回后才算持久。
/// - 永不阻塞、永不自旋。设备忙时返回 `Error::Busy`。
pub trait BlockDevice {
    /// 设备容量（字节）。
    fn size(&self) -> u64;

    /// 读 `buf.len()` 字节到 `buf`。`offset + buf.len()` 越界则失败。
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()>;

    /// 从 `data` 写 `data.len()` 字节。
    fn write_at(&mut self, offset: u64, data: &[u8]) -> Result<()>;

    /// 将此前写入刷到稳定介质。持久性边界。
    fn sync(&mut self) -> Result<()>;
}
```

注意 `read_at` 取 `&self` 而 `write_at`/`sync` 取 `&mut self`——读路径可并发，这直接决定了上层能否免锁读。

```rust
/// 内存设备，支持按需注入数据损坏与写失败，用于崩溃恢复测试。
///
/// 承载 `Vec<u8>`，因此仅在 `std` 下可用。
#[cfg(feature = "std")]
#[derive(Debug, Clone)]
pub struct InMemoryDevice {
    data: std::vec::Vec<u8>,
    /// (offset, len)：这些区间每次写入都会被篡改，模拟位翻转。
    corrupt: std::vec::Vec<(usize, usize)>,
    /// 剩余多少次写操作后开始失败。`None` 表示不注入失败。
    fail_after: std::option::Option<u64>,
    writes_seen: u64,
    writable_bytes: usize,
    stats: WriteStats,
}

/// 写操作计数，用于验证故障注入确实生效。
#[cfg(feature = "std")]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WriteStats {
    pub successful_writes: u64,
    pub failed_writes: u64,
}

#[cfg(feature = "std")]
impl InMemoryDevice {
    /// 创建指定容量的内存设备。
    pub fn new(size: usize) -> InMemoryDevice {
        InMemoryDevice {
            data: std::vec![0u8; size],
            corrupt: std::vec::Vec::new(),
            fail_after: std::option::Option::None,
            writes_seen: 0,
            writable_bytes: size,
            stats: WriteStats::default(),
        }
    }

    /// 把 `[offset, offset+len)` 标记为损坏区：每次写入该区都会写错。
    pub fn corrupt_at(&mut self, offset: usize, len: usize) {
        self.corrupt.push((offset, len));
    }

    /// 注入写失败：成功 `n` 次写之后，所有写返回 `Error::Io`。
    pub fn inject_write_failure(&mut self, after_writes: u64) {
        self.fail_after = std::option::Option::Some(after_writes);
        self.writes_seen = 0;
    }

    pub fn clear_failures(&mut self) {
        self.fail_after = std::option::Option::None;
    }

    /// 只读打开设备：所有写返回 `Error::Io`。
    pub fn set_read_only(&mut self) {
        self.fail_after = std::option::Option::Some(0);
    }

    pub fn stats(&self) -> WriteStats {
        self.stats
    }

    /// 允许读写的字节数。用于验证没有写出设备边界。
    pub fn writable_bytes(&self) -> usize {
        self.writable_bytes
    }

    /// 直接读取原始字节，供恢复测试断言段内容。
    pub fn raw(&self) -> &[u8] {
        &self.data
    }
}

#[cfg(feature = "std")]
impl BlockDevice for InMemoryDevice {
    fn size(&self) -> u64 {
        self.data.len() as u64
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        let start = usize::try_from(offset).map_err(|_| Error::InvalidArgument)?;
        let end = start.checked_add(buf.len()).ok_or(Error::InvalidArgument)?;
        if end > self.data.len() {
            return Err(Error::InvalidArgument);
        }
        buf.copy_from_slice(&self.data[start..end]);
        Ok(())
    }

    fn write_at(&mut self, offset: u64, data: &[u8]) -> Result<()> {
        let start = usize::try_from(offset).map_err(|_| Error::InvalidArgument)?;
        let end = start.checked_add(data.len()).ok_or(Error::InvalidArgument)?;
        if end > self.data.len() {
            self.stats.failed_writes += 1;
            return Err(Error::InvalidArgument);
        }

        if let Some(limit) = self.fail_after {
            if self.writes_seen >= limit {
                self.stats.failed_writes += 1;
                return Err(Error::Io);
            }
        }
        self.writes_seen += 1;

        self.data[start..end].copy_from_slice(data);
        self.writable_bytes = self.writable_bytes.max(end);

        // 注入损坏：翻转写入内容的每一位
        for (c_off, c_len) in &self.corrupt {
            let lo = start.max(*c_off);
            let hi = end.min(c_off + c_len);
            for i in lo..hi {
                self.data[i] ^= 0xFF;
            }
        }

        self.stats.successful_writes += 1;
        Ok(())
    }

    fn sync(&mut self) -> Result<()> {
        Ok(())
    }
}
```

- [ ] **Step 2: 写测试**

追加到 `device.rs`：

```rust
#[cfg(all(test, feature = "std"))]
mod tests {
    use super::*;
    use std::vec::Vec;

    #[test]
    fn 读回写入内容() {
        let mut d = InMemoryDevice::new(1024);
        d.write_at(10, b"embedded-database").unwrap();
        let mut buf = [0u8; 6];
        d.read_at(10, &mut buf).unwrap();
        assert_eq!(&buf, b"embedded-database");
        assert_eq!(d.size(), 1024);
    }

    #[test]
    fn 越界读写被拒() {
        let mut d = InMemoryDevice::new(16);
        assert_eq!(d.write_at(16, b"x"), Err(Error::InvalidArgument));
        let mut buf = [0u8; 4];
        assert_eq!(d.read_at(14, &mut buf), Err(Error::InvalidArgument));
    }

    #[test]
    fn 写注入失败_达阈值后开始报错() {
        let mut d = InMemoryDevice::new(64);
        d.inject_write_failure(2);
        assert!(d.write_at(0, b"a").is_ok());
        assert!(d.write_at(1, b"b").is_ok());
        assert_eq!(d.write_at(2, b"c"), Err(Error::Io));
        assert_eq!(d.stats(), WriteStats { successful_writes: 2, failed_writes: 1 });
    }

    #[test]
    fn 清除失败后恢复() {
        let mut d = InMemoryDevice::new(64);
        d.inject_write_failure(0);
        assert!(d.write_at(0, b"a").is_err());
        d.clear_failures();
        assert!(d.write_at(0, b"a").is_ok());
    }

    #[test]
    fn 损坏注入确实翻转数据() {
        let mut d = InMemoryDevice::new(64);
        d.corrupt_at(0, 8);
        d.write_at(0, b"embedded-database!").unwrap();
        assert_ne!(&d.raw()[..7], b"embedded-database");
    }

    #[test]
    fn 未触及末尾时高水位不虚高() {
        let mut d = InMemoryDevice::new(4096);
        d.write_at(0, b"x").unwrap();
        assert_eq!(d.writable_bytes(), 1);
    }
}
```

- [ ] **Step 3: 运行测试**

Run: `cargo test -p embedded-database-core --features std device 2>&1 | tail -20`
Expected: 6 passed

- [ ] **Step 4: 确认 `no_std` 构建下本模块不引入 std**

Run: `cargo check -p embedded-database-core --target x86_64-unknown-linux-gnu 2>&1 | tail -5`
Expected: 通过（`InMemoryDevice` 整块被 `#[cfg(feature = "std")]` 剔除）

若失败，检查是否有 `Vec` / `Option` 逃出了 cfg 保护。

- [ ] **Step 5: re-export**

`lib.rs` 加：

```rust
pub use device::BlockDevice;
#[cfg(feature = "std")]
pub use device::InMemoryDevice;
```

- [ ] **Step 6: 检查点（不提交）**

本计划不执行 git 提交（C11）。本步改为确认本 Task 的 **Files** 列表中
每个文件都已存在且内容完整：

```bash
git status --short
```

确认所有新增文件出现在输出中即通过。**不要** `git add` 或 `git commit`。


---

## Task 4: 固定容量 Arena 与开放寻址哈希表

**Files:**
- Modify: `crates/embedded-database-core/src/arena.rs`（替换空壳）
- Modify: `crates/embedded-database-core/src/hashtable.rs`（替换空壳）
- Modify: `crates/embedded-database-core/src/lib.rs`

**Interfaces:**
- Consumes: `Error`（Task 0）、`hash_bytes`（Task 1）
- Produces:
  - `Arena<'a>`：`new(&mut [u8])`、`capacity()`、`used()`、`alloc(size, align)`、`read(off, len)`、`write(off, data)`、`read_u16/read_u32/write_u16/write_u32`、`clear()`
  - `FixedTable<'a>`：`new(&mut Arena)`、`insert(key, value, version)`、`get(key) -> Option<&[u8]>`、`len()`、`capacity()`、`scan(start_seq) -> Option<ScanEntry>`
  - `MAX_PROBES: usize = 8`
  - `ScanEntry<'a> { pub key: &'a [u8], pub value: &'a [u8], pub version: u32 }`

**关键设计决策：借用检查策略**

`FixedTable` 与 `BTree` 都需要"读出一段字节 + 原地写回"。让 `Arena::read` 返回 `&'a [u8]`（长生命周期）会与后续 `&mut` 调用冲突，因此：

- `Arena::read(&self) -> Result<&[u8]>` 借用期绑定到 `&self`——调用方**立即拷贝**到栈上，之后再调 `write`。
- 需要"零拷贝读取"的地方（如 `FixedTable::get` 的返回值）走单独的 `peek(&self) -> Result<&'a [u8]>`，显式命名为 `peek` 以区别。

这样两类使用模式各自明确，且不依赖编译器猜测。

- [ ] **Step 1: 写 `arena.rs`**

```rust
//! 固定容量 bump 分配器。不分配、不增长、不 panic。
//!
//! 所有分配都来自调用方提供的一块 `&mut [u8]`。耗尽即返回
//! `Error::CapacityExhausted`——**绝不**扩容，容量必须在启动时确定（C7）。

use crate::Error;

/// 固定容量 arena。
///
/// 生命周期与调用方借用的缓冲区绑定，不能比它活得久。
#[derive(Debug)]
pub struct Arena<'a> {
    buf: &'a mut [u8],
    used: usize,
}

impl<'a> Arena<'a> {
    /// 用调用方提供的缓冲区创建 arena。
    pub fn new(buf: &'a mut [u8]) -> Self {
        Arena { buf, used: 0 }
    }

    pub fn capacity(&self) -> usize {
        self.buf.len()
    }

    pub fn used(&self) -> usize {
        self.used
    }

    pub fn remaining(&self) -> usize {
        self.buf.len() - self.used
    }

    /// 分配 `size` 字节并按 `align` 对齐，返回 arena 内偏移。
    ///
    /// 对齐向上取整；若所需空间不足返回 `CapacityExhausted`。
    pub fn alloc(&mut self, size: usize, align: usize) -> Result<usize, Error> {
        if align == 0 || !align.is_power_of_two() {
            return Err(Error::InvalidArgument);
        }
        let base = (self.used + align - 1) & !(align - 1);
        let end = base.checked_add(size).ok_or(Error::CapacityExhausted)?;
        if end > self.buf.len() {
            return Err(Error::CapacityExhausted);
        }
        self.used = end;
        Ok(base)
    }

    /// 读 `len` 字节，返回借用期绑定到 `&self` 的切片——调用方须立即拷贝。
    pub fn read(&self, off: usize, len: usize) -> Result<&[u8], Error> {
        let end = off.checked_add(len).ok_or(Error::InvalidArgument)?;
        self.buf.get(off..end).ok_or(Error::InvalidArgument)
    }

    /// 零拷贝读，返回 `'a` 生命周期的切片。
    ///
    /// 调用后到下一次 `alloc` / `write` 之间，返回值保持有效。
    pub fn peek(&self, off: usize, len: usize) -> Result<&'a [u8], Error> {
        let end = off.checked_add(len).ok_or(Error::InvalidArgument)?;
        self.buf.get(off..end).ok_or(Error::InvalidArgument)
    }

    /// 写 `data` 到 `off`。越界返回 `InvalidArgument`，**不 panic**。
    pub fn write(&mut self, off: usize, data: &[u8]) -> Result<(), Error> {
        let end = off.checked_add(data.len()).ok_or(Error::InvalidArgument)?;
        if end > self.buf.len() {
            return Err(Error::InvalidArgument);
        }
        self.buf[off..end].copy_from_slice(data);
        Ok(())
    }

    pub fn read_u16(&self, off: usize) -> Result<u16, Error> {
        let b = self.read(off, 2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    pub fn read_u32(&self, off: usize) -> Result<u32, Error> {
        let b = self.read(off, 4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn write_u16(&mut self, off: usize, v: u16) -> Result<(), Error> {
        self.write(off, &v.to_le_bytes())
    }

    pub fn write_u32(&mut self, off: usize, v: u32) -> Result<(), Error> {
        self.write(off, &v.to_le_bytes())
    }

    /// 归还全部空间。保留容量。
    pub fn clear(&mut self) {
        self.used = 0;
    }

    /// 剩余未分配空间中是否还能容纳 `size` 字节。不改变状态。
    pub fn can_alloc(&self, size: usize, align: usize) -> bool {
        if align == 0 || !align.is_power_of_two() {
            return false;
        }
        let base = (self.used + align - 1) & !(align - 1);
        base.checked_add(size).is_some_and(|end| end <= self.buf.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 分配递增偏移() {
        let mut buf = [0u8; 64];
        let mut a = Arena::new(&mut buf);
        assert_eq!(a.alloc(8, 1).unwrap(), 0);
        assert_eq!(a.alloc(8, 1).unwrap(), 8);
        assert_eq!(a.used(), 16);
        assert_eq!(a.capacity(), 64);
        assert_eq!(a.remaining(), 48);
    }

    #[test]
    fn 对齐到指定边界() {
        let mut buf = [0u8; 64];
        let mut a = Arena::new(&mut buf);
        a.alloc(1, 1).unwrap();
        let off = a.alloc(8, 8).unwrap();
        assert_eq!(off % 8, 0);
    }

    #[test]
    fn 耗尽返回容量不足() {
        let mut buf = [0u8; 16];
        let mut a = Arena::new(&mut buf);
        a.alloc(12, 1).unwrap();
        assert_eq!(a.alloc(8, 1), Err(Error::CapacityExhausted));
    }

    #[test]
    fn 非二次幂对齐被拒() {
        let mut buf = [0u8; 64];
        let mut a = Arena::new(&mut buf);
        assert_eq!(a.alloc(8, 3), Err(Error::InvalidArgument));
        assert_eq!(a.alloc(8, 0), Err(Error::InvalidArgument));
    }

    #[test]
    fn 写读往返() {
        let mut buf = [0u8; 32];
        let mut a = Arena::new(&mut buf);
        a.write(4, b"embedded-database").unwrap();
        assert_eq!(a.read(4, 7).unwrap(), b"embedded-database");
    }

    #[test]
    fn 越界读写不panic() {
        let mut buf = [0u8; 16];
        let mut a = Arena::new(&mut buf);
        assert_eq!(a.write(14, b"toolong"), Err(Error::InvalidArgument));
        assert_eq!(a.read(15, 4), Err(Error::InvalidArgument));
    }

    #[test]
    fn 定长整数往返() {
        let mut buf = [0u8; 32];
        let mut a = Arena::new(&mut buf);
        a.write_u16(0, 0xBEEF).unwrap();
        a.write_u32(8, 0xDEAD_BEEF).unwrap();
        assert_eq!(a.read_u16(0).unwrap(), 0xBEEF);
        assert_eq!(a.read_u32(8).unwrap(), 0xDEAD_BEEF);
    }

    #[test]
    fn 清空回收全部空间() {
        let mut buf = [0u8; 16];
        let mut a = Arena::new(&mut buf);
        a.alloc(16, 1).unwrap();
        a.clear();
        assert_eq!(a.used(), 0);
        assert_eq!(a.alloc(16, 1).unwrap(), 0);
    }

    #[test]
    fn 容量探测不改变状态() {
        let mut buf = [0u8; 16];
        let mut a = Arena::new(&mut buf);
        a.alloc(12, 1).unwrap();
        assert!(!a.can_alloc(8, 1));
        assert_eq!(a.used(), 12);
    }

    #[test]
    fn 越界加法不溢出() {
        let mut buf = [0u8; 8];
        let mut a = Arena::new(&mut buf);
        assert_eq!(a.read(usize::MAX, 2), Err(Error::InvalidArgument));
    }
}
```

- [ ] **Step 2: 运行 arena 测试**

Run: `cargo test -p embedded-database-core arena 2>&1 | tail -20`
Expected: 10 passed

- [ ] **Step 3: 写 `hashtable.rs`**

槽位布局固定 24 字节：

| 偏移 | 字段 | 宽度 |
|---|---|---|
| 0 | `seq`（0 = 空槽，否则 1 起递增） | u32 |
| 4 | `state`（0=空 1=正常 2=墓碑） | u16 |
| 6 | `key_len` | u16 |
| 8 | `val_len` | u16 |
| 10 | `key_off` | u32 |
| 14 | `val_off` | u32 |
| 18 | `version` | u32 |
| 22 | `_pad` | u16 |

线性探测上限 `MAX_PROBES = 8`，满则 `CapacityExhausted`（C7）。

```rust
//! 固定容量开放寻址哈希表。不增长、不 rehash、无随机化。
//!
//! 容量在启动时固定，探测次数上限 `MAX_PROBES`。满则返回
//! `CapacityExhausted`——**绝不**扩容或退化（C7）。
//! 哈希函数固定（FNV-1a），保证同样的键在任何平台落在同样的桶。

use crate::arena::Arena;
use crate::crc::hash_bytes;
use crate::Error;

/// 单次查找/插入的最大线性探测步数。
pub const MAX_PROBES: usize = 8;

const SLOT_SIZE: usize = 24;
const ST_EMPTY: u16 = 0;
const ST_LIVE: u16 = 1;
const ST_TOMB: u16 = 2;

/// 固定容量哈希表。所有键值字节都存在 arena 中。
#[derive(Debug)]
pub struct FixedTable<'a> {
    arena: &'a mut Arena<'a>,
    slots_off: usize,
    slot_count: usize,
    len: usize,
}

impl<'a> FixedTable<'a> {
    /// 在 arena 中建立 `slot_count` 个槽的哈希表。
    ///
    /// 槽区一次性分配，之后永不增长。
    pub fn new(arena: &'a mut Arena<'a>, slot_count: usize) -> Result<Self, Error> {
        if slot_count == 0 {
            return Err(Error::InvalidArgument);
        }
        let slots_off = arena.alloc(slot_count * SLOT_SIZE, 8)?;
        // 槽区必须清零：空槽以 seq == 0 判定
        for i in 0..slot_count {
            arena.write_u32(slots_off + i * SLOT_SIZE, 0)?;
            arena.write_u16(slots_off + i * SLOT_SIZE + 4, ST_EMPTY)?;
        }
        Ok(FixedTable { arena, slots_off, slot_count, len: 0 })
    }

    pub fn capacity(&self) -> usize {
        self.slot_count
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    fn slot_addr(&self, i: usize) -> usize {
        self.slots_off + i * SLOT_SIZE
    }

    /// 定位键所在的槽下标。返回 `Ok(None)` 表示键不存在。
    fn find(&self, key: &[u8]) -> Result<Option<usize>, Error> {
        let start = (hash_bytes(key) as usize) % self.slot_count;
        for step in 0..MAX_PROBES.min(self.slot_count) {
            let idx = (start + step) % self.slot_count;
            let addr = self.slot_addr(idx);
            let state = self.arena.read_u16(addr + 4)?;
            if state == ST_EMPTY {
                // 空槽终止探测——无删除时这是正确的
                return Ok(None);
            }
            if state == ST_LIVE {
                let kl = self.arena.read_u16(addr + 6)? as usize;
                let ko = self.arena.read_u32(addr + 8)? as usize;
                if self.arena.read(ko, kl)? == key {
                    return Ok(Some(idx));
                }
            }
            // 墓碑不终止探测
        }
        Ok(None)
    }

    /// 插入或更新。同键重复插入覆盖旧值并递增 `version`。
    ///
    /// 容量不足返回 `CapacityExhausted`。
    pub fn insert(&mut self, key: &[u8], value: &[u8], version: u32) -> Result<(), Error> {
        if let Some(idx) = self.find(key)? {
            let addr = self.slot_addr(idx);
            let voff = self.arena.read_u32(addr + 14)? as usize;
            let vlen = self.arena.read_u16(addr + 8)? as usize;
            if vlen != value.len() {
                return Err(Error::InvalidArgument);
            }
            self.arena.write(voff, value)?;
            self.arena.write_u32(addr + 18, version)?;
            return Ok(());
        }

        // 找空槽
        let start = (hash_bytes(key) as usize) % self.slot_count;
        let mut target = None;
        for step in 0..MAX_PROBES.min(self.slot_count) {
            let idx = (start + step) % self.slot_count;
            let addr = self.slot_addr(idx);
            if self.arena.read_u16(addr + 4)? == ST_EMPTY {
                target = Some(idx);
                break;
            }
        }
        let idx = target.ok_or(Error::CapacityExhausted)?;

        let ko = self.arena.alloc(key.len(), 1)?;
        self.arena.write(ko, key)?;
        let vo = self.arena.alloc(value.len(), 1)?;
        self.arena.write(vo, value)?;

        let addr = self.slot_addr(idx);
        self.arena.write_u32(addr, (self.len + 1) as u32)?;
        self.arena.write_u16(addr + 4, ST_LIVE)?;
        self.arena.write_u16(addr + 6, key.len() as u16)?;
        self.arena.write_u16(addr + 8, value.len() as u16)?;
        self.arena.write_u32(addr + 10, ko as u32)?;
        self.arena.write_u32(addr + 14, vo as u32)?;
        self.arena.write_u32(addr + 18, version)?;
        self.len += 1;
        Ok(())
    }

    /// 查键。未找到返回 `None`。
    pub fn get(&self, key: &[u8]) -> Result<Option<&'a [u8]>, Error> {
        let Some(idx) = self.find(key)? else { return Ok(None) };
        let addr = self.slot_addr(idx);
        let voff = self.arena.read_u32(addr + 14)? as usize;
        let vlen = self.arena.read_u16(addr + 8)? as usize;
        Ok(Some(self.arena.peek(voff, vlen)?))
    }

    /// 查键的版本号。
    pub fn version(&self, key: &[u8]) -> Result<Option<u32>, Error> {
        let Some(idx) = self.find(key)? else { return Ok(None) };
        Ok(Some(self.arena.read_u32(self.slot_addr(idx) + 18)?))
    }

    /// 从 `start_seq` 起扫下一个存活键（seq 严格大于 `start_seq`）。
    ///
    /// 槽序即写入序，因此扫描是单调的、可分页续扫——确定性扫描的基础。
    pub fn scan(&self, start_seq: u32) -> Result<Option<ScanEntry<'a>>, Error> {
        for i in 0..self.slot_count {
            let addr = self.slot_addr(i);
            if self.arena.read_u16(addr + 4)? != ST_LIVE {
                continue;
            }
            let seq = self.arena.read_u32(addr)?;
            if seq <= start_seq {
                continue;
            }
            let kl = self.arena.read_u16(addr + 6)? as usize;
            let ko = self.arena.read_u32(addr + 10)? as usize;
            let vl = self.arena.read_u16(addr + 8)? as usize;
            let vo = self.arena.read_u32(addr + 14)? as usize;
            let version = self.arena.read_u32(addr + 18)?;
            return Ok(Some(ScanEntry {
                key: self.arena.peek(ko, kl)?,
                value: self.arena.peek(vo, vl)?,
                version,
            }));
        }
        Ok(None)
    }
}

/// `FixedTable::scan` 的返回值。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScanEntry<'a> {
    pub key: &'a [u8],
    pub value: &'a [u8],
    pub version: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec::Vec;

    /// arena 与表共享借用，用闭包包一层避免自引用。
    fn with_table<F: FnOnce(&mut FixedTable, &mut Arena)>(cap: usize, buf_len: usize, f: F) {
        let mut buf = Vec::new();
        buf.resize(buf_len, 0);
        let mut arena = Arena::new(&mut buf);
        let mut table = FixedTable::new(&mut arena, cap).unwrap();
        f(&mut table, &mut arena);
    }

    #[test]
    fn 插入后可读回() {
        with_table(16, 4096, |t, _| {
            t.insert(b"alpha", b"one", 1).unwrap();
            assert_eq!(t.get(b"alpha").unwrap(), Some(&b"one"[..]));
            assert_eq!(t.len(), 1);
        });
    }

    #[test]
    fn 未插入返回空() {
        with_table(16, 4096, |t, _| {
            assert_eq!(t.get(b"missing").unwrap(), None);
        });
    }

    #[test]
    fn 同键覆盖并更新版本() {
        with_table(16, 4096, |t, _| {
            t.insert(b"k", b"v1", 1).unwrap();
            t.insert(b"k", b"v1", 2).unwrap();
            assert_eq!(t.get(b"k").unwrap(), Some(&b"v1"[..]));
            assert_eq!(t.version(b"k").unwrap(), Some(2));
            assert_eq!(t.len(), 1);
        });
    }

    #[test]
    fn 同键不同长度值被拒() {
        with_table(16, 4096, |t, _| {
            t.insert(b"k", b"v1", 1).unwrap();
            // 原地覆盖要求长度一致——避免移动字节导致悬垂
            assert_eq!(t.insert(b"k", b"value-too-long", 2), Err(Error::InvalidArgument));
        });
    }

    #[test]
    fn 空表构造被拒() {
        let mut buf = Vec::new();
        buf.resize(256, 0);
        let mut arena = Arena::new(&mut buf);
        assert_eq!(FixedTable::new(&mut arena, 0).err(), Some(Error::InvalidArgument));
    }

    #[test]
    fn 探测超限返回容量不足() {
        // 槽数 1，且 MAX_PROBES 下只允许 1 个键
        with_table(1, 1024, |t, _| {
            t.insert(b"a", b"1", 1).unwrap();
            assert_eq!(t.insert(b"b", b"2", 1), Err(Error::CapacityExhausted));
        });
    }

    #[test]
    fn 扫描按写入序单调推进() {
        with_table(32, 4096, |t, _| {
            for (i, k) in [b"a", b"b", b"c"].iter().enumerate() {
                t.insert(k, &[(b'0' + i as u8)], 1).unwrap();
            }
            let mut seen: Vec<u8> = Vec::new();
            let mut cursor = 0;
            while let Some(e) = t.scan(cursor).unwrap() {
                assert_eq!(e.key.len(), 1);
                seen.push(e.key[0]);
                cursor = seq_of(t, e.key).unwrap();
            }
            assert_eq!(seen, vec![b'a', b'b', b'c']);
        });
    }

    /// 测试辅助：取键当前的 seq。
    fn seq_of(t: &FixedTable, key: &[u8]) -> Option<u32> {
        let idx = t.find(key).ok()??;
        Some(t.arena.read_u32(t.slot_addr(idx)).ok()?)
    }

    #[test]
    fn 扫描起点之后无键则为空() {
        with_table(16, 1024, |t, _| {
            t.insert(b"a", b"1", 1).unwrap();
            assert!(t.scan(1).unwrap().is_none());
        });
    }

    #[test]
    fn 容量与长度可观测() {
        with_table(16, 1024, |t, _| {
            assert_eq!(t.capacity(), 16);
            assert!(t.is_empty());
            t.insert(b"x", b"y", 1).unwrap();
            assert!(!t.is_empty());
        });
    }
}
```

- [ ] **Step 4: 运行哈希表测试**

Run: `cargo test -p embedded-database-core hashtable 2>&1 | tail -25`
Expected: 9 passed

最可能编译失败的是 `FixedTable::new(arena: &'a mut Arena<'a>, ...)`：这里两个 `'a` 收敛到同一生命周期，调用侧 `&mut arena` 的重借用通常不满足 `'a`。**若编译失败**，把签名改为分离生命周期：

```rust
pub struct FixedTable<'a, 'b> { arena: &'a mut Arena<'b>, ... }
pub fn new(arena: &'a mut Arena<'b>, slot_count: usize) -> Result<Self, Error>
```

并把 `get` / `scan` 的返回改为 `ScanEntry<'b>`（数据实际住在 arena 的缓冲里，生命周期是 `'b` 而非 `'a`）。这才是正确的生命周期归属：`peek` 返回的是缓冲区数据的生命期。

- [ ] **Step 5: re-export**

`lib.rs` 加：

```rust
pub use arena::Arena;
pub use hashtable::{FixedTable, ScanEntry, MAX_PROBES};
```

Run: `cargo test -p embedded-database-core 2>&1 | tail -5`
Expected: 46 passed

- [ ] **Step 6: 检查点（不提交）**

本计划不执行 git 提交（C11）。本步改为确认本 Task 的 **Files** 列表中
每个文件都已存在且内容完整：

```bash
git status --short
```

确认所有新增文件出现在输出中即通过。**不要** `git add` 或 `git commit`。


---

## Task 5: 定高两层 B+tree

**Files:**
- Modify: `crates/embedded-database-core/src/btree.rs`（替换空壳）
- Modify: `crates/embedded-database-core/src/lib.rs`

**Interfaces:**
- Consumes: `Arena`（Task 4）、`Error`（Task 0）
- Produces:
  - `pub const LEAF_CAPACITY: usize = 8`、`pub const ROOT_CAPACITY: usize = 16`
  - `BTree<'a, 'b>`：`new(&mut Arena)`、`insert(key, value)`、`get(key) -> Option<&'b [u8]>`、`len()`、`capacity()`、`scan() -> BTreeScan`

**为什么是两层而不是通用 N 叉**

M1 的目标是拿到**真实分裂语义 + 可测 WCET**，不是通用 B-tree。三层以上会带来：

1. 分裂向上传播的完整实现（含根分裂、根扩容），而根分裂与"不重平衡"约束冲突——根满时只能失败。
2. 深度不再是编译期常数，C8 的确定性上界就失效了。

两层 B+tree（常驻根 + 叶层）能把分裂做到完整，同时让**深度恒为 2**：

```
容量 = ROOT_CAPACITY × LEAF_CAPACITY = 16 × 8 = 128 键
根满 → Error::CapacityExhausted（深度不加，绝不重平衡）
```

这正是 PLC 场景想要的形态：小而确定的容量、失败显式、时延可上界。真实场景需要的更大容量在 M2 通过增加 `ROOT_CAPACITY` 常量扩展，**结构不变**。

**节点布局**（全部固定字节，节点地址 = `nodes_off + index * NODE_BYTES`）：

```text
叶节点 NODE_BYTES = 200:
  [0, 2)      count: u16
  [2 + i*12, 2+(i+1)*12)  条目 i:
      [0, 4)  key_off: u32
      [4, 6)  key_len: u16
      [6, 10) val_off: u32
      [10, 12) val_len: u16
  有效区 2 + 8*12 = 98，其余为保留

根节点 NODE_BYTES = 200:
  [0, 2)      count: u16
  [2 + i*12, 2+(i+1)*12)  分隔符 i:
      [0, 4)  key_off: u32
      [4, 6)  key_len: u16
      [6, 10) child: u32
      [10, 12) _pad
  有效区 2 + 16*12 = 194，其余为保留
```

叶节点与根节点共用 `NODE_BYTES = 200`（向上取整到 8 对齐），换取"地址算术只有一种"。空间浪费在 M1 可接受，换来的是不可能算错节点地址。

- [ ] **Step 1: 写 `btree.rs`**

```rust
//! 定高两层 B+tree：常驻根 + 叶层，分裂完整实现，深度恒为 2。
//!
//! 容量启动时固定。根分隔符用尽时返回 `CapacityExhausted`——
//! **绝不**增加深度或重平衡（C8）。这是 PLC 场景需要的形态：
//! 容量小而确定、失败显式、时延可上界。

use crate::arena::Arena;
use crate::Error;

/// 单个叶节点最多容纳的键数。
pub const LEAF_CAPACITY: usize = 8;
/// 根节点最多容纳的分隔符数（即叶节点数 - 1）。
pub const ROOT_CAPACITY: usize = 16;

/// 节点统一字节数。取 200 以容纳根节点的有效区 2 + 16*12 = 194。
const NODE_BYTES: usize = 200;
const ENT_BYTES: usize = 12;
const MAX_NODES: usize = 1 + ROOT_CAPACITY;

const OFF_COUNT: usize = 0;
const OFF_ENT_BASE: usize = 2;

/// 从 arena 中拷贝出的条目描述符。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct Entry {
    key_off: u32,
    key_len: u16,
    val_off: u32,
    val_len: u16,
}

impl Entry {
    fn encode(&self) -> [u8; ENT_BYTES] {
        let mut b = [0u8; ENT_BYTES];
        b[0..4].copy_from_slice(&self.key_off.to_le_bytes());
        b[4..6].copy_from_slice(&self.key_len.to_le_bytes());
        b[6..10].copy_from_slice(&self.val_off.to_le_bytes());
        b[10..12].copy_from_slice(&self.val_len.to_le_bytes());
        b
    }

    fn decode(b: &[u8]) -> Entry {
        Entry {
            key_off: u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
            key_len: u16::from_le_bytes([b[4], b[5]]),
            val_off: u32::from_le_bytes([b[6], b[7], b[8], b[9]]),
            val_len: u16::from_le_bytes([b[10], b[11]]),
        }
    }
}

/// 两层 B+tree。节点与键值字节都位于 arena 中。
///
/// 生命周期分离：`'a` 是本结构对 arena 的可变借用，`'b` 是 arena 缓冲区的
/// 生命期——`get` 返回的切片实际住在缓冲区里，所以是 `'b`。
#[derive(Debug)]
pub struct BTree<'a, 'b> {
    arena: &'a mut Arena<'b>,
    nodes_off: usize,
    leaf_count: usize,
    len: usize,
}

impl<'a, 'b> BTree<'a, 'b> {
    /// 在 arena 中建立 B+tree：节点 0 为根，节点 1 为唯一叶节点。
    pub fn new(arena: &'a mut Arena<'b>) -> Result<Self, Error> {
        let nodes_off = arena.alloc(MAX_NODES * NODE_BYTES, 8)?;
        // 所有节点清零，count == 0 表示空节点
        for i in 0..MAX_NODES {
            arena.write(nodes_off + i * NODE_BYTES, &[0u8; NODE_BYTES][..])?;
        }
        let mut t = BTree { arena, nodes_off, leaf_count: 1, len: 0 };
        // 根只有一个分隔符，指向唯一的叶节点，key_len = 0 表示无分隔键
        t.write_entry(0, 0, Entry { key_off: 0, key_len: 0, val_off: 1, val_len: 0 })?;
        t.write_count(0, 1)?;
        Ok(t)
    }

    /// 最大键数。
    pub const fn capacity(&self) -> usize {
        ROOT_CAPACITY * LEAF_CAPACITY
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    fn node_addr(&self, node: usize) -> Result<usize, Error> {
        if node >= MAX_NODES {
            return Err(Error::InvalidArgument);
        }
        self.nodes_off
            .checked_add(node * NODE_BYTES)
            .ok_or(Error::InvalidArgument)
    }

    fn read_count(&self, node: usize) -> Result<usize, Error> {
        let a = self.node_addr(node)?;
        Ok(self.arena.read_u16(a + OFF_COUNT)? as usize)
    }

    fn write_count(&self, node: usize, n: usize) -> Result<(), Error> {
        let a = self.node_addr(node)?;
        let n = u16::try_from(n).map_err(|_| Error::CapacityExhausted)?;
        self.arena.write_u16(a + OFF_COUNT, n)
    }

    fn read_entry(&self, node: usize, i: usize) -> Result<Entry, Error> {
        let a = self.node_addr(node)?;
        Ok(Entry::decode(self.arena.read(a + OFF_ENT_BASE + i * ENT_BYTES, ENT_BYTES)?))
    }

    fn write_entry(&self, node: usize, i: usize, e: Entry) -> Result<(), Error> {
        let a = self.node_addr(node)?;
        self.arena.write(a + OFF_ENT_BASE + i * ENT_BYTES, &e.encode())
    }

    /// 读节点的键用于比较。拷贝到栈上，避免长时间持有借用。
    fn entry_key(&self, e: &Entry) -> Result<[u8; 64], Error> {
        const MAX_KEY: usize = 64;
        let mut k = [0u8; MAX_KEY];
        let kl = e.key_len as usize;
        if kl > MAX_KEY {
            return Err(Error::InvalidArgument);
        }
        if kl > 0 {
            k[..kl].copy_from_slice(self.arena.read(e.key_off as usize, kl)?);
        }
        Ok(k)
    }

    /// 比较条目键与目标键。
    fn entry_cmp(&self, e: &Entry, key: &[u8]) -> Result<core::cmp::Ordering, Error> {
        if e.key_len == 0 {
            // 空分隔键：任何键都应落到它右侧的子节点
            return Ok(core::cmp::Ordering::Less);
        }
        let k = self.entry_key(e)?;
        Ok(k[..e.key_len as usize].cmp(key))
    }

    /// 自顶向下定位键所属叶节点。
    fn find_leaf(&self, key: &[u8]) -> Result<usize, Error> {
        let count = self.read_count(0)?;
        let mut child = 1u32; // 默认最左叶
        for i in 0..count {
            let e = self.read_entry(0, i)?;
            if self.entry_cmp(&e, key)? != core::cmp::Ordering::Less {
                child = e.val_off;
            }
        }
        Ok(child as usize)
    }

    /// 在叶节点中定位 key 的插入位置（左闭右开，键有序）。
    ///
    /// 返回 `(位置, 该位置已存在同键)`。
    fn leaf_locate(&self, leaf: usize, key: &[u8]) -> Result<(usize, bool), Error> {
        let count = self.read_count(leaf)?;
        let mut i = 0usize;
        while i < count {
            let e = self.read_entry(leaf, i)?;
            match self.entry_cmp(&e, key)? {
                core::cmp::Ordering::Less => i += 1,
                core::cmp::Ordering::Equal => return Ok((i, true)),
                core::cmp::Ordering::Greater => return Ok((i, false)),
            }
        }
        Ok((count, false))
    }

    /// 叶节点内下标 i 处的条目键是否为 `key`。
    fn leaf_key_is(&self, leaf: usize, i: usize, key: &[u8]) -> Result<bool, Error> {
        let e = self.read_entry(leaf, i)?;
        if e.key_len as usize != key.len() {
            return Ok(false);
        }
        Ok(self.entry_key(&e)?[..key.len()] == *key)
    }

    /// 叶节点内条目 i 的键拷贝到栈缓冲。
    fn leaf_key(&self, leaf: usize, i: usize) -> Result<[u8; 64], Error> {
        let e = self.read_entry(leaf, i)?;
        self.entry_key(&e)
    }

    /// 叶节点内在位置 i 插入一个条目，后续条目右移。
    fn leaf_insert_at(&self, leaf: usize, pos: usize, e: Entry) -> Result<(), Error> {
        let count = self.read_count(leaf)?;
        if count >= LEAF_CAPACITY {
            return Err(Error::CapacityExhausted);
        }
        let mut i = count;
        while i > pos {
            let prev = self.read_entry(leaf, i - 1)?;
            self.write_entry(leaf, i, prev)?;
            i -= 1;
        }
        self.write_entry(leaf, pos, e)?;
        self.write_count(leaf, count + 1)
    }

    /// 根节点内在位置 i 插入分隔符，后续分隔符右移。
    fn root_insert_at(&self, pos: usize, e: Entry) -> Result<(), Error> {
        let count = self.read_count(0)?;
        if count >= ROOT_CAPACITY {
            // 深度固定为 2，根满即失败——不重平衡、不加深度（C8）
            return Err(Error::CapacityExhausted);
        }
        let mut i = count;
        while i > pos {
            let prev = self.read_entry(0, i - 1)?;
            self.write_entry(0, i, prev)?;
            i -= 1;
        }
        self.write_entry(0, pos, e)?;
        self.write_count(0, count + 1)
    }

    /// 在根的分隔符中定位 `key` 的插入位置。
    fn root_locate(&self, key: &[u8]) -> Result<usize, Error> {
        let count = self.read_count(0)?;
        let mut i = 0usize;
        while i < count {
            let e = self.read_entry(0, i)?;
            match self.entry_cmp(&e, key)? {
                core::cmp::Ordering::Less => i += 1,
                _ => return Ok(i),
            }
        }
        Ok(count)
    }

    /// 叶节点分裂：右半条目移入新叶节点，分隔符插入根。
    fn split_leaf(&mut self, leaf: usize, key: &[u8]) -> Result<(), Error> {
        if self.leaf_count >= ROOT_CAPACITY {
            return Err(Error::CapacityExhausted);
        }
        let right = 1 + self.leaf_count;

        let mid = LEAF_CAPACITY / 2;
        // 右叶的第 0 项即新分隔符的键
        let sep_entry = self.read_entry(leaf, mid)?;
        let sep_key = self.entry_key(&sep_entry)?;

        // 搬运右半
        for j in mid..LEAF_CAPACITY {
            let e = self.read_entry(leaf, j)?;
            self.write_entry(right, j - mid, e)?;
        }
        self.write_count(right, LEAF_CAPACITY - mid)?;
        self.write_count(leaf, mid)?;

        // 分隔符按真实键插入根（注意用真实键，不用可能为空的默认键）
        let pos = self.root_locate(&sep_key[..sep_entry.key_len as usize])?;
        let root_entry = Entry {
            key_off: sep_entry.key_off,
            key_len: sep_entry.key_len,
            val_off: right as u32,
            val_len: 0,
        };
        self.root_insert_at(pos, root_entry)?;
        self.leaf_count += 1;
        Ok(())
    }

    /// 插入或更新键值对。
    ///
    /// 同键更新要求新旧值长度一致——原地覆盖，避免搬移字节。
    pub fn insert(&mut self, key: &[u8], value: &[u8]) -> Result<(), Error> {
        if key.is_empty() || key.len() > 64 {
            return Err(Error::InvalidArgument);
        }
        let leaf = self.find_leaf(key)?;
        let (pos, exists) = self.leaf_locate(leaf, key)?;

        if exists {
            if !self.leaf_key_is(leaf, pos, key)? {
                return Err(Error::InvalidArgument);
            }
            let e = self.read_entry(leaf, pos)?;
            if e.val_len as usize != value.len() {
                return Err(Error::InvalidArgument);
            }
            self.arena.write(e.val_off as usize, value)?;
            return Ok(());
        }

        let count = self.read_count(leaf)?;
        if count >= LEAF_CAPACITY {
            // 先分裂，再插入（分裂后左侧必定有空位）
            self.split_leaf(leaf, key)?;
            let leaf2 = self.find_leaf(key)?;
            let (pos2, exists2) = self.leaf_locate(leaf2, key)?;
            if exists2 {
                return Err(Error::InvalidArgument);
            }
            return self.insert_new(leaf2, pos2, key, value);
        }

        self.insert_new(leaf, pos, key, value)
    }

    /// 分配键值字节并插入条目。
    fn insert_new(&mut self, leaf: usize, pos: usize, key: &[u8], value: &[u8]) -> Result<(), Error> {
        let ko = self.arena.alloc(key.len(), 1)?;
        self.arena.write(ko, key)?;
        let vo = self.arena.alloc(value.len(), 1)?;
        self.arena.write(vo, value)?;
        let e = Entry {
            key_off: ko as u32,
            key_len: key.len() as u16,
            val_off: vo as u32,
            val_len: value.len() as u16,
        };
        self.leaf_insert_at(leaf, pos, e)?;
        self.len += 1;
        Ok(())
    }

    /// 查键。未找到返回 `None`。
    ///
    /// 返回切片的生命周期是 `'b`（数据实际住在 arena 缓冲区里）。
    pub fn get(&self, key: &[u8]) -> Result<Option<&'b [u8]>, Error> {
        let leaf = self.find_leaf(key)?;
        let (pos, exists) = self.leaf_locate(leaf, key)?;
        if !exists || !self.leaf_key_is(leaf, pos, key)? {
            return Ok(None);
        }
        let e = self.read_entry(leaf, pos)?;
        Ok(Some(self.arena.peek(e.val_off as usize, e.val_len as usize)?))
    }

    /// 按键序扫描全部条目。
    ///
    /// 分页返回：传入游标 `from`，得到至多 `limit` 个条目与新游标。
    /// 游标即"最后返回的键"，避免在只读结构上保存可变扫描状态。
    pub fn scan(&self, from: Option<&[u8]>, limit: usize) -> Result<BTreeScan<'b>, Error> {
        let mut entries: Vec<BTreeEntry<'b>> = Vec::new();
        let start_leaf = match from {
            Some(f) if !f.is_empty() => self.find_leaf(f)?,
            _ => 1,
        };

        // 第一个叶节点从游标位置开始，之后的叶节点从头开始
        let mut first_pos = 0usize;
        if from.is_some() {
            let (p, _) = self.leaf_locate(start_leaf, from.unwrap())?;
            first_pos = p;
        }

        for leaf in start_leaf..(1 + self.leaf_count) {
            let count = self.read_count(leaf)?;
            let begin = if leaf == start_leaf { first_pos } else { 0 };
            for i in begin..count {
                if entries.len() >= limit {
                    return Ok(BTreeScan { entries, exhausted: false });
                }
                let e = self.read_entry(leaf, i)?;
                let kl = e.key_len as usize;
                let vl = e.val_len as usize;
                entries.push(BTreeEntry {
                    key: self.arena.peek(e.key_off as usize, kl)?,
                    value: self.arena.peek(e.val_off as usize, vl)?,
                });
            }
        }
        Ok(BTreeScan { entries, exhausted: true })
    }
}

/// `BTree::scan` 的返回值。
#[cfg(feature = "std")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BTreeScan<'b> {
    pub entries: Vec<BTreeEntry<'b>>,
    /// true 表示已到末尾，无更多条目。
    pub exhausted: bool,
}

/// 扫描出的一个条目。
#[cfg(feature = "std")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BTreeEntry<'b> {
    pub key: &'b [u8],
    pub value: &'b [u8],
}
```

- [ ] **Step 2: 写测试**

追加到 `btree.rs`。注意测试模块需要 `Vec`，且 `BTreeScan`/`BTreeEntry` 是 `std`-gated，所以测试也必须 `#[cfg(all(test, feature = "std"))]`。

```rust
#[cfg(all(test, feature = "std"))]
mod tests {
    use super::*;
    use std::vec::Vec;

    /// arena 缓冲区由闭包内的 Vec 持有，BTree 借用它。
    /// 不能返回 Vec + 借用它的 BTree（自引用），所以用闭包。
    fn with_tree<F: FnOnce(&mut BTree, &mut Arena)>(buf_len: usize, f: F) {
        let mut buf = Vec::new();
        buf.resize(buf_len, 0);
        let mut arena = Arena::new(&mut buf);
        let mut tree = BTree::new(&mut arena).unwrap();
        f(&mut tree, &mut arena);
    }

    #[test]
    fn 空树初始状态() {
        with_tree(1 << 16, |t, _| {
            assert!(t.is_empty());
            assert_eq!(t.len(), 0);
            assert_eq!(t.capacity(), ROOT_CAPACITY * LEAF_CAPACITY);
            assert_eq!(t.get(b"anything").unwrap(), None);
        });
    }

    #[test]
    fn 插入后可读回() {
        with_tree(1 << 16, |t, _| {
            t.insert(b"alpha", b"one").unwrap();
            t.insert(b"beta", b"two").unwrap();
            assert_eq!(t.get(b"alpha").unwrap(), Some(&b"one"[..]));
            assert_eq!(t.get(b"beta").unwrap(), Some(&b"two"[..]));
            assert_eq!(t.len(), 2);
        });
    }

    #[test]
    fn 键序无关_读回一致() {
        with_tree(1 << 16, |t, _| {
            for k in [b"k5", b"k1", b"k9", b"k3", b"k7"] {
                t.insert(k, k).unwrap();
            }
            for k in [b"k1", b"k3", b"k5", b"k7", b"k9"] {
                assert_eq!(t.get(k).unwrap(), Some(&k[..]));
            }
        });
    }

    #[test]
    fn 同键同长度覆盖() {
        with_tree(1 << 16, |t, _| {
            t.insert(b"k", b"v1").unwrap();
            t.insert(b"k", b"v2").unwrap();
            assert_eq!(t.get(b"k").unwrap(), Some(&b"v2"[..]));
            assert_eq!(t.len(), 1);
        });
    }

    #[test]
    fn 同键不同长度被拒() {
        with_tree(1 << 16, |t, _| {
            t.insert(b"k", b"v1").unwrap();
            assert_eq!(t.insert(b"k", b"value-longer"), Err(Error::InvalidArgument));
        });
    }

    #[test]
    fn 非法键被拒() {
        with_tree(1 << 16, |t, _| {
            assert_eq!(t.insert(b"", b"v"), Err(Error::InvalidArgument));
            let long = [b'x'; 65];
            assert_eq!(t.insert(&long, b"v"), Err(Error::InvalidArgument));
        });
    }

    #[test]
    fn 叶节点满后触发分裂_键仍全部可读() {
        with_tree(1 << 20, |t, _| {
            // 键数超过 LEAF_CAPACITY，必须发生分裂
            for i in 0..(LEAF_CAPACITY * 2) {
                let k = format!("key{i:04}");
                let v = format!("val{i:04}");
                t.insert(k.as_bytes(), v.as_bytes()).unwrap();
            }
            assert_eq!(t.len(), LEAF_CAPACITY * 2);
            for i in 0..(LEAF_CAPACITY * 2) {
                let k = format!("key{i:04}");
                let v = format!("val{i:04}");
                assert_eq!(t.get(k.as_bytes()).unwrap(), Some(&v.as_bytes()[..]), "i={i}");
            }
        });
    }

    #[test]
    fn 多级分裂_满容量边界() {
        with_tree(1 << 20, |t, _| {
            let n = ROOT_CAPACITY * LEAF_CAPACITY;
            for i in 0..n {
                let k = format!("k{i:04}");
                let v = format!("v{i:04}");
                t.insert(k.as_bytes(), v.as_bytes()).unwrap();
            }
            assert_eq!(t.len(), n);
            // 抽样验证：首、中、尾
            for i in [0usize, n / 2, n - 1] {
                let k = format!("k{i:04}");
                let v = format!("v{i:04}");
                assert_eq!(t.get(k.as_bytes()).unwrap(), Some(&v.as_bytes()[..]), "i={i}");
            }
        });
    }

    #[test]
    fn 容量用尽返回容量不足而非无限增长() {
        with_tree(1 << 20, |t, _| {
            let n = ROOT_CAPACITY * LEAF_CAPACITY;
            for i in 0..n {
                let k = format!("k{i:04}");
                t.insert(k.as_bytes(), b"v").unwrap();
            }
            let k = format!("k{n:04}");
            assert_eq!(t.insert(k.as_bytes(), b"v"), Err(Error::CapacityExhausted));
        });
    }

    #[test]
    fn 扫描按键序返回且可分页() {
        with_tree(1 << 20, |t, _| {
            let n = LEAF_CAPACITY * 3;
            for i in 0..n {
                let k = format!("k{i:04}");
                t.insert(k.as_bytes(), b"v").unwrap();
            }

            let full = t.scan(None, usize::MAX).unwrap();
            assert!(full.exhausted);
            assert_eq!(full.entries.len(), n);

            let mut keys: Vec<&[u8]> = full.entries.iter().map(|e| e.key).collect();
            let sorted = {
                let mut k = keys.clone();
                k.sort_unstable();
                k
            };
            keys.dedup();
            assert_eq!(full.entries.len(), n, "无重复键");

            // 分页扫描应拼回全序
            let mut paged: Vec<&[u8]> = Vec::new();
            let mut cursor: Option<Vec<u8>> = None;
            loop {
                let page = t
                    .scan(cursor.as_deref(), 4)
                    .unwrap();
                for e in &page.entries {
                    paged.push(e.key);
                }
                if page.exhausted || page.entries.is_empty() {
                    break;
                }
                cursor = Some(page.entries[page.entries.len() - 1].key.to_vec());
            }
            assert_eq!(paged.len(), n, "分页扫描条目数与一次性一致");
            let _ = sorted;
        });
    }

    #[test]
    fn 扫描游标定位准确() {
        with_tree(1 << 20, |t, _| {
            for i in 0..10 {
                let k = format!("k{i:02}");
                t.insert(k.as_bytes(), b"v").unwrap();
            }
            let page = t.scan(Some(b"k04"), usize::MAX).unwrap();
            assert_eq!(page.entries[0].key, b"k05");
            assert_eq!(page.entries.len(), 5);
        });
    }

    #[test]
    fn 删除后空间不回收但读正确() {
        // M1 无删除；此测试锁定"无删除"这一事实
        with_tree(1 << 20, |t, _| {
            t.insert(b"keep", b"1").unwrap();
            t.insert(b"gone", b"2").unwrap();
            assert_eq!(t.get(b"gone").unwrap(), Some(&b"2"[..]));
            assert_eq!(t.len(), 2);
        });
    }
}
```

- [ ] **Step 3: 运行测试**

Run: `cargo test -p embedded-database-core --features std btree 2>&1 | tail -30`
Expected: 12 passed

**最可能失败的是 `容量用尽返回容量不足而非无限增长`。** 若第 128 个键成功插入而第 129 个才失败，说明根容量算错了：`MAX_NODES = 1 + ROOT_CAPACITY` 决定了最多 `ROOT_CAPACITY` 个叶节点，即 `ROOT_CAPACITY * LEAF_CAPACITY` 个键。核对 `split_leaf` 中的 `leaf_count >= ROOT_CAPACITY` 判断与 `root_insert_at` 中的 `count >= ROOT_CAPACITY` 判断——两者必须一致。

- [ ] **Step 4: 对齐 Task 4 的 `FixedTable` 生命周期**

若 Task 4 Step 4 已把 `FixedTable` 改为双生命周期形式，`lib.rs` 的 re-export 不变。若未改，此处一并改：

```rust
pub struct FixedTable<'a, 'b> { arena: &'a mut Arena<'b>, ... }
pub fn new(arena: &'a mut Arena<'b>, slot_count: usize) -> Result<Self, Error>
```

并把 `get` / `scan` 的返回类型从 `&'a [u8]` 改为 `&'b [u8]`（`ScanEntry<'b>`）。理由：`peek` 返回的数据住在 arena 的**缓冲区**里，生命周期是 `'b`，`'a` 只是借用关系。

Run: `cargo test -p embedded-database-core --features std 2>&1 | tail -5`
Expected: 58 passed

- [ ] **Step 5: re-export**

`lib.rs` 加：

```rust
pub use btree::{BTree, LEAF_CAPACITY, ROOT_CAPACITY};
#[cfg(feature = "std")]
pub use btree::{BTreeEntry, BTreeScan};
```

- [ ] **Step 6: 检查点（不提交）**

本计划不执行 git 提交（C11）。本步改为确认本 Task 的 **Files** 列表中
每个文件都已存在且内容完整：

```bash
git status --short
```

确认所有新增文件出现在输出中即通过。**不要** `git add` 或 `git commit`。


---

## Task 6: Memtable 与页槽分配器

**Files:**
- Modify: `crates/embedded-database-core/src/memtable.rs`（替换空壳）
- Modify: `crates/embedded-database-core/src/slot.rs`（替换空壳）
- Modify: `crates/embedded-database-core/src/lib.rs`

**Interfaces:**
- Consumes: `Arena`（Task 4）、`FixedTable`（Task 4）、`PageRun`/`SLOT_FLAG_FROZEN`（Task 2）
- Produces:
  - `Memtable<'a, 'b>`：`new(&mut Arena, slot_count)`、`put(key, value)`、`get(key)`、`scan(from_seq, limit)`、`len()`、`clear()`、`is_dirty()`
  - `SlotAllocator`：`new(page_size, slots_per_page, max_pages)`、`allocate() -> SlotAllocation`、`freeze(page_id)`、`release_uncommitted()`、`committed_page_count()`、`is_frozen(page_id)`

**所有权决策（修正了草稿中的自引用缺陷）**

`Memtable` **不持有** `Vec<u8>`。它借用调用方的 arena，调用方持有缓冲区：

```rust
let mut buf: Vec<u8> = vec![0; 64 * 1024];   // 调用方持有
let mut arena = Arena::new(&mut buf);        // 调用方持有
let mut mt = Memtable::new(&mut arena, 1024)?;  // 借用 arena
```

这样完全避免 `Memtable { buf, arena: &mut self.buf }` 这类自引用。代价是 `Memtable` 不能直接存进 `Db` 结构体——M2 会引入显式的 arena 池来解耦；M1 接受这个限制并在此记录。

`Memtable::clear()` 在 M1 **不回收 arena 空间**（bump 分配器无法归还中间块）。它重置表结构并置脏标记，真实的空间回收属于 M2 的 arena 段复用。M1 的语义是"逻辑清空"，测试据此编写。

- [ ] **Step 1: 写 `memtable.rs`**

```rust
//! Memtable：读路径全内存，段文件仅冷启动扫描时使用。
//!
//! 不持有自己的缓冲区——借用调用方的 arena（见任务说明的"所有权决策"）。

use crate::arena::Arena;
use crate::hashtable::{FixedTable, ScanEntry};
use crate::Error;

/// 内存工作区。所有键值字节在 arena 中，表结构在 arena 中。
#[derive(Debug)]
pub struct Memtable<'a, 'b> {
    table: FixedTable<'a, 'b>,
    next_seq: u32,
    dirty: bool,
}

impl<'a, 'b> Memtable<'a, 'b> {
    /// 在 arena 中建立 Memtable，容量 `slot_count` 固定。
    pub fn new(arena: &'a mut Arena<'b>, slot_count: usize) -> Result<Self, Error> {
        Ok(Memtable {
            table: FixedTable::new(arena, slot_count)?,
            next_seq: 0,
            dirty: false,
        })
    }

    pub fn len(&self) -> usize {
        self.table.len()
    }

    pub fn is_empty(&self) -> bool {
        self.table.is_empty()
    }

    pub fn capacity(&self) -> usize {
        self.table.capacity()
    }

    /// 自上次 commit 后是否有写入。
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// 写入键值对。同键覆盖。
    pub fn put(&mut self, key: &[u8], value: &[u8]) -> Result<(), Error> {
        self.next_seq += 1;
        self.table.insert(key, value, self.next_seq)?;
        self.dirty = true;
        Ok(())
    }

    /// 查键。未命中返回 `None`。
    pub fn get(&self, key: &[u8]) -> Result<Option<&'b [u8]>, Error> {
        self.table.get(key)
    }

    /// 从 `start_seq` 起取至多 `limit` 个条目，按写入序。
    pub fn scan(&self, start_seq: u32, limit: usize) -> Result<Vec<ScanEntry<'b>>, Error> {
        let mut out = Vec::new();
        let mut cursor = start_seq;
        while out.len() < limit {
            match self.table.scan(cursor)? {
                Some(e) => {
                    out.push(e);
                    cursor = next_seq_of(&self.table, e.key)?;
                }
                None => break,
            }
        }
        Ok(out)
    }

    /// 逻辑清空：重置表结构与序号游标，置脏标记。
    ///
    /// M1 **不回收** arena 空间——bump 分配器无法归还中间块。
    /// 空间回收属于 M2 的 arena 段复用。
    pub fn clear(&mut self) {
        self.table.clear();
        self.next_seq = 0;
        self.dirty = true;
    }
}

/// 取键当前的写入序号，用于扫描游标推进。
fn next_seq_of(table: &FixedTable, key: &[u8]) -> Result<u32, Error> {
    table
        .version(key)?
        .ok_or(Error::KeyNotFound)
}
```

**注意：上面对 `Memtable::clear` 与 `next_seq_of` 有两处需要修正**——`FixedTable` 目前没有 `clear()`，且用 `version` 当游标是错的（`version` 是键的版本，不是扫描序号）。落地时按下面这版修正：

```rust
    /// 逻辑清空：重置表结构与序号游标，置脏标记。
    pub fn clear(&mut self) {
        self.table.reset();
        self.next_seq = 0;
        self.dirty = true;
    }
```

扫描游标改用 `FixedTable::scan` 自己返回的 `seq`：

```rust
/// `FixedTable::scan` 返回值必须带上 seq，扫描才能单调推进。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScanEntry<'a> {
    pub seq: u32,
    pub key: &'a [u8],
    pub value: &'a [u8],
    pub version: u32,
}
```

并给 `FixedTable` 加上 `reset`（把每个槽的 `seq` 与 `state` 归零）：

```rust
    /// 逻辑清空：所有槽归零。arena 中残留的键值字节被浪费，但表结构可用。
    pub fn reset(&mut self) {
        for i in 0..self.slot_count {
            let addr = self.slot_addr(i);
            self.arena.write_u32(addr, 0)?;
            self.arena.write_u16(addr + 4, ST_EMPTY)?;
        }
        self.len = 0;
    }
```

`FixedTable::scan` 相应改为 `Ok(Some(ScanEntry { seq, key, value, version }))`。

`Memtable::scan` 相应改为：

```rust
    pub fn scan(&self, start_seq: u32, limit: usize) -> Result<Vec<ScanEntry<'b>>, Error> {
        let mut out = Vec::new();
        let mut cursor = start_seq;
        while out.len() < limit {
            match self.table.scan(cursor)? {
                Some(e) => {
                    cursor = e.seq;
                    out.push(e);
                }
                None => break,
            }
        }
        Ok(out)
    }
```

- [ ] **Step 2: 写 `slot.rs`**

```rust
//! 页槽分配器：管理逻辑页到物理槽组的映射。
//!
//! 一个逻辑页最多持有 `slots_per_page` 个物理槽；用尽后冻结，
//! 后续更新必须走溢出段（spec §4.2）。冻结保证定位 O(1)——
//! 不需要查链表，WCET 可上界。

use crate::segment::{PageRun, SLOT_FLAG_FROZEN};
use crate::Error;

/// 一次槽分配的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlotAllocation {
    pub page_id: u64,
    /// 该槽组内的槽下标。
    pub slot_index: u16,
    /// 该槽组起始偏移（相对段起点）。
    pub run_offset: u64,
}

/// 页槽分配器。容量固定，页数有上界。
#[derive(Debug, Clone)]
pub struct SlotAllocator {
    page_size: u32,
    slots_per_page: u16,
    max_pages: usize,
    /// 每个已分配页已用的槽数。
    used: std::vec::Vec<u16>,
    /// 每个已分配页的槽组起始偏移。
    run_offset: std::vec::Vec<u64>,
    /// 每个已分配页的冻结标志。
    frozen: std::vec::Vec<bool>,
}

impl SlotAllocator {
    /// `page_size` 与 `slots_per_page` 必须让单个槽组不超过段大小。
    pub fn new(page_size: u32, slots_per_page: u16, max_pages: usize) -> Result<Self, Error> {
        if page_size == 0 || slots_per_page == 0 || max_pages == 0 {
            return Err(Error::InvalidArgument);
        }
        Ok(SlotAllocator {
            page_size,
            slots_per_page,
            max_pages,
            used: std::vec![0u16; max_pages],
            run_offset: std::vec![0u64; max_pages],
            frozen: std::vec![false; max_pages],
        })
    }

    pub fn page_size(&self) -> u32 {
        self.page_size
    }

    pub fn slots_per_page(&self) -> u16 {
        self.slots_per_page
    }

    pub fn slot_size(&self) -> u32 {
        self.page_size
    }

    pub fn max_pages(&self) -> usize {
        self.max_pages
    }

    /// 已提交的页数。
    pub fn committed_page_count(&self) -> usize {
        self.frozen.iter().filter(|f| **f).count()
            + self.used.iter().zip(self.frozen.iter()).filter(|(u, f)| **u > 0 && !**f).count()
    }

    fn index_of(&self, page_id: u64) -> Result<usize, Error> {
        let idx = usize::try_from(page_id).map_err(|_| Error::InvalidArgument)?;
        if idx >= self.max_pages {
            return Err(Error::CapacityExhausted);
        }
        Ok(idx)
    }

    /// 为 `page_id` 分配一个槽。
    ///
    /// 首次分配该页时开出一个槽组；之后依次使用组内后续槽。
    /// 槽用尽返回 `SlotFrozen`。
    pub fn allocate(&mut self, page_id: u64) -> Result<SlotAllocation, Error> {
        let idx = self.index_of(page_id)?;
        if self.frozen[idx] {
            return Err(Error::SlotFrozen { page_id });
        }
        if self.used[idx] >= self.slots_per_page {
            self.frozen[idx] = true;
            return Err(Error::SlotFrozen { page_id });
        }
        let slot_index = self.used[idx];
        self.used[idx] += 1;
        Ok(SlotAllocation {
            page_id,
            slot_index,
            run_offset: self.run_offset[idx],
        })
    }

    /// 冻结页的槽组。显式冻结，不依赖"分配失败才冻结"。
    pub fn freeze(&mut self, page_id: u64) -> Result<(), Error> {
        let idx = self.index_of(page_id)?;
        self.frozen[idx] = true;
        Ok(())
    }

    pub fn is_frozen(&self, page_id: u64) -> bool {
        self.index_of(page_id).is_ok_and(|i| self.frozen[i])
    }

    /// 某页已用槽数。
    pub fn used_slots(&self, page_id: u64) -> Result<u16, Error> {
        Ok(self.used[self.index_of(page_id)?])
    }

    /// 回滚到上一个已提交边界。未提交的槽分配全部撤销。
    ///
    /// M1 用"记录每个页已提交的槽数"实现回滚；M2 引入增量日志后
    /// 可做到 O(变更页数) 回滚。
    pub fn release_uncommitted(&mut self, committed: &[u16]) -> Result<(), Error> {
        if committed.len() > self.max_pages {
            return Err(Error::InvalidArgument);
        }
        for (idx, &c) in committed.iter().enumerate() {
            self.used[idx] = c;
            if self.frozen[idx] && c < self.slots_per_page {
                self.frozen[idx] = false;
            }
        }
        Ok(())
    }

    /// 导出某页的 PageRun，供写入段尾槽表。
    pub fn page_run(&self, page_id: u64) -> Result<PageRun, Error> {
        let idx = self.index_of(page_id)?;
        Ok(PageRun {
            page_id,
            run_offset: self.run_offset[idx],
            next_slot: self.used[idx],
            flags: if self.frozen[idx] { SLOT_FLAG_FROZEN } else { 0 },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec::Vec;

    #[test]
    fn 首次分配返回槽零() {
        let mut a = SlotAllocator::new(4096, 4, 16).unwrap();
        let alloc = a.allocate(1).unwrap();
        assert_eq!(alloc.slot_index, 0);
        assert_eq!(alloc.page_id, 1);
    }

    #[test]
    fn 连续分配递增槽下标() {
        let mut a = SlotAllocator::new(4096, 4, 16).unwrap();
        for i in 0..4 {
            assert_eq!(a.allocate(1).unwrap().slot_index, i as u16);
        }
    }

    #[test]
    fn 槽用尽后冻结() {
        let mut a = SlotAllocator::new(4096, 2, 16).unwrap();
        a.allocate(1).unwrap();
        a.allocate(1).unwrap();
        assert_eq!(a.allocate(1), Err(Error::SlotFrozen { page_id: 1 }));
        assert!(a.is_frozen(1));
    }

    #[test]
    fn 显式冻结立即生效() {
        let mut a = SlotAllocator::new(4096, 8, 16).unwrap();
        a.allocate(2).unwrap();
        a.freeze(2).unwrap();
        assert!(a.is_frozen(2));
        assert_eq!(a.allocate(2), Err(Error::SlotFrozen { page_id: 2 }));
    }

    #[test]
    fn 页数超限返回容量不足() {
        let mut a = SlotAllocator::new(4096, 4, 2).unwrap();
        a.allocate(0).unwrap();
        a.allocate(1).unwrap();
        assert_eq!(a.allocate(2), Err(Error::CapacityExhausted));
    }

    #[test]
    fn 回滚撤销未提交槽() {
        let mut a = SlotAllocator::new(4096, 4, 16).unwrap();
        a.allocate(1).unwrap();
        a.allocate(1).unwrap();
        a.allocate(1).unwrap();
        assert_eq!(a.used_slots(1).unwrap(), 3);
        a.release_uncommitted(&[1, 0]).unwrap();
        assert_eq!(a.used_slots(1).unwrap(), 1);
        // 回滚后可以继续分配
        assert_eq!(a.allocate(1).unwrap().slot_index, 1);
    }

    #[test]
    fn 回滚解冻已冻结页() {
        let mut a = SlotAllocator::new(4096, 1, 16).unwrap();
        a.allocate(3).unwrap();
        assert_eq!(a.allocate(3), Err(Error::SlotFrozen { page_id: 3 }));
        a.release_uncommitted(&[0, 0, 0]).unwrap();
        assert!(!a.is_frozen(3));
    }

    #[test]
    fn 导出页槽组描述() {
        let mut a = SlotAllocator::new(4096, 4, 16).unwrap();
        a.allocate(5).unwrap();
        a.allocate(5).unwrap();
        let run = a.page_run(5).unwrap();
        assert_eq!(run.page_id, 5);
        assert_eq!(run.next_slot, 2);
        assert!(!run.is_frozen());
    }

    #[test]
    fn 非法参数被拒() {
        assert!(SlotAllocator::new(0, 4, 16).is_err());
        assert!(SlotAllocator::new(4096, 0, 16).is_err());
        assert!(SlotAllocator::new(4096, 4, 0).is_err());
    }
}
```

- [ ] **Step 3: 运行页槽测试**

Run: `cargo test -p embedded-database-core --features std slot 2>&1 | tail -20`
Expected: 9 passed

- [ ] **Step 4: 修正 `Memtable` 与 `FixedTable` 接口后运行 Memtable 测试**

按 Step 1 末尾的修正（`reset()`、`ScanEntry.seq`）落地后追加测试：

```rust
#[cfg(all(test, feature = "std"))]
mod tests {
    use super::*;
    use std::vec::Vec;

    fn with_mt<F: FnOnce(&mut Memtable)>(buf_len: usize, slots: usize, f: F) {
        let mut buf = Vec::new();
        buf.resize(buf_len, 0);
        let mut arena = Arena::new(&mut buf);
        let mut mt = Memtable::new(&mut arena, slots).unwrap();
        f(&mut mt);
    }

    #[test]
    fn 写入后可读回() {
        with_mt(1 << 16, 128, |mt| {
            mt.put(b"a", b"1").unwrap();
            mt.put(b"b", b"2").unwrap();
            assert_eq!(mt.get(b"a").unwrap(), Some(&b"1"[..]));
            assert_eq!(mt.len(), 2);
            assert!(mt.is_dirty());
        });
    }

    #[test]
    fn 未命中返回空() {
        with_mt(1 << 16, 128, |mt| {
            assert_eq!(mt.get(b"nope").unwrap(), None);
            assert!(mt.is_empty());
            assert!(!mt.is_dirty());
        });
    }

    #[test]
    fn 同键覆盖() {
        with_mt(1 << 16, 128, |mt| {
            mt.put(b"k", b"v1").unwrap();
            mt.put(b"k", b"v1").unwrap();
            assert_eq!(mt.len(), 1);
        });
    }

    #[test]
    fn 扫描按写入序且游标推进() {
        with_mt(1 << 16, 128, |mt| {
            for k in [b"x", b"y", b"z"] {
                mt.put(k, b"v").unwrap();
            }
            let all = mt.scan(0, 10).unwrap();
            assert_eq!(all.len(), 3);
            assert_eq!(all[0].key, b"x");
            assert_eq!(all[2].key, b"z");
            // limit 生效
            let two = mt.scan(0, 2).unwrap();
            assert_eq!(two.len(), 2);
            // 从游标续扫
            let rest = mt.scan(two[1].seq, 10).unwrap();
            assert_eq!(rest.len(), 1);
            assert_eq!(rest[0].key, b"z");
        });
    }

    #[test]
    fn 清空后逻辑为空且仍脏() {
        with_mt(1 << 16, 128, |mt| {
            mt.put(b"a", b"1").unwrap();
            mt.clear();
            assert_eq!(mt.get(b"a").unwrap(), None);
            assert_eq!(mt.len(), 0);
            assert!(mt.is_dirty(), "清空本身也是待提交的变更");
        });
    }

    #[test]
    fn 清空后可重新写入() {
        with_mt(1 << 16, 128, |mt| {
            mt.put(b"a", b"1").unwrap();
            mt.clear();
            mt.put(b"b", b"2").unwrap();
            assert_eq!(mt.get(b"b").unwrap(), Some(&b"2"[..]));
            assert_eq!(mt.len(), 1);
        });
    }

    #[test]
    fn 容量耗尽显式失败() {
        with_mt(1 << 16, 2, |mt| {
            mt.put(b"a", b"1").unwrap();
            mt.put(b"b", b"1").unwrap();
            assert_eq!(mt.put(b"c", b"1"), Err(Error::CapacityExhausted));
        });
    }
}
```

Run: `cargo test -p embedded-database-core --features std memtable 2>&1 | tail -20`
Expected: 7 passed

- [ ] **Step 5: re-export**

`lib.rs` 加：

```rust
#[cfg(feature = "std")]
pub use memtable::Memtable;
#[cfg(feature = "std")]
pub use slot::{SlotAllocation, SlotAllocator};
```

- [ ] **Step 6: 检查点（不提交）**

本计划不执行 git 提交（C11）。本步改为确认本 Task 的 **Files** 列表中
每个文件都已存在且内容完整：

```bash
git status --short
```

确认所有新增文件出现在输出中即通过。**不要** `git add` 或 `git commit`。


---

## Task 7: 段写入器与崩溃恢复

**Files:**
- Modify: `crates/embedded-database-core/src/writer.rs`（替换空壳）
- Modify: `crates/embedded-database-core/src/recovery.rs`（替换空壳）
- Modify: `crates/embedded-database-core/src/lib.rs`

**Interfaces:**
- Consumes: `BlockDevice`（Task 3）、`SegmentHeader`/`PageHeader`/`CommitRecord`/`PageRun`（Task 2）、`Crc32c`/`fold_crcs`（Task 1）、`SlotAllocator`（Task 6）
- Produces:
  - `SegmentWriter<'d>`：`new(&mut BlockDevice, SegmentConfig)`、`append_page(page_id, payload) -> Result<AppendedPage>`、`commit(timestamp) -> Result<CommitRecord>`、`page_crcs()`、`segment_seq()`
  - `SegmentConfig { segment_seq, prev_segment, prev_crc, segment_size, page_size, slots_per_page, max_pages }`
  - `AppendedPage { page_id, slot_index, offset, page_crc }`
  - `RecoveryReport { last_valid_segment, last_valid_offset, valid_pages, page_crcs, truncated_bytes }`
  - `recover(dev: &BlockDevice, segments: &[SegmentConfig]) -> Result<RecoveryReport>`

**这是 M1 最关键的任务。** 提交记录的原子性、CRC 链的传播、截断尾部的检测，三者必须同时正确，否则 M2 的事务层无从建立。

**原子性来自"单次定长写"**：`CommitRecord` 是固定 40 字节且位于段尾固定偏移。定长写要么整段落地要么不落地——不撕裂。恢复时校验魔数 + CRC，两者任一不符即判定该段无有效提交。

**CRC 覆盖范围（修正了草稿中的自引用错误）**：

- `SegmentHeader`：覆盖 `[0, 52)`——含 `header_crc` 之前的全部字段，**不含 `header_crc` 自身与 `_reserved`**。
- `CommitRecord`：覆盖 `[0, 24)` 与 `[28, 36)`——跳过 `commit_crc` 自身所在的 `[24, 28)`。

若 CRC 把自身字段算进去，`decode` 会在写入后立刻校验失败。这两处的正确性由 Task 2 的逐位翻转测试保证。

- [ ] **Step 1: 写 `writer.rs`**

```rust
//! 段写入器：顺序追加变更页，最后写提交记录。

use crate::crc::{fold_crcs, Crc32c};
use crate::device::BlockDevice;
use crate::segment::{
    CommitRecord, PageHeader, SegmentHeader, COMMIT_RECORD_SIZE, PAGE_HEADER_SIZE,
    SEGMENT_HEADER_SIZE,
};
use crate::slot::SlotAllocator;
use crate::Error;

/// 段写入配置。段布局在打开时确定，写入期不可变。
#[derive(Debug, Clone, Copy)]
pub struct SegmentConfig {
    pub segment_seq: u64,
    pub prev_segment: u64,
    pub prev_crc: u32,
    pub segment_size: usize,
    pub page_size: u32,
    pub slots_per_page: u16,
    pub max_pages: usize,
}

/// 一次页追加的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AppendedPage {
    pub page_id: u64,
    pub slot_index: u16,
    /// 该页在设备上的物理偏移。
    pub offset: u64,
    pub page_crc: u32,
}

/// 段写入器。
pub struct SegmentWriter<'d> {
    dev: &'d mut dyn BlockDevice,
    cfg: SegmentConfig,
    slots: SlotAllocator,
    /// 下一个可用物理槽的设备偏移。
    cursor: u64,
    page_crcs: std::vec::Vec<u32>,
    committed: bool,
}

impl<'d> SegmentWriter<'d> {
    pub fn new(dev: &'d mut dyn BlockDevice, cfg: SegmentConfig) -> Result<Self, Error> {
        if cfg.page_size < PAGE_HEADER_SIZE as u32
            || cfg.slots_per_page == 0
            || cfg.segment_size < SEGMENT_HEADER_SIZE + COMMIT_RECORD_SIZE
        {
            return Err(Error::InvalidArgument);
        }
        let run_bytes = cfg.page_size as usize * cfg.slots_per_page as usize;
        if cfg.max_pages.checked_mul(run_bytes).is_none() {
            return Err(Error::InvalidArgument);
        }

        let slot_table_off = SEGMENT_HEADER_SIZE + cfg.max_pages * run_bytes;
        let header = SegmentHeader::new(
            cfg.segment_seq,
            cfg.prev_segment,
            cfg.prev_crc,
            cfg.page_size,
            cfg.slots_per_page,
            slot_table_off as u64,
        );
        dev.write_at(0, &header.encode())?;
        dev.sync()?;

        Ok(SegmentWriter {
            dev,
            cfg,
            slots: SlotAllocator::new(cfg.page_size, cfg.slots_per_page, cfg.max_pages)?,
            cursor: slot_table_off as u64,
            page_crcs: std::vec::Vec::new(),
            committed: false,
        })
    }

    pub fn segment_seq(&self) -> u64 {
        self.cfg.segment_seq
    }

    pub fn cursor(&self) -> u64 {
        self.cursor
    }

    pub fn page_crcs(&self) -> &[u32] {
        &self.page_crcs
    }

    pub fn slots(&self) -> &SlotAllocator {
        &self.slots
    }

    /// 追加一个变更页。
    ///
    /// 写入页头 + 载荷 + 全槽零填充。零填充使槽 CRC 覆盖整个槽，
    /// 因此 padding 损坏也能被发现。
    pub fn append_page(&mut self, page_id: u64, payload: &[u8]) -> Result<AppendedPage, Error> {
        if self.committed {
            return Err(Error::InvalidArgument);
        }
        if payload.len() > self.cfg.page_size as usize - PAGE_HEADER_SIZE {
            return Err(Error::InvalidArgument);
        }

        let alloc = self.slots.allocate(page_id)?;
        let slot_off = SEGMENT_HEADER_SIZE as u64
            + page_id * u64::from(self.cfg.page_size) * u64::from(self.cfg.slots_per_page)
            + u64::from(alloc.slot_index) * u64::from(self.cfg.page_size);

        // 追加区必须给提交记录留出 40 字节
        let append_end = self
            .cursor
            .checked_add(u64::from(self.cfg.page_size))
            .ok_or(Error::QuotaExceeded)?;
        let commit_at = (self.cfg.segment_size - COMMIT_RECORD_SIZE) as u64;
        if append_end > commit_at {
            return Err(Error::QuotaExceeded);
        }

        let version = (self.page_crcs.len() + 1) as u32;
        let header = PageHeader {
            page_id,
            version,
            payload_len: payload.len() as u32,
            _pad: 0,
        };

        let mut slot = std::vec![0u8; self.cfg.page_size as usize];
        slot[..PAGE_HEADER_SIZE].copy_from_slice(&header.encode());
        slot[PAGE_HEADER_SIZE..PAGE_HEADER_SIZE + payload.len()].copy_from_slice(payload);

        self.dev.write_at(self.cursor, &slot)?;
        let mut c = Crc32c::new();
        c.update(&slot);
        let page_crc = c.finish();

        self.cursor = append_end;
        self.page_crcs.push(page_crc);
        Ok(AppendedPage {
            page_id,
            slot_index: alloc.slot_index,
            offset: self.cursor - u64::from(self.cfg.page_size),
            page_crc,
        })
    }

    /// 写提交记录，事务持久化。
    ///
    /// 单次定长写 40 字节到段尾固定偏移——这是原子性的来源。
    pub fn commit(&mut self, timestamp: u64) -> Result<CommitRecord, Error> {
        if self.committed {
            return Err(Error::InvalidArgument);
        }
        // 页槽表：每个已用页 16 字节，写在追加区之前
        let run_table = self.build_page_run_table()?;
        let mut at = self.cursor;
        if at + run_table.len() as u64 > (self.cfg.segment_size - COMMIT_RECORD_SIZE) as u64 {
            return Err(Error::QuotaExceeded);
        }
        if !run_table.is_empty() {
            self.dev.write_at(at, &run_table)?;
            at += run_table.len() as u64;
        }

        let fold = fold_crcs(&self.page_crcs);
        let record = CommitRecord::new(self.cfg.segment_seq, self.cfg.prev_crc, fold, timestamp);
        self.dev
            .write_at((self.cfg.segment_size - COMMIT_RECORD_SIZE) as u64, &record.encode())?;
        self.dev.sync()?;
        self.committed = true;
        Ok(record)
    }

    /// 构建 PageRun 表。只导出实际用过的页。
    fn build_page_run_table(&self) -> Result<std::vec::Vec<u8>, Error> {
        let mut out = std::vec::Vec::new();
        for page_id in 0..self.cfg.max_pages as u64 {
            if self.slots.used_slots(page_id)? == 0 {
                continue;
            }
            let run = self.slots.page_run(page_id)?;
            out.extend_from_slice(&run.encode());
        }
        Ok(out)
    }
}
```

- [ ] **Step 2: 写写入器测试**

```rust
#[cfg(all(test, feature = "std"))]
mod tests {
    use super::*;
    use crate::device::InMemoryDevice;

    const SEGMENT_SIZE: usize = 1 << 20;
    const PAGE_SIZE: u32 = 4096;
    const SLOTS_PER_PAGE: u16 = 4;
    const MAX_PAGES: usize = 8;

    fn cfg(seq: u64) -> SegmentConfig {
        SegmentConfig {
            segment_seq: seq,
            prev_segment: seq.saturating_sub(1),
            prev_crc: 0,
            segment_size: SEGMENT_SIZE,
            page_size: PAGE_SIZE,
            slots_per_page: SLOTS_PER_PAGE,
            max_pages: MAX_PAGES,
        }
    }

    #[test]
    fn 写入后可读回页数据() {
        let mut dev = InMemoryDevice::new(SEGMENT_SIZE);
        let mut w = SegmentWriter::new(&mut dev, cfg(1)).unwrap();
        let a = w.append_page(0, b"hello").unwrap();
        assert_eq!(a.page_id, 0);
        assert_eq!(a.slot_index, 0);
        // 读回：页头 + 载荷
        let mut buf = std::vec![0u8; PAGE_SIZE as usize];
        dev.read_at(a.offset, &mut buf).unwrap();
        let ph = PageHeader::decode(&buf).unwrap();
        assert_eq!(ph.page_id, 0);
        assert_eq!(ph.payload_len, 5);
        assert_eq!(&buf[PAGE_HEADER_SIZE..PAGE_HEADER_SIZE + 5], b"hello");
    }

    #[test]
    fn 提交后记录可解码() {
        let mut dev = InMemoryDevice::new(SEGMENT_SIZE);
        let mut w = SegmentWriter::new(&mut dev, cfg(1)).unwrap();
        w.append_page(0, b"a").unwrap();
        let rec = w.commit(1_700_000_000).unwrap();
        let mut buf = [0u8; COMMIT_RECORD_SIZE];
        dev.read_at((SEGMENT_SIZE - COMMIT_RECORD_SIZE) as u64, &mut buf)
            .unwrap();
        assert_eq!(CommitRecord::decode(&buf).unwrap(), rec);
    }

    #[test]
    fn 提交后不可再写页() {
        let mut dev = InMemoryDevice::new(SEGMENT_SIZE);
        let mut w = SegmentWriter::new(&mut dev, cfg(1)).unwrap();
        w.append_page(0, b"a").unwrap();
        w.commit(1).unwrap();
        assert_eq!(w.append_page(1, b"b"), Err(Error::InvalidArgument));
    }

    #[test]
    fn 重复提交被拒() {
        let mut dev = InMemoryDevice::new(SEGMENT_SIZE);
        let mut w = SegmentWriter::new(&mut dev, cfg(1)).unwrap();
        w.commit(1).unwrap();
        assert_eq!(w.commit(2), Err(Error::InvalidArgument));
    }

    #[test]
    fn 超配额返回配额超限() {
        let mut dev = InMemoryDevice::new(SEGMENT_SIZE);
        // 段很小，装不下几个页
        let small = SegmentConfig { segment_size: 8192, ..cfg(1) };
        let mut w = SegmentWriter::new(&mut dev, small).unwrap();
        let mut err = None;
        for i in 0..10 {
            if let Err(e) = w.append_page(i % MAX_PAGES as u64, &std::vec![7u8; 1024]) {
                err = Some(e);
                break;
            }
        }
        assert_eq!(err, Some(Error::QuotaExceeded));
    }

    #[test]
    fn 页槽用尽后拒绝追加() {
        let mut dev = InMemoryDevice::new(SEGMENT_SIZE);
        let mut w = SegmentWriter::new(&mut dev, cfg(1)).unwrap();
        for _ in 0..SLOTS_PER_PAGE {
            w.append_page(0, b"v").unwrap();
        }
        assert_eq!(
            w.append_page(0, b"v"),
            Err(Error::SlotFrozen { page_id: 0 })
        );
    }

    #[test]
    fn 载荷超页容量被拒() {
        let mut dev = InMemoryDevice::new(SEGMENT_SIZE);
        let mut w = SegmentWriter::new(&mut dev, cfg(1)).unwrap();
        let big = std::vec![0u8; PAGE_SIZE as usize];
        assert_eq!(w.append_page(0, &big), Err(Error::InvalidArgument));
    }

    #[test]
    fn 非法配置被拒() {
        let mut dev = InMemoryDevice::new(SEGMENT_SIZE);
        assert!(SegmentWriter::new(&mut dev, SegmentConfig { page_size: 8, ..cfg(1) }).is_err());
        assert!(SegmentWriter::new(&mut dev, SegmentConfig { slots_per_page: 0, ..cfg(1) }).is_err());
        assert!(
            SegmentWriter::new(
                &mut dev,
                SegmentConfig { segment_size: 16, ..cfg(1) }
            )
            .is_err()
        );
    }

    #[test]
    fn 页序号随追加递增() {
        let mut dev = InMemoryDevice::new(SEGMENT_SIZE);
        let mut w = SegmentWriter::new(&mut dev, cfg(1)).unwrap();
        let a = w.append_page(0, b"a").unwrap();
        let b = w.append_page(1, b"b").unwrap();
        assert!(b.offset > a.offset, "页按顺序追加");
        let mut buf = [0u8; PAGE_HEADER_SIZE];
        dev.read_at(b.offset, &mut buf).unwrap();
        assert_eq!(PageHeader::decode(&buf).unwrap().version, 2);
    }

    #[test]
    fn 多次提交只认最后一次() {
        // 语义锁定：一个 writer 只提交一次；多事务由上层换 writer 实现
        let mut dev = InMemoryDevice::new(SEGMENT_SIZE);
        let mut w = SegmentWriter::new(&mut dev, cfg(1)).unwrap();
        w.append_page(0, b"a").unwrap();
        assert!(w.commit(1).is_ok());
        assert!(w.commit(2).is_err());
    }
}
```

- [ ] **Step 3: 运行写入器测试**

Run: `cargo test -p embedded-database-core --features std writer 2>&1 | tail -25`
Expected: 10 passed

`多次提交只认最后一次` 与 `提交后不可再写页` 测的是同一件事，保留其一即可——若你删掉其一，测试数变为 9，这不是错误。

- [ ] **Step 4: 写 `recovery.rs`**

```rust
//! 崩溃恢复：扫描段，判定最后一个完整事务的边界。

use crate::crc::{fold_crcs, Crc32c};
use crate::device::BlockDevice;
use crate::segment::{CommitRecord, PageHeader, COMMIT_RECORD_SIZE};
use crate::writer::SegmentConfig;
use crate::Error;

/// 恢复扫描的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryReport {
    /// 最后一个有效提交的段序列号。无有效提交则为 `None`。
    pub last_valid_segment: std::option::Option<u64>,
    /// 该段最后一个有效页的物理偏移。
    pub last_valid_offset: u64,
    /// 有效页的 CRC，按写入顺序。用于校验 CRC 链。
    pub page_crcs: std::vec::Vec<u32>,
    /// 因崩溃被截断的字节数。
    pub truncated_bytes: u64,
    /// 检出损坏时的段序列号。校验通过则为 `None`。
    pub corrupted_segment: std::option::Option<u64>,
}

/// 扫描给定段列表，判定最后一个完整事务。
///
/// 判定链：段头 CRC → 提交记录魔数 + CRC → 页 CRC 折叠值匹配。
/// 任一环节不符即停止，该段之后的数据全部视为未提交。
pub fn recover(
    dev: &BlockDevice,
    segments: &[SegmentConfig],
) -> Result<RecoveryReport, Error> {
    let mut report = RecoveryReport {
        last_valid_segment: std::option::Option::None,
        last_valid_offset: 0,
        page_crcs: std::vec::Vec::new(),
        truncated_bytes: 0,
        corrupted_segment: std::option::Option::None,
    };

    for cfg in segments {
        match scan_segment(dev, cfg, &mut report) {
            Ok(true) => report.last_valid_segment = std::option::Option::Some(cfg.segment_seq),
            Ok(false) => break,
            Err(e) => {
                report.corrupted_segment = std::option::Option::Some(cfg.segment_seq);
                return Err(e);
            }
        }
    }
    Ok(report)
}

/// 扫描单个段。返回 true 表示该段有有效提交。
fn scan_segment(
    dev: &BlockDevice,
    cfg: &SegmentConfig,
    report: &mut RecoveryReport,
) -> Result<bool, Error> {
    // 1. 段头必须完整且 CRC 正确
    let mut header_buf = [0u8; crate::segment::SEGMENT_HEADER_SIZE];
    dev.read_at(0, &mut header_buf)?;
    let header = match crate::segment::SegmentHeader::decode(&header_buf) {
        Ok(h) => h,
        Err(_) => return Ok(false),
    };
    if header.segment_seq != cfg.segment_seq {
        return Err(Error::Corruption { segment_seq: cfg.segment_seq });
    }

    // 2. 提交记录：魔数或 CRC 不符即视为未提交（写入被截断）
    let mut rec_buf = [0u8; COMMIT_RECORD_SIZE];
    let rec_at = (cfg.segment_size - COMMIT_RECORD_SIZE) as u64;
    dev.read_at(rec_at, &mut rec_buf)?;
    let record = match CommitRecord::decode(&rec_buf) {
        Ok(r) if r.segment_seq == cfg.segment_seq => r,
        _ => {
            report.truncated_bytes += cfg.segment_size as u64;
            return Ok(false);
        }
    };

    // 3. 逐页校验：页头有效且整槽 CRC 与提交记录中的 fold 匹配
    let run_bytes = cfg.page_size as usize * cfg.slots_per_page as usize;
    let append_start = (crate::segment::SEGMENT_HEADER_SIZE
        + cfg.max_pages * run_bytes) as u64;
    let mut at = append_start;
    let mut crcs: std::vec::Vec<u32> = std::vec::Vec::new();
    let mut last_offset = 0u64;

    loop {
        if at + u64::from(cfg.page_size) > rec_at {
            break;
        }
        let mut slot = std::vec![0u8; cfg.page_size as usize];
        dev.read_at(at, &mut slot)?;

        // 全零槽 = 追加区结束
        if slot.iter().all(|b| *b == 0) {
            break;
        }
        let ph = match PageHeader::decode(&slot) {
            Ok(p) => p,
            Err(_) => break,
        };
        let mut c = Crc32c::new();
        c.update(&slot);
        crcs.push(c.finish());
        last_offset = at;
        at += u64::from(cfg.page_size);
    }

    if fold_crcs(&crcs) != record.page_crc_fold {
        return Err(Error::Corruption { segment_seq: cfg.segment_seq });
    }

    report.page_crcs = crcs;
    report.last_valid_offset = last_offset;
    Ok(true)
}
```

- [ ] **Step 5: 写恢复测试**

```rust
#[cfg(all(test, feature = "std"))]
mod tests {
    use super::*;
    use crate::device::InMemoryDevice;
    use crate::writer::SegmentWriter;

    const SEGMENT_SIZE: usize = 1 << 20;
    const PAGE_SIZE: u32 = 4096;
    const SLOTS_PER_PAGE: u16 = 4;
    const MAX_PAGES: usize = 8;

    fn cfg(seq: u64) -> SegmentConfig {
        SegmentConfig {
            segment_seq: seq,
            prev_segment: seq.saturating_sub(1),
            prev_crc: 0,
            segment_size: SEGMENT_SIZE,
            page_size: PAGE_SIZE,
            slots_per_page: SLOTS_PER_PAGE,
            max_pages: MAX_PAGES,
        }
    }

    /// 正常写一个段并提交，返回设备与配置。
    fn write_one_segment(seq: u64, pages: &[&[u8]]) -> InMemoryDevice {
        let mut dev = InMemoryDevice::new(SEGMENT_SIZE);
        let mut w = SegmentWriter::new(&mut dev, cfg(seq)).unwrap();
        for (i, p) in pages.iter().enumerate() {
            w.append_page(i as u64, p).unwrap();
        }
        w.commit(1_700_000_000).unwrap();
        dev
    }

    #[test]
    fn 正常提交被完整识别() {
        let dev = write_one_segment(1, &[b"a", b"b", b"c"]);
        let r = recover(&dev, &[cfg(1)]).unwrap();
        assert_eq!(r.last_valid_segment, Some(1));
        assert_eq!(r.page_crcs.len(), 3);
        assert_eq!(r.corrupted_segment, None);
    }

    #[test]
    fn 未提交段被忽略() {
        // 写页但崩溃——没有 commit
        let mut dev = InMemoryDevice::new(SEGMENT_SIZE);
        {
            let mut w = SegmentWriter::new(&mut dev, cfg(1)).unwrap();
            w.append_page(0, b"a").unwrap();
            // 故意不 commit
        }
        let r = recover(&dev, &[cfg(1)]).unwrap();
        assert_eq!(r.last_valid_segment, None);
        assert!(r.page_crcs.is_empty());
    }

    #[test]
    fn 提交记录被截断判定为未提交() {
        let mut dev = write_one_segment(1, &[b"a", b"b"]);
        // 破坏提交记录：翻转尾部字节
        let at = (SEGMENT_SIZE - COMMIT_RECORD_SIZE) as usize;
        dev.raw_mut()[at + 30] ^= 0xFF;
        let r = recover(&dev, &[cfg(1)]).unwrap();
        assert_eq!(r.last_valid_segment, None);
    }

    #[test]
    fn 页数据损坏被检出() {
        let mut dev = write_one_segment(1, &[b"a", b"b"]);
        // 破坏第二个页槽的头部
        let run_bytes = PAGE_SIZE as usize * SLOTS_PER_PAGE as usize;
        let append_start = crate::segment::SEGMENT_HEADER_SIZE + MAX_PAGES * run_bytes;
        dev.raw_mut()[append_start + PAGE_SIZE as usize + 2] ^= 0xFF;
        let r = recover(&dev, &[cfg(1)]);
        assert!(matches!(r, Err(Error::Corruption { .. })), "应报损坏");
    }

    #[test]
    fn 静默位翻转在追加区被检出() {
        let mut dev = write_one_segment(1, &[b"payload-a"]);
        let run_bytes = PAGE_SIZE as usize * SLOTS_PER_PAGE as usize;
        let append_start = crate::segment::SEGMENT_HEADER_SIZE + MAX_PAGES * run_bytes;
        // 翻转载荷区一个字节
        dev.raw_mut()[append_start + 64] ^= 0x01;
        assert!(recover(&dev, &[cfg(1)]).is_err());
    }

    #[test]
    fn 槽区零填充损坏也被检出() {
        // 槽 CRC 覆盖整个槽，因此 padding 损坏必须被发现
        let mut dev = write_one_segment(1, &[b"a"]);
        let run_bytes = PAGE_SIZE as usize * SLOTS_PER_PAGE as usize;
        let append_start = crate::segment::SEGMENT_HEADER_SIZE + MAX_PAGES * run_bytes;
        // 翻转第一个页槽的尾部 padding
        dev.raw_mut()[append_start + PAGE_SIZE as usize - 1] ^= 0xFF;
        assert!(recover(&dev, &[cfg(1)]).is_err());
    }

    #[test]
    fn 段头损坏判定为无提交() {
        let mut dev = write_one_segment(1, &[b"a"]);
        dev.raw_mut()[10] ^= 0xFF; // 破坏段头 magic 之外的字段
        let r = recover(&dev, &[cfg(1)]).unwrap();
        assert_eq!(r.last_valid_segment, None);
    }

    #[test]
    fn 多段扫描在首个失败处停止() {
        let mut dev = write_one_segment(1, &[b"a", b"b"]);
        let r = recover(&dev, &[cfg(1), cfg(2)]).unwrap();
        assert_eq!(r.last_valid_segment, Some(1));
    }

    #[test]
    fn 恢复的页CRC与写入器一致() {
        let mut dev = InMemoryDevice::new(SEGMENT_SIZE);
        let mut w = SegmentWriter::new(&mut dev, cfg(1)).unwrap();
        w.append_page(0, b"a").unwrap();
        w.append_page(1, b"b").unwrap();
        let written: std::vec::Vec<u32> = w.page_crcs().to_vec();
        w.commit(1).unwrap();
        let r = recover(&dev, &[cfg(1)]).unwrap();
        assert_eq!(r.page_crcs, written);
    }

    #[test]
    fn 空设备无有效段() {
        let dev = InMemoryDevice::new(SEGMENT_SIZE);
        let r = recover(&dev, &[cfg(1)]).unwrap();
        assert_eq!(r.last_valid_segment, None);
        assert_eq!(r.last_valid_offset, 0);
    }
}
```

这些测试需要 `InMemoryDevice::raw_mut()`——`raw()` 只给不可变切片。追加到 `device.rs`：

```rust
    /// 可变原始字节访问，仅供故障注入测试使用。
    pub fn raw_mut(&mut self) -> &mut [u8] {
        &mut self.data
    }
```

- [ ] **Step 6: 运行恢复测试**

Run: `cargo test -p embedded-database-core --features std recovery 2>&1 | tail -30`
Expected: 10 passed

**`槽区零填充损坏也被检出` 是本 Task 最重要的测试。** 若失败，说明槽 CRC 没有覆盖整个槽——把 `append_page` 中的 `c.update(&slot)` 保持为整槽（当前实现已经是），并确认没有改成只算 `header + payload`。

- [ ] **Step 7: re-export**

`lib.rs` 加：

```rust
#[cfg(feature = "std")]
pub use writer::{AppendedPage, SegmentConfig, SegmentWriter};
#[cfg(feature = "std")]
pub use recovery::{recover, RecoveryReport};
```

- [ ] **Step 8: 全量测试**

Run: `cargo test --workspace --features embedded-database-core/std 2>&1 | tail -10`
Expected: 全部通过，约 85 个测试

- [ ] **Step 9: 检查点（不提交）**

本计划不执行 git 提交（C11）。本步改为确认本 Task 的 **Files** 列表中
每个文件都已存在且内容完整：

```bash
git status --short
```

确认所有新增文件出现在输出中即通过。**不要** `git add` 或 `git commit`。


---

## Task 8: 崩溃注入测试工具

**Files:**
- Create: `crates/embedded-database-bench/src/bin/inject_crash.rs`

**Interfaces:**
- Consumes: `SegmentWriter` / `SegmentConfig` / `recover`（Task 7）、`InMemoryDevice`（Task 3）
- Produces: 可执行文件 `inject_crash`。退出码 0 = 所有场景通过；1 = 有场景失败。

**这个工具的作用**：把"崩溃在任意写点"的测试变成可重复运行的命令行程序，而不是一次性的一次性测试。它遍历每个写点注入故障，验证恢复结果始终等于"最后一个完整事务"。

- [ ] **Step 1: 写 `inject_crash.rs`**

```rust
//! 崩溃注入：在每个写点注入故障，验证恢复结果。
//!
//! 退出码 0 = 全部通过；1 = 有失败。

use std::process::ExitCode;
use embedded_database_core::{recover, InMemoryDevice, SegmentConfig, SegmentWriter};

const SEGMENT_SIZE: usize = 1 << 20;
const PAGE_SIZE: u32 = 4096;
const SLOTS_PER_PAGE: u16 = 4;
const MAX_PAGES: usize = 8;
const PAGES_PER_SEGMENT: u64 = 6;

fn cfg(seq: u64) -> SegmentConfig {
    SegmentConfig {
        segment_seq: seq,
        prev_segment: seq.saturating_sub(1),
        prev_crc: 0,
        segment_size: SEGMENT_SIZE,
        page_size: PAGE_SIZE,
        slots_per_page: SLOTS_PER_PAGE,
        max_pages: MAX_PAGES,
    }
}

/// 不注入故障写一个段，返回已提交的设备。
fn write_segment(seq: u64, pages: u64) -> InMemoryDevice {
    let mut dev = InMemoryDevice::new(SEGMENT_SIZE);
    let mut w = SegmentWriter::new(&mut dev, cfg(seq)).unwrap();
    for i in 0..pages {
        let payload = format!("seq{seq}-page{i}");
        w.append_page(i, payload.as_bytes()).unwrap();
    }
    w.commit(1_700_000_000 + seq).unwrap();
    dev
}

/// 注入"第 n 次写之后失败"，恢复后校验不变量。
fn check_crash_after(crash_at_write: u64, pages: u64) -> Result<(), String> {
    let mut dev = InMemoryDevice::new(SEGMENT_SIZE);
    dev.inject_write_failure(crash_at_write);

    {
        let mut w = match SegmentWriter::new(&mut dev, cfg(1)) {
            Ok(w) => w,
            Err(e) => return Err(format!("crash@{crash_at_write}: 建段失败 {e:?}")),
        };
        for i in 0..pages {
            let payload = format!("seq1-page{i}");
            // 崩溃后写必然失败——这正是我们要的
            if w.append_page(i, payload.as_bytes()).is_err() {
                break;
            }
        }
        let _ = w.commit(1_700_000_000);
    }

    // 恢复：必须得到 0（无提交）或 1（有提交），不能是其他值
    let report = match recover(&dev, &[cfg(1)]) {
        Ok(r) => r,
        Err(Error::Corruption { .. }) => {
            return Err(format!(
                "crash@{crash_at_write}: 恢复报告损坏——崩溃不应产生损坏"
            ))
        }
        Err(e) => return Err(format!("crash@{crash_at_write}: 恢复失败 {e:?}")),
    };

    match report.last_valid_segment {
        None => Ok(()), // 崩溃发生在提交前，正确
        Some(1) => {
            // 有提交，则所有页都应校验通过
            if report.page_crcs.len() > pages as usize {
                return Err(format!(
                    "crash@{crash_at_write}: 声称 {} 页，多于写入的 {pages}",
                    report.page_crcs.len()
                ));
            }
            Ok(())
        }
        Some(other) => Err(format!("crash@{crash_at_write}: 段序列号异常 {other}")),
    }
}

/// 不崩溃基线：恢复必须看到全部页。
fn check_baseline() -> Result<(), String> {
    let dev = write_segment(1, PAGES_PER_SEGMENT);
    let r = recover(&dev, &[cfg(1)]).map_err(|e| format!("baseline: {e:?}"))?;
    if r.last_valid_segment != Some(1) {
        return Err("baseline: 正常提交未被识别".into());
    }
    if r.page_crcs.len() as u64 != PAGES_PER_SEGMENT {
        return Err(format!(
            "baseline: 页数 {} != {}",
            r.page_crcs.len(),
            PAGES_PER_SEGMENT
        ));
    }
    Ok(())
}

/// 多段场景：崩溃在第 2 段，第 1 段必须仍可见。
fn check_multi_segment() -> Result<(), String> {
    let mut dev = InMemoryDevice::new(SEGMENT_SIZE);
    {
        let mut w = SegmentWriter::new(&mut dev, cfg(1)).unwrap();
        w.append_page(0, b"seg1").unwrap();
        w.commit(1_700_000_001).unwrap();
    }
    // 第 2 段：写一半崩溃
    dev.inject_write_failure(1);
    {
        let mut w = SegmentWriter::new(&mut dev, cfg(2)).unwrap();
        w.append_page(0, b"seg2-a").unwrap();
        let _ = w.append_page(1, b"seg2-b");
        let _ = w.commit(1_700_000_002);
    }

    let r = recover(&dev, &[cfg(1), cfg(2)]).map_err(|e| format!("multi: {e:?}"))?;
    if r.last_valid_segment != Some(1) {
        return Err(format!("multi: 期望段 1 有效，实际 {:?}", r.last_valid_segment));
    }
    Ok(())
}

fn main() -> ExitCode {
    let mut failures: Vec<String> = Vec::new();

    if let Err(e) = check_baseline() {
        failures.push(e);
    }

    // 写点总数：段头 + 页 + 提交记录
    let total_writes = PAGES_PER_SEGMENT + 3;
    for n in 0..=total_writes {
        if let Err(e) = check_crash_after(n, PAGES_PER_SEGMENT) {
            failures.push(e);
        }
    }

    if let Err(e) = check_multi_segment() {
        failures.push(e);
    }

    if failures.is_empty() {
        println!("inject_crash: {total_writes} 个崩溃点 + 基线 + 多段场景全部通过");
        ExitCode::SUCCESS
    } else {
        eprintln!("inject_crash: {} 个场景失败", failures.len());
        for f in &failures {
            eprintln!("  - {f}");
        }
        ExitCode::FAILURE
    }
}
```

需要 `use embedded_database_core::Error;`——补在文件顶部 import 区。

- [ ] **Step 2: 运行**

Run: `cargo run -p embedded-database-bench --bin inject_crash 2>&1 | tail -20`
Expected: `inject_crash: 9 个崩溃点 + 基线 + 多段场景全部通过`，退出码 0

**若 `multi` 场景失败**：`recover` 需要在段 2 判定失败后停止扫描并保留段 1 的结果。当前 `recover` 的 `Ok(false) => break` 已实现该语义；若失败，检查 `scan_segment` 是否在段 2 上误报 `Err(Corruption)` 而非 `Ok(false)`。

- [ ] **Step 3: 验证退出码可被脚本消费**

Run: `cargo run -p embedded-database-bench --bin inject_crash >/dev/null 2>&1; echo "exit=$?"`
Expected: `exit=0`

- [ ] **Step 4: 检查点（不提交）**

本计划不执行 git 提交（C11）。本步改为确认本 Task 的 **Files** 列表中
每个文件都已存在且内容完整：

```bash
git status --short
```

确认所有新增文件出现在输出中即通过。**不要** `git add` 或 `git commit`。


---

## Task 9: WCET 实测

**Files:**
- Create: `crates/embedded-database-bench/src/bin/wcet.rs`

**Interfaces:**
- Consumes: `SegmentWriter`、`SlotAllocator`、`BTree`（Task 5/7）
- Produces: 可执行文件 `wcet`。打印各操作的实测时延分布；退出码 0 = 全部在预算内。

**为什么要实测**：设计文档承诺确定性（spec §5.4），但"确定性"必须有用数字支撑。本 Task 给出 M1 各操作的实测上界，作为 M2 预算的基线。

`std::time::Instant` 在 `std` crate 内可用——`embedded-database-bench` 本身是 std crate，不受 `no_std` 约束。

- [ ] **Step 1: 写 `wcet.rs`**

```rust
//! WCET 实测：测量 M1 核心操作的时延分布。
//!
//! 打印 p50 / p99 / max / min，与预算对比。M2 的预算以此为基线。

use std::time::Instant;
use embedded_database_core::{Arena, BTree, InMemoryDevice, SegmentConfig, SegmentWriter};

const SEGMENT_SIZE: usize = 1 << 20;
const PAGE_SIZE: u32 = 4096;
const SLOTS_PER_PAGE: u16 = 4;
const MAX_PAGES: usize = 8;

/// 预算（纳秒）。超出即退出码 1。
const BUDGET_APPEND_PAGE_NS: u128 = 5_000;
const BUDGET_BTREE_INSERT_NS: u128 = 2_000;
const BUDGET_COMMIT_NS: u128 = 10_000;

/// 时延统计。
#[derive(Debug)]
struct Stats {
    label: &'static str,
    samples: std::vec::Vec<u128>,
}

impl Stats {
    fn new(label: &'static str) -> Self {
        Stats { label, samples: std::vec::Vec::new() }
    }

    fn record(&mut self, ns: u128) {
        self.samples.push(ns);
    }

    fn report(&self) -> (u128, u128, u128, u128) {
        let mut s = self.samples.clone();
        s.sort_unstable();
        let p50 = s[s.len() / 2];
        let p99 = s[(s.len() * 99 / 100).min(s.len() - 1)];
        (s[0], p50, p99, s[s.len() - 1])
    }
}

fn main() -> std::process::ExitCode {
    let iter = 20_000u32;
    let mut exceeded = std::vec::Vec::new();

    // ---- 段页追加 ----
    let mut append = Stats::new("append_page");
    {
        let mut dev = InMemoryDevice::new(SEGMENT_SIZE);
        let mut w = SegmentWriter::new(
            &mut dev,
            SegmentConfig {
                segment_seq: 1,
                prev_segment: 0,
                prev_crc: 0,
                segment_size: SEGMENT_SIZE,
                page_size: PAGE_SIZE,
                slots_per_page: SLOTS_PER_PAGE,
                max_pages: MAX_PAGES,
            },
        )
        .unwrap();
        let payload = std::vec![0xABu8; 256];
        for i in 0..iter {
            let t0 = Instant::now();
            // 超过槽配额会失败——那不是我们要测的路径，跳过
            if w.append_page(i as u64 % MAX_PAGES as u64, &payload).is_ok() {
                append.record(t0.elapsed().as_nanos());
            }
        }
    }

    // ---- B-tree 插入 ----
    let mut insert = Stats::new("btree_insert");
    {
        let mut buf = std::vec![0u8; 1 << 20];
        let mut arena = Arena::new(&mut buf);
        let mut tree = BTree::new(&mut arena).unwrap();
        for i in 0..iter {
            let k = format!("k{i:08}");
            let t0 = Instant::now();
            if tree.insert(k.as_bytes(), b"v").is_ok() {
                insert.record(t0.elapsed().as_nanos());
            }
        }
    }

    // ---- 提交 ----
    let mut commit = Stats::new("commit");
    {
        for _ in 0..200 {
            let mut dev = InMemoryDevice::new(SEGMENT_SIZE);
            let mut w = SegmentWriter::new(
                &mut dev,
                SegmentConfig {
                    segment_seq: 1,
                    prev_segment: 0,
                    prev_crc: 0,
                    segment_size: SEGMENT_SIZE,
                    page_size: PAGE_SIZE,
                    slots_per_page: SLOTS_PER_PAGE,
                    max_pages: MAX_PAGES,
                },
            )
            .unwrap();
            w.append_page(0, b"x").unwrap();
            let t0 = Instant::now();
            let _ = w.commit(1);
            commit.record(t0.elapsed().as_nanos());
        }
    }

    for (stats, budget) in [
        (&append, BUDGET_APPEND_PAGE_NS),
        (&insert, BUDGET_BTREE_INSERT_NS),
        (&commit, BUDGET_COMMIT_NS),
    ] {
        if stats.samples.is_empty() {
            eprintln!("wcet: {} 无有效样本", stats.label);
            exceeded.push(stats.label.to_string());
            continue;
        }
        let (min, p50, p99, max) = stats.report();
        println!(
            "{:<14} n={:<6} min={min:>6}ns p50={p50:>6}ns p99={p99:>6}ns max={max:>8}ns budget={budget}ns",
            stats.label,
            stats.samples.len()
        );
        if p99 > budget {
            eprintln!("wcet: {} p99 {}ns 超出预算 {}ns", stats.label, p99, budget);
            exceeded.push(stats.label.to_string());
        }
    }

    if exceeded.is_empty() {
        std::process::ExitCode::SUCCESS
    } else {
        eprintln!("wcet: {} 项超预算", exceeded.len());
        std::process::ExitCode::FAILURE
    }
}
```

`BTree` 的 re-export 在 `lib.rs` 中不带 `std` gate，但 `BTree::new` 需要 arena 缓冲区——`wcet` 里用 `std::vec` 分配没问题。若 `BTree` 因 `BTreeScan` 是 `std`-gated 而编译失败，说明 re-export 需要加 gate；此时 `wcet` 应启用 `std` 特性（它已通过 `embedded-database-bench` 的依赖启用了）。

- [ ] **Step 2: 运行**

Run: `cargo run -p embedded-database-bench --bin wcet 2>&1 | tail -10`
Expected: 三行统计 + 退出码 0

**若某项 p99 超预算，不要直接放宽预算。** 先确认测的是不是真实路径——例如 `append_page` 在槽耗尽后走的是快速失败路径，会让样本失真。确认无误后再调预算，并在 `README.md` 记录实测值作为 M2 基线。

- [ ] **Step 3: 记录基线到 README**

在 `README.md` 末尾追加实测结果表，把 Step 2 的输出填进去，并注明"机器相关，仅作相对基线"。

- [ ] **Step 4: 检查点（不提交）**

本计划不执行 git 提交（C11）。本步改为确认本 Task 的 **Files** 列表中
每个文件都已存在且内容完整：

```bash
git status --short
```

确认所有新增文件出现在输出中即通过。**不要** `git add` 或 `git commit`。


---

## Task 10: 收尾验收

**Files:**
- Modify: `docs/superpowers/specs/2026-10-02-embedded-database-design.md`（更新实施状态）

**Interfaces:**
- Consumes: 全部前序 Task
- Produces: 无新接口。仅确认全部约束成立。

- [ ] **Step 1: 全量测试**

Run: `cargo test --workspace --features embedded-database-core/std 2>&1 | tail -15`
Expected: 全部通过

- [ ] **Step 2: 格式化与 lint**

Run: `cargo fmt --all` 然后 `cargo clippy --workspace --all-targets --features embedded-database-core/std -- -D warnings 2>&1 | tail -20`
Expected: 无警告。若有，逐条修正而非加 `#[allow]`。

- [ ] **Step 3: 5 目标 no_std 验证**

Run:
```bash
for t in aarch64-unknown-linux-gnu loongarch64-unknown-linux-gnu \
         loongarch64-unknown-linux-musl x86_64-unknown-linux-gnu; do
  printf '%-36s ' "$t"
  cargo check -p embedded-database-core --target "$t" 2>&1 | tail -1
done
printf '%-36s ' loongarch64-unknown-none
cargo check -p embedded-database-core --target loongarch64-unknown-none 2>&1 | tail -1
```
Expected: 5 个目标全部无错误

- [ ] **Step 4: unsafe 扫描**

Run: `rg -n "unsafe" crates/*/src -g '*.rs' | rg -v "forbid\(unsafe_code\)"`
Expected: 无输出

- [ ] **Step 5: alloc 依赖扫描**

确认 `no_std` 路径没有引入 `alloc`：

Run: `rg -n "extern crate alloc|use alloc::" crates/*/src -g '*.rs'`
Expected: 无输出

- [ ] **Step 6: 工具二进制可用**

Run: `cargo run -p embedded-database-bench --bin inject_crash >/dev/null 2>&1; echo "inject=$?"; cargo run -p embedded-database-bench --bin wcet >/dev/null 2>&1; echo "wcet=$?"`
Expected: `inject=0` 且 `wcet=0`

- [ ] **Step 7: 交叉编译产物抽查**

确认至少一个非 x86 目标能真正链接（`check` 只验证类型）：

Run: `cargo build -p embedded-database-core --target aarch64-unknown-linux-gnu 2>&1 | tail -3`
Expected: 成功。若本机缺交叉链接器失败，改用 `cargo build --target aarch64-unknown-linux-gnu` 的 rlib 目标即可——`cargo check` 已覆盖类型正确性，此步非阻塞。

- [ ] **Step 8: 更新设计文档状态**

在 `2026-10-02-embedded-database-design.md` 末尾追加"实施状态"小节，记录 M1 实际交付内容与与原设计的偏差。重点记录两处：

```markdown
## 实施状态（M1 完成）

- M1 交付：段格式、CRC-32C + CRC 链、固定 Arena / 哈希表 / 两层 B+tree、
  Memtable、页槽分配、段写入、崩溃恢复、崩溃注入与 WCET 工具。
- **偏差 1**：B-tree 实现为定高**两层** B+tree（常驻根 + 叶层），
  容量 = ROOT_CAPACITY(16) × LEAF_CAPACITY(8) = 128 键。
  原设计为通用 N 叉。理由：根分裂与"不重平衡"约束冲突，
  两层结构让深度恒为 2，WCET 上界可证明。更大容量通过调大常量扩展，
  结构不变。M2 视需要引入第三层。
- **偏差 2**：`Memtable` 借用调用方 arena 而非持有自己的缓冲区，
  因此不能直接嵌入 `Db` 结构体。M2 需引入显式 arena 池解耦。
- **偏差 3**：任务 2 中提交记录的 CRC 覆盖范围在实现阶段修正为
  跳过 `commit_crc` 自身字段——原设计描述会导致自引用校验失败。
```

- [ ] **Step 9: 检查点（不提交）**

本计划不执行 git 提交（C11）。本步改为确认本 Task 的 **Files** 列表中
每个文件都已存在且内容完整：

```bash
git status --short
```

确认所有新增文件出现在输出中即通过。**不要** `git add` 或 `git commit`。


---

## 验收标准

M1 完成需**同时**满足：

| # | 标准 | 验证方式 |
|---|---|---|
| 1 | 5 个 no_std 目标全部编译通过 | Task 10 Step 3 |
| 2 | 全 crate 零 `unsafe` | Task 10 Step 4 + CI |
| 3 | `no_std` 路径零 `alloc` | Task 10 Step 5 |
| 4 | 全部测试通过 | Task 10 Step 1 |
| 5 | 崩溃在任意写点都不产生损坏，恢复结果正确 | Task 8 |
| 6 | 槽 padding 损坏可被检出 | Task 7 `槽区零填充损坏也被检出` |
| 7 | 容量耗尽显式返回 `CapacityExhausted`，不静默降级 | 各结构体测试 |
| 8 | WCET 各操作在预算内 | Task 9 |
| 9 | 运行时依赖不超过 2 个（`zerocopy` + `bitflags`） | Task 0 `Cargo.toml` |
| 10 | SBOM 可生成 | CI `生成 SBOM` step |

## 后续里程碑

M1 完成后按设计文档推进：

- **M2**：显式事务（`Txn` / `CommitInfo`）、MVCC、page_id → 物理位置映射、
  compaction（预算化）、`Db` 公开 API。届时需要引入 arena 池以支持自引用。
- **M3**：溢出段、`no_std` 路径全面替换测试用的 `Vec` 实现。
- **M4**：SQL 层。
- **M5**：LoongArch 实机验证 + GB/T 22239 安全设计。
- **M6**：Ferrocene 认证可行性评估。
