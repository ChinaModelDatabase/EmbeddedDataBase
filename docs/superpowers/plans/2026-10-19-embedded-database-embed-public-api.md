# Spec 3a：embedded-database-embed 公开 API 门面 — 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让 `embedded-database-embed` 从 10 行 `Placeholder` 变成 `embedded-database-core` 之上稳定的 `Db` / `Txn` 纯 Rust 门面，并让门禁命令真正覆盖它。

**Architecture:** 与 `embedded-database-kv` 同构——`#![no_std]` 外壳 + `#[cfg(any(test, feature = "std"))] mod imp` + `Placeholder` 兜底 + `std = ["embedded-database-core/std"]`。门面类型 `Db<'d>` / `Txn<'db,'d>` 各自内含 core 同名类型，方法**显式转发**（不 `pub use` core 的 `Db`/`Txn`），参数/返回类型与错误直接复用 core 并再导出。测试沿用仓库惯例（`src/` 内 `#[cfg(test)] mod tests`），持久化用例经 `embedded-database-os::FileDevice` 走真文件。

**Tech Stack:** Rust 1.81（`rust-toolchain.toml` pin）、`embedded-database-core`（`std` 特性）、`embedded-database-os`（dev-dependency）、cargo workspace。

**设计文档:** `docs/superpowers/specs/2026-10-19-embedded-database-embed-public-api-design.md`（已批准）。

## Global Constraints

- **每个新 shell 先执行**：`export PATH="/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH"`。
- **禁止改动 `crates/embedded-database-core/`**（零 core 改动）。embed 只使用 core 既有公开 API。
- **禁止新增第三方依赖**：新增的 dev-dependency 只有 `embedded-database-os`（workspace 成员）。`Cargo.lock` 的第三方集合不得变化（Spec 2 的审计范围）。
- **禁止 `unsafe`**：全仓 `#![forbid(unsafe_code)]`；embed 不得出现任何 `unsafe` 标记。
- **no_std 边界**：`crates/embedded-database-embed/src/lib.rs` 保持 `#![no_std]`；门面仅在 `#[cfg(any(test, feature = "std"))]` 下存在；`#[cfg(not(any(test, feature = "std")))]` 路径只导出 `Placeholder`。CI `nostd` job 会对 5 个目标跑 `cargo check -p embedded-database-core -p embedded-database-kv -p embedded-database-embed --target <t>`，此路径必须一直可编译。
- **测试位置**：仓库**没有** `crates/*/tests/` 目录；测试写在 `crates/embedded-database-embed/src/lib.rs` 内的 `#[cfg(test)] mod tests`。原因：`cargo test --workspace --features embedded-database-core/std` 只开 core 的 `std`，`tests/` 集成测试（`cfg(test)` 为假）看不到门面。
- **门禁命令（Task 5 之后即为最终形态）**：
  - `cargo fmt --all --check`（干净）
  - `cargo clippy --workspace --all-targets --features embedded-database-core/std,embedded-database-embed/std -- -D warnings`（干净）
  - `cargo test --workspace --features embedded-database-core/std,embedded-database-embed/std`（**≥ 301 + 新增 embed 用例**；原 301 项（core 216 + kv 8 + os 5 + sql 72）不得删除或改弱）
  - 另加不变式：`cargo test -p embedded-database-embed`（**不带** `std` 特性）也必须通过——防止门面被写成"只能在带特性时测试"。
- **文档与提交语言**：中文。提交信息风格 `<type>: <描述>`（`type` ∈ feat/chore/ci/test/docs/fix/docs），一个逻辑单元一提交，提交前 `git status` 应干净。
- **不改历史 plans**：`docs/superpowers/plans/2026-10-02-*.md` 记录的是当时状态，本次不改（理由写入账本）。

---

### Task 1: 特性门控与依赖接线

**Files:**
- Modify: `crates/embedded-database-embed/Cargo.toml`（全文替换）
- Modify: `crates/embedded-database-embed/src/lib.rs`（全文替换为骨架版）

**Interfaces:**
- Consumes: 无（起点是 10 行 `Placeholder`）。
- Produces: `embedded-database-embed` 具备 `std` 特性（= `embedded-database-core/std`）与 dev-dependencies（`embedded-database-core` +`std`、`embedded-database-os`）；lib.rs 具 `imp`（空）/ `Placeholder` 结构。Task 2 往 `imp` 里填内容**并加入 `pub use imp::*;` 重导出**，Task 3 往 `mod tests` 里加用例。

- [ ] **Step 1: 替换 Cargo.toml**

把 `crates/embedded-database-embed/Cargo.toml` 全文替换为（与 `crates/embedded-database-kv/Cargo.toml` 同构）：

```toml
[package]
name = "embedded-database-embed"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true
description = "embedded-database 公开嵌入 API"

[features]
default = []
# std 路径：启用 embedded-database-core 的 std（Db/Txn 等导出），提供完整门面实现。
# no_std 路径（default）：仅导出 Placeholder，保证裸机目标可编译（C1）。
std = ["embedded-database-core/std"]

[dependencies]
embedded-database-core = { workspace = true, default-features = false }

[dev-dependencies]
# 测试在 host 上编译，需要 embedded-database-core/std 的 Db/InMemoryDevice，
# 以及真文件后端 embedded-database-os::FileDevice（持久化用例）。
# resolver v2：构建测试时 dev-dependency 特性与普通依赖统一。
embedded-database-core = { workspace = true, features = ["std"] }
embedded-database-os = { workspace = true }

[lints.rust]
unsafe_code = "forbid"
```

- [ ] **Step 2: 替换 lib.rs 为骨架版**

把 `crates/embedded-database-embed/src/lib.rs` 全文替换为（`imp` 目前为空，Task 2 填充）：

```rust
#![no_std]
#![forbid(unsafe_code)]

//! embedded-database 公开嵌入 API（`Db` / `Txn` 门面）。
//!
//! # 定位
//!
//! 本 crate 是 `embedded-database-core` 存储引擎面向外部使用者的公开面：调用者只需依赖
//! `embedded-database-embed`，不必接触 core 的内部类型与观测接口。实现是**显式转发**
//! （不 `pub use` core 的 `Db`/`Txn`），因此 core 的内部演进不会自动改变本 crate
//! 的公开面。
//!
//! # 特性
//!
//! - `std`：启用完整门面（`Db` / `Txn`），依赖 `embedded-database-core/std`。
//! - 默认（no_std）：仅导出 `Placeholder`，保证裸机目标可编译（C1）。
//!
//! # 明确不做
//!
//! `delete`/墓碑、范围迭代（`Cursor` 不外泄）、C/FFI ABI、真正的 no_std 可用
//! （core 的 `Db`/`Txn` 仍受 `std` 门控）、core 的内部观测面
//! （`page_map_entries`/`read_payload`/`try_lock_writer` 等）与 M2 占位实现
//! （`Db::compact`）。

// 测试与 std 路径需要 std（Vec、format! 等）。
#[cfg(any(test, feature = "std"))]
extern crate std;

/// std 路径：完整门面实现。
#[cfg(any(test, feature = "std"))]
mod imp {}

/// no_std 路径占位类型。完整门面需要 `std` 特性。
#[cfg(not(any(test, feature = "std")))]
#[derive(Debug)]
pub struct Placeholder;
```

（此处**不**写 `pub use imp::*;`：对空模块做 glob 重导出会触发 rustc 1.81 的 `unused_imports` 警告，使 `clippy -- -D warnings` 失败。该行在 Task 2 随 `imp` 内容一并加入。）

- [ ] **Step 3: 验证双向可编译**

```bash
cargo check -p embedded-database-embed
cargo check -p embedded-database-embed --features std
```

Expected: 两条都 `Finished`（无 error、无 warning）。第一条走 `Placeholder` 路径，第二条走 `imp`（当前为空）。

- [ ] **Step 4: 验证 no_std 目标可编译**

```bash
for t in aarch64-unknown-linux-gnu loongarch64-unknown-linux-gnu loongarch64-unknown-linux-musl loongarch64-unknown-none x86_64-unknown-linux-gnu; do cargo check -p embedded-database-embed --target "$t" || exit 1; done
```

Expected: 5 个目标全部 `Finished`（与 CI `nostd` job 同款验证）。

- [ ] **Step 5: 验证命名空间特性语法可用（Task 5 依赖它）**

```bash
cargo check --workspace --features embedded-database-core/std,embedded-database-embed/std
```

Expected: `Finished`（virtual workspace 根下 `embedded-database-embed/std` 命名空间语法被接受；Task 1 之前此命令报 `none of the selected packages contains these features: embedded-database-embed/std`，实测）。

- [ ] **Step 6: 提交**

```bash
git add crates/embedded-database-embed/Cargo.toml crates/embedded-database-embed/src/lib.rs
git commit -m "feat: embedded-database-embed 特性门控与依赖接线（std = embedded-database-core/std）"
```

---

### Task 2: `Db` / `Txn` 门面类型与单元测试

**Files:**
- Modify: `crates/embedded-database-embed/src/lib.rs`（在 Task 1 骨架上填充 `imp` 与 `mod tests`）

**Interfaces:**
- Consumes: Task 1 的 `std` 特性与 dev-dependencies。
- Produces: 公开面 `Db::open/config/active_segment/stats/put/get/flush/compact_with_budget/begin`、`Txn::deadline/put/get/commit`；再导出 `BlockDevice, CommitInfo, CompactionBudget, CompactReport, Config, DbStats, Deadline, EncryptionKey, Error, InMemoryDevice, IntegrityLevel, Result, Sm4Key, TxnOptions`（14 个类型，`Sm4Key` 为 `EncryptionKey.sm4` 的字段类型）。Task 3/4 在此之上加测试与文档。

- [ ] **Step 1: 写入 `imp` 模块（门面类型 + 再导出）**

将 `crates/embedded-database-embed/src/lib.rs` 中 `mod imp {}` **连同其上一行的 `#[cfg(any(test, feature = "std"))]` 属性**一起替换为下面的模块内容，并在**文件末尾**（`Placeholder` 定义之后）追加重导出两行——两者合计构成本步的完整改动：

```rust
#[cfg(any(test, feature = "std"))]
pub use imp::*;
```

模块内容：

```rust
#[cfg(any(test, feature = "std"))]
mod imp {
    use core::fmt;
    use std::vec::Vec;

    // 参数/返回类型与错误：直接复用 core，不引入 embed 自有类型。
    // EncryptionKey 是 Config.encryption_key 的字段类型，必须一并导出，
    // 否则调用者无法构造启用加密的配置。Sm4Key 是 EncryptionKey.sm4 的
    // 字段类型，core 只给了结构体定义（无 Default、无构造函数），
    // 不导出则调用者连 EncryptionKey 都无法拼出来，故随 EncryptionKey 一并导出。
    pub use embedded_database_core::{
        BlockDevice, CommitInfo, CompactReport, CompactionBudget, Config, DbStats, Deadline,
        EncryptionKey, Error, InMemoryDevice, IntegrityLevel, Result, Sm4Key, TxnOptions,
    };

    /// 嵌入式数据库句柄（公开门面，包装 `embedded_database_core::Db`）。
    pub struct Db<'d> {
        inner: embedded_database_core::Db<'d>,
    }

    impl<'d> fmt::Debug for Db<'d> {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_struct("Db").finish_non_exhaustive()
        }
    }

    impl<'d> Db<'d> {
        /// 打开数据库。冷启动时扫描已有段并重建映射表。
        pub fn open(dev: &'d mut dyn BlockDevice, cfg: Config) -> Result<Db<'d>> {
            Ok(Db {
                inner: embedded_database_core::Db::open(dev, cfg)?,
            })
        }

        /// 当前配置（打开时确定，运行期不变）。
        pub fn config(&self) -> &Config {
            self.inner.config()
        }

        /// 下一个写入将使用的段号。
        pub fn active_segment(&self) -> u64 {
            self.inner.active_segment()
        }

        /// 计数快照（commit/read/write/error 计数与最近一次错误）。
        pub fn stats(&self) -> DbStats {
            self.inner.stats()
        }

        /// 便捷写入：`begin` → `put` → `commit` → `flush`。
        ///
        /// 返回 `flush` 的提交结果（真正落盘的段号与字节数）。
        /// key 含 `0xFF` 时返回 `Err(Error::InvalidArgument)`：payload 以首个
        /// `0xFF` 分隔 key/value。批量写入请自行 `begin` + 多次 `Txn::put` +
        /// `flush`——本方法每次调用都完整走一遍提交与刷盘（写放大）。
        pub fn put(&mut self, k: &[u8], v: &[u8]) -> Result<CommitInfo> {
            let mut txn = self.begin(TxnOptions::default())?;
            txn.put(k, v)?;
            txn.commit()?;
            self.flush()
        }

        /// 读取键值。未命中返回 `Ok(None)`。
        ///
        /// 只读**已 flush** 的提交（`Txn::commit` 仅写入 `pending_writes`）；
        /// `Db::put` 自带 flush，故「put 后可 get」。
        /// **不带 deadline**：需要超时语义的读请用
        /// `begin(TxnOptions { deadline })` 再调 `Txn::get`。
        ///
        /// 实现说明：`payload` 解析（`key ++ 0xFF ++ value`）与 core `Txn::get`、
        /// `embedded-database-kv` 同构——core 已冻结，且 `&self` 读路径不能取写者锁，
        /// 故就地复刻而非改 core 抽象。
        pub fn get(&self, k: &[u8]) -> Result<Option<Vec<u8>>> {
            if let Some(page_id) = self.inner.lookup_key(k) {
                let payload = self.inner.read_payload(page_id)?;
                // payload 格式：key ++ 0xFF ++ value
                if let Some(pos) = payload.iter().position(|&b| b == 0xFF) {
                    if &payload[..pos] == k {
                        return Ok(Some(payload[pos + 1..].to_vec()));
                    }
                }
            }
            Ok(None)
        }

        /// 把 `pending_writes` 落盘到活跃段（同段追加或开新段）。
        pub fn flush(&mut self) -> Result<CommitInfo> {
            self.inner.flush()
        }

        /// 预算化段合并。
        ///
        /// Deadline 契约（C7）：过期且 `budget.interruptible = true` → 返回
        /// `interrupted: true` 的报告；过期且 `interruptible = false` →
        /// `Err(Error::DeadlineExceeded)`；未过期 → 正常执行。
        pub fn compact_with_budget(
            &mut self,
            budget: &CompactionBudget,
            deadline: Option<Deadline>,
        ) -> Result<CompactReport> {
            self.inner.compact_with_budget(budget, deadline)
        }

        /// 开启事务。写者锁被占用时返回 `Err(Error::Busy)`（绝不等待）。
        pub fn begin(&mut self, opts: TxnOptions) -> Result<Txn<'_, 'd>> {
            Ok(Txn {
                inner: self.inner.begin(opts)?,
            })
        }
    }

    /// 事务句柄（公开门面，包装 `embedded_database_core::Txn`）。
    ///
    /// 未 `commit` 即 drop：变更丢弃，写者锁由 core 的 `Txn::Drop` 释放
    /// （本类型不实现 `Drop`，以保住 `commit(self)` 的字段移动）。
    pub struct Txn<'db, 'd> {
        inner: embedded_database_core::Txn<'db, 'd>,
    }

    impl<'db, 'd> fmt::Debug for Txn<'db, 'd> {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_struct("Txn").finish_non_exhaustive()
        }
    }

    impl<'db, 'd> Txn<'db, 'd> {
        /// 事务 deadline（来自 `TxnOptions::deadline`）。
        pub fn deadline(&self) -> Option<Deadline> {
            self.inner.deadline()
        }

        /// 写入键值对。key 含 `0xFF` 或 deadline 已过期时返回错误。
        pub fn put(&mut self, k: &[u8], v: &[u8]) -> Result<()> {
            self.inner.put(k, v)
        }

        /// 读取键值：先查本事务变更（last-wins），再查已提交数据。
        pub fn get(&self, k: &[u8]) -> Result<Option<Vec<u8>>> {
            self.inner.get(k)
        }

        /// 提交事务（变更进入 `pending_writes`，需 `Db::flush` 才落盘）。
        pub fn commit(self) -> Result<CommitInfo> {
            self.inner.commit()
        }
    }
}
```

- [ ] **Step 2: 加入单元测试模块**

在 `crates/embedded-database-embed/src/lib.rs` **文件末尾**追加：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 门面_基本读写往返() {
        let mut dev = InMemoryDevice::new(4 << 20);
        let mut db = Db::open(&mut dev, Config::default()).unwrap();
        let info = db.put(b"key1", b"value1").unwrap();
        assert!(info.segment_seq >= 1);
        assert_eq!(db.get(b"key1").unwrap(), Some(b"value1".to_vec()));
    }

    #[test]
    fn 门面_未写入键返回_none() {
        let mut dev = InMemoryDevice::new(4 << 20);
        let mut db = Db::open(&mut dev, Config::default()).unwrap();
        db.put(b"present", b"v").unwrap();
        assert_eq!(db.get(b"missing").unwrap(), None);
    }

    #[test]
    fn 门面_put_含0xff的key被拒() {
        let mut dev = InMemoryDevice::new(4 << 20);
        let mut db = Db::open(&mut dev, Config::default()).unwrap();
        assert_eq!(
            db.put(&[0x01, 0xFF, 0x02], b"value"),
            Err(Error::InvalidArgument)
        );
    }

    #[test]
    fn 门面_提交后flush前不可见_flush后可见() {
        let mut dev = InMemoryDevice::new(4 << 20);
        let mut db = Db::open(&mut dev, Config::default()).unwrap();
        let mut txn = db.begin(TxnOptions::default()).unwrap();
        txn.put(b"k", b"v").unwrap();
        txn.commit().unwrap();
        // commit 只进 pending_writes，未 flush 前 key_map 里没有该键
        assert_eq!(db.get(b"k").unwrap(), None);
        db.flush().unwrap();
        assert_eq!(db.get(b"k").unwrap(), Some(b"v".to_vec()));
    }

    #[test]
    fn 门面_未提交事务drop后变更丢弃且锁释放() {
        let mut dev = InMemoryDevice::new(4 << 20);
        let mut db = Db::open(&mut dev, Config::default()).unwrap();
        {
            let mut txn = db.begin(TxnOptions::default()).unwrap();
            txn.put(b"k", b"v").unwrap();
            // 不 commit，直接 drop
        }
        // 能再次 begin ⇒ core 的 Txn::Drop 已释放写者锁
        let txn2 = db.begin(TxnOptions::default()).unwrap();
        drop(txn2);
        assert_eq!(db.get(b"k").unwrap(), None);
    }

    #[test]
    fn 门面_compact_with_budget后数据仍可回读() {
        let mut dev = InMemoryDevice::new(8 << 20);
        let mut db = Db::open(&mut dev, Config::default()).unwrap();
        db.put(b"k1", b"v1").unwrap();
        db.put(b"k2", b"v2").unwrap();
        let report = db
            .compact_with_budget(&CompactionBudget::default(), None)
            .unwrap();
        assert!(!report.interrupted);
        assert_eq!(db.get(b"k1").unwrap(), Some(b"v1".to_vec()));
        assert_eq!(db.get(b"k2").unwrap(), Some(b"v2".to_vec()));
    }

    #[test]
    fn 门面_stats随操作增长() {
        let mut dev = InMemoryDevice::new(4 << 20);
        let mut db = Db::open(&mut dev, Config::default()).unwrap();
        assert_eq!(db.stats().commit_count, 0);
        assert_eq!(db.stats().write_count, 0);
        db.put(b"k", b"v").unwrap();
        assert_eq!(db.stats().commit_count, 1);
        assert_eq!(db.stats().write_count, 1);
    }

    #[test]
    fn 门面_过期deadline使put返回_deadlineexceeded() {
        let mut dev = InMemoryDevice::new(4 << 20);
        let mut db = Db::open(&mut dev, Config::default()).unwrap();
        // Deadline(0) 是确定性已过期值（`now >= 0` 恒真，见 core types.rs:20-26）
        let opts = TxnOptions {
            deadline: Some(Deadline(0)),
        };
        let mut txn = db.begin(opts).unwrap();
        assert_eq!(txn.deadline(), Some(Deadline(0)));
        assert_eq!(txn.put(b"k", b"v"), Err(Error::DeadlineExceeded));
    }

    #[test]
    fn 门面_配置与活跃段可访问_加密配置仅凭再导出类型构造() {
        // Sm4Key 无 Default、无构造函数，但 `bytes` 字段为 pub：
        // 仅用 embedded_database_embed 再导出的类型即可拼出 EncryptionKey → 加密 Config。
        let cfg = Config {
            encryption_key: Some(EncryptionKey {
                sm4: Sm4Key { bytes: [0x42; 16] },
                hmac: [0x7F; 32],
            }),
            ..Config::default()
        };
        let mut dev = InMemoryDevice::new(4 << 20);
        let mut db = Db::open(&mut dev, cfg).unwrap();
        assert_eq!(db.config().page_size, Config::default().page_size);
        assert!(db.active_segment() >= 1);
        db.put(b"enc", b"v").unwrap();
        assert_eq!(db.get(b"enc").unwrap(), Some(b"v".to_vec()));
    }
}
```

- [ ] **Step 3: 跑测试（新用例）**

```bash
cargo test -p embedded-database-embed --features std
```

Expected: `test result: ok. 9 passed; 0 failed`（9 个用例）；末尾 `Doc-tests embedded_database_embed ... 0 passed`（示例是 Task 4 的事）。

- [ ] **Step 4: 跑无特性测试路径（不变式）**

```bash
cargo test -p embedded-database-embed
```

Expected: 同样 `9 passed`（`cfg(test)` 使 `imp` 可见，`std` 经 dev-dependency 统一打开）。

- [ ] **Step 5: 核对公开面逐项与设计一致**

```bash
rg -n "^\s+pub (fn|struct|use)" crates/embedded-database-embed/src/lib.rs
```

Expected: `Db` / `Txn` 的 `pub fn` 集合与设计 §3.2 一致（`open/config/active_segment/stats/put/get/flush/compact_with_budget/begin`、`deadline/put/get/commit`）；`pub use embedded_database_core::{...}` 一行列出 14 个类型（`BlockDevice, CommitInfo, CompactionBudget, CompactReport, Config, DbStats, Deadline, EncryptionKey, Error, InMemoryDevice, IntegrityLevel, Result, Sm4Key, TxnOptions`，其中 `Sm4Key` 是 `EncryptionKey.sm4` 的字段类型）；**没有** `pub use embedded_database_core::Db` 或 `pub use embedded_database_core::Txn` 之类的别名。

- [ ] **Step 6: 生成文档，确认无警告（含无 broken intra-doc links）**

```bash
cargo doc -p embedded-database-embed --features std --no-deps 2>&1 | tail -5
```

Expected: `Generated .../embedded_database_embed/index.html` 且无 `warning:`。

- [ ] **Step 7: 提交**

```bash
git add crates/embedded-database-embed/src/lib.rs
git commit -m "feat: embedded-database-embed 公开 Db/Txn 门面与单元测试"
```

---

### Task 3: 文件后端持久化测试（端到端）

**Files:**
- Modify: `crates/embedded-database-embed/src/lib.rs`（在 Task 2 的 `mod tests` 内追加 1 个用例 + 辅助函数）

**Interfaces:**
- Consumes: Task 2 的 `Db` / `Config`；Task 1 的 dev-dependency `embedded-database-os`。
- Produces: 通过公开门面 + 真文件后端验证「写入 → 关闭 → 重新打开 → 回读」的端到端证据。

- [ ] **Step 1: 加入持久化用例**

把 `crates/embedded-database-embed/src/lib.rs` 中 `mod tests {` 的下一行 `use super::*;` 替换为：

```rust
    use super::*;
    use std::format;
    use std::path::PathBuf;
    use embedded_database_os::FileDevice;
```

并在 `mod tests` 内（最后一个用例之后）追加：

```rust
    /// 每个用例独立临时文件，避免并行测试互相污染（与 os/file.rs 的 temp_path 同款）。
    fn 临时路径() -> PathBuf {
        static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "embedded-database-embed-test-{}-{}.db",
            std::process::id(),
            n
        ))
    }

    #[test]
    fn 门面_文件后端持久化重开后回读() {
        let path = 临时路径();
        // 设备容量 4 MiB = 默认 Config 的 max_segments(4) × segment_size(1 MiB)，
        // 保证段槽位不会越界，测的是持久化而不是边界。
        {
            let mut dev = FileDevice::open(&path, 4 << 20).unwrap();
            let mut db = Db::open(&mut dev, Config::default()).unwrap();
            db.put(b"persisted", b"value").unwrap();
            // db 在此作用域结束时先于 dev drop（逆序），借用关系满足
        }

        let mut dev = FileDevice::open(&path, 4 << 20).unwrap();
        let db = Db::open(&mut dev, Config::default()).unwrap();
        assert_eq!(db.get(b"persisted").unwrap(), Some(b"value".to_vec()));

        drop(db);
        drop(dev);
        let _ = std::fs::remove_file(&path);
    }
```

- [ ] **Step 2: 跑该用例**

```bash
cargo test -p embedded-database-embed --features std 门面_文件后端持久化重开后回读
```

Expected: `test result: ok. 1 passed; 0 failed`。

- [ ] **Step 3: 跑全部 embed 测试**

```bash
cargo test -p embedded-database-embed --features std
```

Expected: `10 passed; 0 failed`。

- [ ] **Step 4: 确认无残留临时文件**

```bash
ls /tmp/embedded-database-embed-test-*.db 2>/dev/null | wc -l
```

Expected: `0`（macOS 上 `std::env::temp_dir()` 可能是 `/var/folders/...`；若 `ls` 找不到目录而返回 0 亦可）。若有残留，说明用例没走到清理那行——回看 Step 1 的 `drop` + `remove_file` 是否齐全。

- [ ] **Step 5: 提交**

```bash
git add crates/embedded-database-embed/src/lib.rs
git commit -m "test: embed 文件后端持久化端到端用例（FileDevice 重开回读）"
```

---

### Task 4: 公开面文档与可执行示例（doctest）

**Files:**
- Modify: `crates/embedded-database-embed/src/lib.rs`（crate 级文档补稳定面承诺；`Db` / `Txn` 各加 1 个 `# 示例` doctest）

**Interfaces:**
- Consumes: Task 2 的 `Db` / `Txn` / 再导出类型。
- Produces: crate 级稳定面承诺 + 2 个真实执行的 doctest（`Doc-tests embedded_database_embed ... 2 passed`）。

- [ ] **Step 1: crate 级文档补「稳定面承诺」小节**

把 `crates/embedded-database-embed/src/lib.rs` 中这一段：

```rust
//! # 明确不做
```

替换为：

```rust
//! # 稳定面承诺
//!
//! 稳定面 = [`Db`] / [`Txn`] 及其方法签名，以及本 crate 再导出的类型
//! （[`BlockDevice`]、[`Config`]、[`TxnOptions`]、[`Deadline`]、[`Error`] 等）。
//! 对稳定面的不兼容变更必须提升 minor/major 版本并在提交信息中说明
//! （仓库当前无 `CHANGELOG.md`；在引入 CHANGELOG 前以版本号 + 提交信息承载）。
//! `Placeholder` 与内部模块 `imp` 不在承诺范围内。
//!
//! # 明确不做
```

- [ ] **Step 2: 给 `Db` 加 doctest 示例**

把 `crates/embedded-database-embed/src/lib.rs` 中：

```rust
    /// 嵌入式数据库句柄（公开门面，包装 `embedded_database_core::Db`）。
    pub struct Db<'d> {
```

替换为：

```rust
    /// 嵌入式数据库句柄（公开门面，包装 `embedded_database_core::Db`）。
    ///
    /// # 示例
    ///
    /// ```
    /// use embedded_database_embed::{Config, Db, InMemoryDevice};
    ///
    /// let mut dev = InMemoryDevice::new(4 << 20);
    /// let mut db = Db::open(&mut dev, Config::default()).unwrap();
    /// db.put(b"key", b"value").unwrap();
    /// assert_eq!(db.get(b"key").unwrap(), Some(b"value".to_vec()));
    /// ```
    pub struct Db<'d> {
```

- [ ] **Step 3: 给 `Txn` 加 doctest 示例**

把 `crates/embedded-database-embed/src/lib.rs` 中：

```rust
    /// 事务句柄（公开门面，包装 `embedded_database_core::Txn`）。
```

替换为：

```rust
    /// 事务句柄（公开门面，包装 `embedded_database_core::Txn`）。
    ///
    /// # 示例
    ///
    /// ```
    /// use embedded_database_embed::{Config, Db, InMemoryDevice, TxnOptions};
    ///
    /// let mut dev = InMemoryDevice::new(4 << 20);
    /// let mut db = Db::open(&mut dev, Config::default()).unwrap();
    /// let mut txn = db.begin(TxnOptions::default()).unwrap();
    /// txn.put(b"k1", b"v1").unwrap();
    /// txn.put(b"k2", b"v2").unwrap();
    /// txn.commit().unwrap();
    /// db.flush().unwrap(); // commit 只进 pending_writes，flush 才落盘
    /// assert_eq!(db.get(b"k1").unwrap(), Some(b"v1".to_vec()));
    /// ```
```

- [ ] **Step 4: 验证 doctest 在带特性时真实执行**

```bash
cargo test -p embedded-database-embed --features std 2>&1 | rg "Doc-tests|test result:" | tail -6
```

Expected: `Doc-tests embedded_database_embed` 之后 `test result: ok. 2 passed; 0 failed`；同时前面的 unittest 行为 `10 passed`。
若 doctest 为 `0 passed`：说明 rustdoc 没收集到 `imp`（私有模块）内的示例——**不要**降级为 `no_run`/`ignore`，改为在报告中如实记录并上报控制者。

- [ ] **Step 5: 验证无特性时 doctest 不报错**

```bash
cargo test -p embedded-database-embed 2>&1 | rg "Doc-tests|test result:" | tail -4
```

Expected: `Doc-tests embedded_database_embed ... test result: ok. 0 passed; 0 failed`（`std` 关闭时门面与示例一并消失，不会编译失败）。

- [ ] **Step 6: 验证文档无警告**

```bash
cargo doc -p embedded-database-embed --features std --no-deps 2>&1 | rg -c "warning" || echo "0 warnings"
```

Expected: `0 warnings`（`Db`/`Txn`/`Placeholder` 的 intra-doc 链接都能解析；`Placeholder` 链接在 std 路径下悬空——若出现 `unresolved link to Placeholder`，把该链接降级为反引号纯文本 `` `Placeholder` ``）。

- [ ] **Step 7: 提交**

```bash
git add crates/embedded-database-embed/src/lib.rs
git commit -m "docs: embed 稳定面承诺与 Db/Txn 可执行示例（doctest）"
```

---

### Task 5: 门禁命令纳入 embed（CI host job）

**Files:**
- Modify: `.github/workflows/ci.yml:14-15`

**Interfaces:**
- Consumes: Task 1 的 `std` 特性（命令里的 `embedded-database-embed/std` 依赖它存在）。
- Produces: clippy 与 test 都在带 `embedded-database-embed/std` 的特性集下运行；embed 的 doctest 因此进入门禁。

- [ ] **Step 1: 修改 clippy 与 test 两步**

把 `.github/workflows/ci.yml` 中：

```yaml
      - run: cargo clippy --workspace --all-targets --features embedded-database-core/std -- -D warnings
      - run: cargo test --workspace --features embedded-database-core/std
```

替换为：

```yaml
      # embedded-database-embed/std 与 embedded-database-core/std 都必须显式打开：embed 的门面（含 doctest）
      # 受 cfg(any(test, feature = "std")) 门控，doctest 编译时 cfg(test) 为假，
      # 不带该特性则 embed 的 doctest 在门禁中永不执行。
      - run: cargo clippy --workspace --all-targets --features embedded-database-core/std,embedded-database-embed/std -- -D warnings
      - run: cargo test --workspace --features embedded-database-core/std,embedded-database-embed/std
```

- [ ] **Step 2: 静态核对 YAML（无新增 job、其余 job 未动）**

```bash
rg -n "^  [a-z-]+:|features embedded-database" .github/workflows/ci.yml
```

Expected: job 列表仍是 `host` / `nostd` / `supply-chain` / `big-endian-guard-expected-failure`；两条 `- run:` 行都含 `embedded-database-core/std,embedded-database-embed/std`；`nostd` job 的 `cargo check -p embedded-database-core -p embedded-database-kv -p embedded-database-embed --target ...` 未变。

- [ ] **Step 3: 本地跑改后的门禁命令（fmt）**

```bash
cargo fmt --all --check
```

Expected: 无输出（退出码 0）。

- [ ] **Step 4: 本地跑改后的门禁命令（clippy）**

```bash
cargo clippy --workspace --all-targets --features embedded-database-core/std,embedded-database-embed/std -- -D warnings
```

Expected: `Finished`，无 warning。

- [ ] **Step 5: 本地跑改后的门禁命令（test）**

```bash
cargo test --workspace --features embedded-database-core/std,embedded-database-embed/std 2>&1 | rg "test result:|Doc-tests" | tail -14
```

Expected: 各 crate 结果与基线一致且总数 ≥ 301；embed 行为 `10 passed`（unittest）+ `Doc-tests embedded_database_embed ... 2 passed`。逐 crate 基线：core 216、kv 8、os 5、sql 72。

- [ ] **Step 6: 提交**

```bash
git add .github/workflows/ci.yml
git commit -m "ci: host job 门禁命令纳入 embedded-database-embed/std（覆盖 embed doctest）"
```

---

### Task 6: 文档如实化与账本

**Files:**
- Modify: `README.md`（状态行 + 构建命令 + 新增小节）
- Modify: `docs/superpowers/specs/2026-10-02-embedded-database-project-summary.md:58-60`
- Modify: `docs/superpowers/specs/2026-10-02-embedded-database-delivery-checklist.md:12`
- Modify: `docs/superpowers/specs/2026-10-02-embedded-database-certification-template.md:34`
- Modify: `docs/superpowers/specs/2026-10-02-embedded-database-operations-manual.md:394`
- Modify: `.superpowers/sdd/progress.md`（Spec 3a 记账行 + M15 核销注记）

**Interfaces:**
- Consumes: Task 1–5 的产物与**实测数字**（embed 源码行数、workspace 测试总数与 doctest 数）。
- Produces: 文档状态与代码一致；账本可追溯。

- [ ] **Step 1: 采集实测数字（后续所有替换都用这些数）**

```bash
wc -l crates/embedded-database-embed/src/lib.rs
cargo test --workspace --features embedded-database-core/std,embedded-database-embed/std 2>&1 | rg "^test result: ok" | awk '{s+=$4} END {print "TOTAL", s}'
cargo test --workspace --features embedded-database-core/std,embedded-database-embed/std 2>&1 | rg -A1 "Doc-tests embedded_database_embed" | tail -1
```

Expected: 第一行给出 embed 行数 `N`；第二行 `TOTAL <总测试数>`（应 = 313 = 311 单元 + 2 doctest：**awk 的 `{s+=$4}` 汇总的是所有 `test result: ok` 行，其中包含 `Doc-tests embedded_database_embed` 的 doctest 结果行**，即 301 基线 + embed 10 单元 = 311 单元，再加 embed 2 doctest = 313）；第三行给出 `2 passed` 的 doctest 行。**记下 N 与 TOTAL**（下面用 `N` / `TOTAL` 指代）。

- [ ] **Step 2: README 构建命令补特性**

把 `README.md` 中：

```bash
cargo test --features embedded-database-core/std
```

替换为：

```bash
cargo test --features embedded-database-core/std,embedded-database-embed/std
```

- [ ] **Step 3: README 状态行更新测试总数**

把 `README.md` 中：

```
- 状态：M1–M36 已核实完成，301 测试全绿（core 216 + kv 8 + os 5 + sql 72）；
```

替换为（把 `TOTAL` 换成 Step 1 实测值；embed 拆出显示）：

```
- 状态：M1–M36 已核实完成，TOTAL 测试全绿（core 216 + kv 8 + os 5 + sql 72 + embed 10），
  含 embed 门面 2 个 doctest（Spec 3a）；
```

- [ ] **Step 4: README 新增「嵌入 API（公开面）」小节**

在 `README.md` 的 `## 架构约束` 一行**之前**插入：

````markdown
## 嵌入 API（公开面）

`embedded-database-embed` 是 `embedded-database-core` 面向使用者的稳定公开面，提供 `Db` / `Txn` 门面
（`open` / `put` / `get` / `flush` / `compact_with_budget` / `begin`）。默认 no_std
仅导出 `Placeholder`；完整门面需 `std` 特性：

```toml
embedded-database-embed = { path = "crates/embedded-database-embed", features = ["std"] }
```

```rust
use embedded_database_embed::{Config, Db, InMemoryDevice};

let mut dev = InMemoryDevice::new(4 << 20);
let mut db = Db::open(&mut dev, Config::default()).unwrap();
db.put(b"key", b"value").unwrap();
assert_eq!(db.get(b"key").unwrap(), Some(b"value".to_vec()));
```

稳定面 = `Db` / `Txn` 及其方法签名与再导出类型；不兼容变更需提升 minor/major 版本。
明确不做：`delete` / 范围迭代 / C-FFI / 真正的 no_std 可用
（详见 `docs/superpowers/specs/2026-10-19-embedded-database-embed-public-api-design.md`）。

````

- [ ] **Step 5: project-summary.md 更新 embed 行数**

把 `docs/superpowers/specs/2026-10-02-embedded-database-project-summary.md` 中（两处独立替换，勿跨越 `embedded-database-sql` 等中间行）：

(a)

```
  `embedded-database-embed`（10 行）、`embedded-database-os`（182 行）、
```

替换为（`N` = Step 1 实测行数）：

```
  `embedded-database-embed`（N 行，公开门面，Spec 3a）、`embedded-database-os`（182 行）、
```

(b)

```
- 合计约 11554 行 Rust 源码
```

替换为：

```
- 合计约 11554 行 Rust 源码（合计为历史近似值，未含 Spec 3a 的 embed 门面增量）
```

- [ ] **Step 6: delivery-checklist.md 更新 embed 行**

把 `docs/superpowers/specs/2026-10-02-embedded-database-delivery-checklist.md` 中：

```
| embedded-database-embed | 公开嵌入 API | 10 | ✅ |
```

替换为（`N` = Step 1 实测行数）：

```
| embedded-database-embed | 公开嵌入 API（`Db`/`Txn` 门面） | N | ✅ |
```

并在该表格的 `| **合计** | | **~11554** | |` 行下方追加一行脚注：

```
（Spec 3a 后 embedded-database-embed 为实测行数；合计为历史近似值，未重算。）
```

- [ ] **Step 7: certification-template.md 更新 embed 行**

把 `docs/superpowers/specs/2026-10-02-embedded-database-certification-template.md` 中：

```
| embedded-database-embed | ~10 | <1% |
```

替换为（`N` = Step 1 实测行数，`P` = `round(N / 8900 * 100)` 的整数值）：

```
| embedded-database-embed | ~N | ~P% |
```

并在该表格的 `| **合计** | **~8900** | **100%** |` 行下方追加一行脚注：

```
（Spec 3a 后 embedded-database-embed 为实测行数；合计与占比为历史近似值，未重算。）
```

- [ ] **Step 8: operations-manual.md 更新 embed 行**

把 `docs/superpowers/specs/2026-10-02-embedded-database-operations-manual.md` 中：

```
| `embedded-database-embed` | 嵌入 API | ✅ | 禁止 |
```

替换为：

```
| `embedded-database-embed` | 嵌入 API（公开门面，std 门控） | ✅（std 门控） | 禁止 |
```

- [ ] **Step 9: 账本追加 Spec 3a 记账行**

在 `.superpowers/sdd/progress.md` **文件末尾**追加（`N` / `TOTAL` 用 Step 1 实测值）：

```markdown

## 2026-10-19 Spec 3a 执行记录（subagent-driven）

- Spec3a 计划: docs/superpowers/plans/2026-10-19-embedded-database-embed-public-api.md（BASE=8ef6d1a）
- Spec3a 设计: docs/superpowers/specs/2026-10-19-embedded-database-embed-public-api-design.md（8ef6d1a）
- Spec3a 范围: embedded-database-embed 由 10 行 Placeholder → 公开 Db/Txn 门面（显式转发 core，不 pub use core 的 Db/Txn）+ 10 单元用例 + 2 doctest；零 core 改动；无新第三方依赖（dev-dep 仅 workspace 成员 embedded-database-os）。
- Spec3a 关键决策（有据变更）: 门禁命令由 `--features embedded-database-core/std` 改为 `--features embedded-database-core/std,embedded-database-embed/std`——embed 门面受 cfg(any(test, feature="std")) 门控，doctest 编译时 cfg(test) 为假，不带该特性则 embed 的 doctest 在门禁中永不执行（实测：带特性 2 passed / 不带 0 passed）。CI host job 与 README 构建片段同步。
- Spec3a 未改历史 plans（2026-10-02-embedded-database-m13-delivery.md:136 / m15-wrap-up.md:71 的「~10 行」记录当时状态）；当前状态表述已在 README/project-summary/delivery-checklist/certification-template/operations-manual 中更新。
- Spec3a pre-flight 裁决（用户确认）: `Db::get` 就地复刻 payload 解析（与 core `Txn::get`/kv 同构）判为「设计已决」（core 冻结 + `&self` 读路径不取写者锁）；已在 `Db::get` 文档注释注明理由。评审若标 plan-mandated Important，不再回圈。
- Spec3a 计划调整（执行中有据修正）: Task 1 骨架**不写** `pub use imp::*;`——空模块 glob 重导出触发 rustc 1.81 `unused_imports`，使 `clippy -- -D warnings` 失败（实测）；该行改由 Task 2 随 `imp` 内容一并加入。
- Spec3a 实测: embed 源码 N 行；workspace TOTAL 测试全绿（core 216 + kv 8 + os 5 + sql 72 + embed 10）；embed doctest 2 passed。
```

- [ ] **Step 10: 账本 M15 条追加核销注记**

找到 `.superpowers/sdd/progress.md` 中 `- [x] **M15 全部完成**：文档一致性修复 + 最终交付清单 + 设计文档同步，0 提交。`（约 580 行），在其**下一行**追加：

```
  - ⚠️ 核销 2026-10-19（Spec 3a）：M15 所记 `embedded-database-embed`「~10 行」为当时状态；Spec 3a 已把 embed 实装为公开 `Db`/`Txn` 门面，状态表述已同步至 README 与四份交付文档（历史 plans 保留原状）。
```

- [ ] **Step 11: 复核文档一致性**

```bash
rg -n "embedded-database-embed" README.md docs/superpowers/specs/2026-10-02-embedded-database-{project-summary,delivery-checklist,certification-template,operations-manual}.md
```

Expected: 出现的每条表述都与实现一致——无「10 行」「~10」「<1%」残留（`certification-template` 的 `~N` 为实测值）；README 出现了新的「嵌入 API（公开面）」小节。

以及历史 plans 未被改动：

```bash
git status --short docs/superpowers/plans/
```

Expected: 无对 `2026-10-02-embedded-database-m13-delivery.md` / `m15-wrap-up.md` 的改动（`2026-10-19-*` 计划文件本身属本次新增，允许出现在 Task 间提交里）。

- [ ] **Step 12: 提交**

```bash
git add README.md docs/superpowers/specs/2026-10-02-embedded-database-project-summary.md docs/superpowers/specs/2026-10-02-embedded-database-delivery-checklist.md docs/superpowers/specs/2026-10-02-embedded-database-certification-template.md docs/superpowers/specs/2026-10-02-embedded-database-operations-manual.md .superpowers/sdd/progress.md
git commit -m "docs: embed 状态如实化（README 公开面小节 + 交付文档行数）+ Spec 3a 记账"
```

---

## 终验（所有任务完成后）

- [ ] **门禁全绿（最终形态）**

```bash
cargo fmt --all --check && \
cargo clippy --workspace --all-targets --features embedded-database-core/std,embedded-database-embed/std -- -D warnings && \
cargo test --workspace --features embedded-database-core/std,embedded-database-embed/std 2>&1 | rg "^test result: ok"
```

Expected: fmt 无输出；clippy `Finished`；test 各 crate 全 ok，总数为 Step 1 的 TOTAL，`Doc-tests embedded_database_embed` 为 `2 passed`。

- [ ] **无特性路径不变**

```bash
cargo test -p embedded-database-embed && cargo check -p embedded-database-embed
```

Expected: `10 passed`；`cargo check` 成功（`Placeholder` 路径）。

- [ ] **零 core 改动**

```bash
git diff 8ef6d1a..HEAD --stat -- crates/embedded-database-core/ Cargo.toml Cargo.lock
```

Expected: 仅 `Cargo.lock | 1 +`——即 `embedded-database-embed` 的依赖列表新增 `embedded-database-os`（Global Constraints 允许的 workspace 成员 dev-dependency）；`crates/embedded-database-core/` 与根 `Cargo.toml` 必须无改动，且**不得**出现第三方 `[[package]]` 增删。若出现 core 改动或新的第三方包：按 Global Constraints 回退。（原计划此处写「空输出」有误——允许的 dev-dep 必然改动 lock 的一行依赖边。）

- [ ] **无 unsafe**

```bash
rg -n "unsafe" crates/embedded-database-embed/src/
```

Expected: 仅出现 `forbid(unsafe_code)` 一行。

- [ ] **工作树干净**

```bash
git status --short
```

Expected: 空输出。
