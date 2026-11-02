# Spec 3a：embedded-database-embed 公开 API（`Db` / `Txn` 纯 Rust 门面）

日期：2026-10-19 · 状态：已实施（2026-10-19，见文末「实施状态与勘误」） · 前置：Spec 2（真实供应链审计）已完成（commit `2bc6c60`）

## 1. 背景

`crates/embedded-database-embed` 目前是 10 行 `Placeholder`：

```rust
#![no_std]
#![forbid(unsafe_code)]
//! embedded-database 公开嵌入 API。
//! M1 阶段仅占位：完整 `Db` / `Txn` 接口属于 M2。
#[derive(Debug)]
pub struct Placeholder;
```

其 crate 文档称「完整 `Db`/`Txn` 接口属于 M2」，但事实上引擎早已交付：`embedded-database-core` 在 `feature = "std"` 下导出 `Db<'d>` / `Txn<'db,'d>`（M2/M5/M12），`embedded-database-kv` 已按同构模式包了一层 `KvDb`，`embedded-database-os` 提供文件后端。**embed 是路线图上唯一「文档说该有、实际没有」的公开面**（`docs/superpowers/specs/2026-10-02-embedded-database-design.md:146` 把 embed 定义为「公开 API、事务管理、Deadline 传播、配额管理」）。

原始设计文档 §5.5 已给出 embed API 草图（`docs/superpowers/specs/2026-10-02-embedded-database-design.md:339-355`），但草图与当前 core 的真实签名有若干差异。本 spec 的定位是：**把 §5.5 的意图落到 core 的真实能力上，并公开一个可承诺稳定性的最小面**——不多做（不做 delete/迭代器/FFI），也不少做（不把 core 内部观测面泄出去）。

### 1.1 实测事实（2026-10-19，本机）

| 事实 | 证据 |
|---|---|
| core 的 `Db<'d>` 公开方法 | `crates/embedded-database-core/src/db.rs:100,256,261,266,271,279,285,294,299,307,312,322,371,395,596,602,622,642,660`（`open/config/arena_pool/active_segment/try_lock_writer/unlock_writer/lookup_page/page_map_len/page_map_entries/pending_writes_len/stats/read_payload/read_page_with_deadline/flush/update_key_map/lookup_key/begin/compact/compact_with_budget`） |
| `Db::open(dev: &'d mut dyn BlockDevice, cfg: Config) -> Result<Db<'d>, Error>` | `db.rs:100` |
| `Txn<'db,'d>` 公开方法 | `db.rs:843,851,867,894,915`（`deadline/put/get/scan/commit`）；`put` 拒绝含 `0xFF` 的 key（`db.rs:851-860`），`get` 返回 `Option<Vec<u8>>`（`db.rs:867`），`scan` 返回 `Cursor<'_>`（`db.rs:894`） |
| `Txn::commit` 只把变更放进 `Db.pending_writes`（**未落盘**），并在提交时释放写者锁 | `db.rs:915-945`（`self.db.unlock_writer()` 在 940 行） |
| `Txn` 未提交即 drop → 变更丢弃 + 释放写者锁 | `db.rs:948-954` |
| `Db::compact()` 是 M2 占位实现（只验锁，不做合并）；`compact_with_budget(&CompactionBudget, Option<Deadline>) -> Result<CompactReport, Error>` 才是真实实现 | `db.rs:642-658`（占位）、`db.rs:660-...`（C7 deadline 契约） |
| `Config: Default`、`TxnOptions: Default`、`CompactionBudget: Default`，`IntegrityLevel` 默认 `Commit` | `types.rs:173-187`、`types.rs:98-101`、`types.rs:148-156`、`types.rs:111-120` |
| `Config` 是 std 可用但**非** std 门控的公开类型；`DbStats` 是 `#[cfg(feature = "std")]` 门控 | `types.rs:158-171`、`types.rs:127-135`；core 再导出见 `crates/embedded-database-core/src/lib.rs:84` |
| core 的 `Db`/`Txn`/`Cursor`/`InMemoryDevice`/`DbStats`/`CommitInfo` 全部 `std` 门控 | `crates/embedded-database-core/src/lib.rs:50-90` |
| `BlockDevice` trait 是 core-only（非 std 门控），签名为 `size/read_at/write_at/sync` | `crates/embedded-database-core/src/lib.rs:63`、`crates/embedded-database-core/src/device.rs` |
| `embedded-database-os::FileDevice::open(&Path, size) -> Result<Self>` 实现 `BlockDevice`，仅依赖 `embedded-database-core`（与 embed 无环） | `crates/embedded-database-os/src/file.rs`、`crates/embedded-database-os/Cargo.toml` |
| kv 的同构模式：`#![no_std]` + `#[cfg(any(test, feature = "std"))] mod imp` + `Placeholder` + `std = ["embedded-database-core/std"]` + dev-dep `embedded-database-core{features=["std"]}` | `crates/embedded-database-kv/src/lib.rs:1-12`、`crates/embedded-database-kv/Cargo.toml` |
| embed 当前 `Cargo.toml` **无** `[features]` 表，依赖 `embedded-database-core.workspace = true`（workspace 定义为 `default-features = false`） | `crates/embedded-database-embed/Cargo.toml`、根 `Cargo.toml` |
| CI `nostd` job 对 5 个目标执行 `cargo check -p embedded-database-core -p embedded-database-kv -p embedded-database-embed --target <t>`——embed 必须在**无 std 特性**下可编译 | `.github/workflows/ci.yml:35-48` |
| CI `host` job 的门禁为 `cargo fmt --all --check`、`cargo clippy --workspace --all-targets --features embedded-database-core/std -- -D warnings`、`cargo test --workspace --features embedded-database-core/std` | `.github/workflows/ci.yml:13-15` |
| 仓库**没有** `crates/*/tests/` 目录，全部测试是 `#[cfg(test)]` 内联在 `src/` | `ls crates/*/tests` 无匹配；`crates/embedded-database-os/src/lib.rs:9` |
| 当前测试基线 301（core 216 + kv 8 + os 5 + sql 72） | `README.md` 状态行；`docs/superpowers/plans/2026-10-18-ledger-baseline-readme.md:14` |
| 多处文档把 embed 记为「~10 行 / ✅」 | `docs/superpowers/specs/2026-10-02-embedded-database-project-summary.md:58`、`delivery-checklist.md:12`、`certification-template.md:34`、`operations-manual.md:394`；历史 plans `2026-10-02-embedded-database-m13-delivery.md:136`、`m15-wrap-up.md:71` |

**关键的 cfg/门禁推论（本 spec 的硬约束）**：`cargo test --workspace --features embedded-database-core/std` 只打开 **core** 的 `std`，**不会**打开 embed 自己的 `std`。embed 的门面经由 `cfg(any(test, feature = "std"))` 门控，因此：

- **单元测试**（`cfg(test)` 为真）在该命令下会正常编译并运行——这也是仓库既有惯例（无 `tests/` 目录）能把门面测起来的原因；
- **doctest / 集成测试**（`cfg(test)` 为假）在该命令下**看不到门面**——`tests/` 集成测试要么编译失败、要么被 `cfg` 静默跳过（即 CI 中永不执行）；
- 因此若要在门禁里真正执行 embed 的 doctest，命令必须显式带上 `embedded-database-embed/std`。

## 2. 目标与范围

**目标**：`embedded-database-embed` 成为**面向外部使用者的稳定 Rust 门面**，暴露引擎的最小可用集，并如实承载 deadline 传播与写者 `Busy` 语义；文档与代码状态一致（不再有「文档说该有、代码是占位」的错位）。

**范围内**：

1. `crates/embedded-database-embed/Cargo.toml`：补齐 `[features]`（`default = []`、`std = ["embedded-database-core/std"]`）+ dev-dependencies（`embedded-database-core{features=["std"]}`、`embedded-database-os`）。
2. `crates/embedded-database-embed/src/lib.rs`：crate 级文档（含稳定面承诺）+ `imp` 模块（`Db`/`Txn` 门面）+ 再导出 + `Placeholder` 保留 + 单元测试 + doctest。
3. `.github/workflows/ci.yml`：`host` job 的 clippy / test 命令带上 `embedded-database-embed/std`（让 embed 门面与非 test 编译路径（含 doctest）进入门禁）。
4. 文档如实化：`README.md` 新增「嵌入 API（公开面）」小节；修正 4 处把 embed 记为「~10 行」的当前状态表述。
5. 账本：`.superpowers/sdd/progress.md` 追加 Spec 3a 记账行，并在 M15 条下追加核销注记（embed 由占位转为已实现）。

**范围外**（明确不做，见 §3.6）：`delete`/墓碑；范围迭代器（`Cursor` 不外泄）；C/FFI ABI；真正的 no_std 可用（core 的 `Db`/`Txn` 仍 std 门控）；`embedded-database-flash` FTL（Spec 3b）；`compact()` 占位实现转正；对历史 plans 的追溯性改写。

## 3. 设计

### 3.1 no_std 边界与 Cargo.toml

与 `embedded-database-kv` 完全同构：

```toml
[features]
default = []
# std 路径：启用 embedded-database-core 的 std（Db/Txn 等导出），提供完整门面实现。
# no_std 路径（default）：仅导出 Placeholder，保证裸机目标可编译（CI nostd job）。
std = ["embedded-database-core/std"]

[dependencies]
embedded-database-core = { workspace = true, default-features = false }

[dev-dependencies]
# 测试在 host 上编译，需要 core/std 的 Db/InMemoryDevice，以及真文件后端。
embedded-database-core = { workspace = true, features = ["std"] }
embedded-database-os = { workspace = true }
```

lib.rs 结构（与 kv 同构）：

```rust
#![no_std]
#![forbid(unsafe_code)]
//! crate 文档：定位 + 稳定面承诺 + no_std/std 门控说明 + 示例

#[cfg(any(test, feature = "std"))]
mod imp;
#[cfg(any(test, feature = "std"))]
pub use imp::*;

/// no_std 占位：保证 crate 在无 std 特性时非空且可编译。
#[cfg(not(any(test, feature = "std")))]
#[derive(Debug)]
pub struct Placeholder;
```

### 3.2 公开 API 契约（std 门控）

```rust
pub struct Db<'d> { /* core::Db<'d> */ }

impl<'d> Db<'d> {
    pub fn open(dev: &'d mut dyn BlockDevice, cfg: Config) -> Result<Db<'d>, Error>;
    pub fn config(&self) -> &Config;
    pub fn active_segment(&self) -> u64;
    pub fn stats(&self) -> DbStats;
    pub fn put(&mut self, k: &[u8], v: &[u8]) -> Result<CommitInfo, Error>;
    pub fn get(&self, k: &[u8]) -> Result<Option<Vec<u8>>, Error>;
    pub fn flush(&mut self) -> Result<CommitInfo, Error>;
    pub fn compact_with_budget(
        &mut self,
        budget: &CompactionBudget,
        deadline: Option<Deadline>,
    ) -> Result<CompactReport, Error>;
    pub fn begin(&mut self, opts: TxnOptions) -> Result<Txn<'_, 'd>, Error>;
}

pub struct Txn<'db, 'd> { /* core::Txn<'db, 'd> */ }

impl<'db, 'd> Txn<'db, 'd> {
    pub fn deadline(&self) -> Option<Deadline>;
    pub fn put(&mut self, k: &[u8], v: &[u8]) -> Result<(), Error>;
    pub fn get(&self, k: &[u8]) -> Result<Option<Vec<u8>>, Error>;
    pub fn commit(self) -> Result<CommitInfo, Error>;
}
```

签名与 core 逐一对应（`Db::open`/`config`/`active_segment`/`stats`/`flush`/`compact_with_budget`/`begin`、`Txn::deadline`/`put`/`get`/`commit`），不新增语义。实现为**显式转发**（不 `pub use` core 类型），使 embed 的公开面独立于 core 的内部演进。

### 3.3 类型再导出清单

```rust
pub use embedded_database_core::{
    // 参数/返回类型（方法签名必需）
    BlockDevice, CommitInfo, CompactionBudget, CompactReport, Config, Deadline, DbStats,
    EncryptionKey, Error, IntegrityLevel, Result, Sm4Key, TxnOptions,
};
// 门面可见时才可见（即与 imp 同一门控），故写在 imp 内：
#[cfg(any(test, feature = "std"))]
pub use embedded_database_core::InMemoryDevice;
```

- `EncryptionKey` 必须导出：它是 `Config.encryption_key: Option<EncryptionKey>` 的字段类型，不导出则调用者无法构造启用加密的配置。
- `Sm4Key` 必须随 `EncryptionKey` 一并导出（共 14 个类型）：它是 `EncryptionKey.sm4` 的字段类型，而 core 只给了 `pub struct Sm4Key { pub bytes: [u8; 16] }`（无 `Default`、无构造函数，见 `crates/embedded-database-core/src/sm4.rs:40-42`）——不导出则调用者即使拿到 `EncryptionKey` 类型也拼不出它的值。
- `Result`/`Error` 直接复用 core（**用户的明确选择**）：`pub use embedded_database_core::{Error, Result}`，不引入 embed 自有错误类型、不做 newtype 包裹。
- `InMemoryDevice` 导出便于调用者与文档示例；真文件后端由 `embedded-database-os::FileDevice` 提供，embed **不**依赖 os（os 仅作 dev-dependency）。

### 3.4 语义与不变量（写入 rustdoc）

| 项 | 语义 |
|---|---|
| `Db::put` | 便捷路径 = `begin(TxnOptions::default())` → `Txn::put` → `Txn::commit` → `Db::flush`，返回**`flush` 的** `CommitInfo`（真正落盘的段序号与字节数）。key 含 `0xFF` → `InvalidArgument`（payload 以首个 `0xFF` 分隔 key/value）。 |
| `Db::get` | `lookup_key` → `read_payload` → 解析 `key ++ 0xFF ++ value`。**只读已 flush 的提交**（`commit` 只进 `pending_writes`）；`Db::put` 自带 flush，故「put 后可 get」。**不带 deadline**——需要超时语义的读用 `begin(TxnOptions { deadline })` → `Txn::get`。 |
| `Db::flush` | 把 `pending_writes` 落盘到活跃段（同段追加或开新段），更新段序号与映射表。 |
| `Db::begin` | 先 `auto_flush` 再取写者锁；锁被占用返回 `Error::Busy`（**绝不自旋等待**，设计文档 §5.4）。 |
| `Txn` 生命周期 | `commit` 后变更仅在 `pending_writes`，需 `Db::flush()` 才持久化；未 `commit` 即 drop → 变更丢弃 + 释放写者锁（由 core `Txn::Drop` 保证，embed 的 `Txn` 不实现 `Drop`，以保住 `commit(self)` 的字段移动）。 |
| `Txn::put`/`Txn::get` | 携带 `opts.deadline` 时先检查 `remaining()`；`put` 拒绝含 `0xFF` 的 key。 |
| `compact_with_budget` | 采用 core 的 C7 deadline 契约：过期 + `interruptible = true` → 返回 `interrupted: true` 的报告；过期 + `interruptible = false` → `Err(DeadlineExceeded)`；未过期 → 正常执行。 |
| `stats` | 返回 core 的计数快照（`commit_count`/`read_count`/`write_count`/`error_count`/`last_error`）。 |

### 3.5 与原始设计文档 §5.5 的如实偏差（写入 rustdoc 或 README）

1. `Txn::get` 返回 `Option<Vec<u8>>`，非 §5.5 草图的 `Option<&[u8]>`。
2. **不暴露 `Txn::scan`**：其返回 `core::Cursor`，会把内部类型泄进公开面（记入范围外）。
3. **不暴露 `Db::compact()`**（M2 占位实现）：稳定面上不放占位实现，只给 `compact_with_budget`。
4. `BlockDevice` 采用 core 实际签名（`size()`，无 §5.5 草图的 `capacity()`/`block_size()`/`erase_block_size()`）。
5. 不暴露 `page_map_entries`/`lookup_page`/`read_payload`/`pending_writes_len`/`try_lock_writer`/`arena_pool` 等内部观测面。
6. `Db::begin` 在 core 中取 `&mut self`（§5.5 草图写作 `&self`）。

### 3.6 明确不做

- 不引入 C/FFI ABI（会打破全仓 `forbid(unsafe_code)` 的叙事；需另立 spec 并逐行审计）。
- 不改 core 的 std 门控（真正的 no_std 可用需把 `Db`/`Txn`/`recovery` 改为 alloc-only，属独立大改造）。
- 不做 `delete`/墓碑与范围迭代（需先给 core 加能力）。
- 不为 embed 引入自有错误类型或 deprecation 机制（无外部消费者，属提前投入）。

## 4. 交付物

1. `crates/embedded-database-embed/Cargo.toml`：`[features]`（`default = []`、`std = ["embedded-database-core/std"]`）+ `[dependencies] embedded-database-core { default-features = false }` + `[dev-dependencies]`（core std、embedded-database-os）。
2. `crates/embedded-database-embed/src/lib.rs`：
   - crate 级文档：定位、no_std/std 门控、稳定面范围、示例；
   - `#[cfg(any(test, feature = "std"))] mod imp`：`Db`/`Txn` 门面（§3.2 签名）+ 再导出（§3.3）+ 1–2 个 rustdoc 示例（doctest）；
   - `#[cfg(not(any(test, feature = "std")))] pub struct Placeholder;`（原样保留，更新文案）；
   - `#[cfg(all(test, feature = "std"))] mod tests`：单元用例（见 §6.3）。
3. `.github/workflows/ci.yml`：`host` job 的 clippy 与 test 命令加 `,embedded-database-embed/std`（理由见 §1.1 推论：不加以则 embed 的 doctest 门禁中永不执行）；`nostd` job 不变（已覆盖 embed）。**不**新增 job、**不**改动 trigger 与其它 job。
4. 文档如实化：
   - `README.md`：新增「嵌入 API（公开面）」小节——稳定面清单、`Db`/`Txn` 用法示例、门禁命令（含 `embedded-database-embed/std`）、以及「非目标」一句；同时把 README「构建」片段里的测试命令补上 `,embedded-database-embed/std`（与 §4.3 的 CI 命令保持一致）；
   - `docs/superpowers/specs/2026-10-02-embedded-database-project-summary.md:58`、`delivery-checklist.md:12`、`certification-template.md:34`：把 embed「10 行 / ~10 / <1%」的当前状态表述更新为已实现（如实写 API 面）；
   - `docs/superpowers/specs/2026-10-02-embedded-database-operations-manual.md:394`：核对表述（行已记 ✅；如与实现不符则改）；
   - **历史 plans 不改**：`2026-10-02-embedded-database-m13-delivery.md:136`、`m15-wrap-up.md:71` 记录的是当时状态，属历史记录（与 Spec 2 对文档的处理口径一致），理由写入账本记账行。
5. `.superpowers/sdd/progress.md`：Spec 3a 记账行 + M15 条下核销注记。

## 5. 执行顺序（供实施计划参考）

1. Cargo.toml（features + dev-deps）→ `cargo check -p embedded-database-embed`（default）与 `--features std` 双向可编译。
2. lib.rs 门面 + 再导出 + rustdoc（含 doctest）。
3. 单元测试（含 `embedded-database-os::FileDevice` 持久化用例）+ 门禁全跑。
4. CI 命令更新（clippy/test 加 `embedded-database-embed/std`）。
5. 文档如实化 + 账本。
6. 门禁复核（§6）与提交（每逻辑单元一提交：crate 门面 / 测试 / CI / 文档+账本）。

## 6. 验收标准

1. **双向可编译**：`cargo check -p embedded-database-embed`（无特性）成功且只暴露 `Placeholder`；`cargo check -p embedded-database-embed --features std` 成功且暴露 §3.2/§3.3 全部项；`cargo check -p embedded-database-core -p embedded-database-kv -p embedded-database-embed --target <5 个 no_std 目标>` 全部通过（与 CI `nostd` job 一致）。
2. **公开面与 §3.2/§3.3 逐字一致**：以 `cargo doc -p embedded-database-embed --features std` 或源码核对为准；实现是显式转发，无 `pub use embedded_database_core::{Db, Txn}` 式别名。
3. **测试**：embed 单元用例覆盖——open + put/get 往返、缺失键返回 `None`、含 `0xFF` key 的 `put` 返回 `InvalidArgument`、`begin → put → commit → flush` 后再 `get` 可见、未 commit 的 `Txn` drop 后变更不可见且可再次 `begin`（写者锁已释放）、`compact_with_budget` 后数据仍可回读、`stats()` 计数随操作增长；**持久化用例**：`forbid(unsafe_code)` 约束下用 `embedded-database-os::FileDevice` 建临时文件 → 写入 → drop `Db` → 重新 `open` 同文件 → 回读一致。用例写在 `src/` 内 `#[cfg(all(test, feature = "std"))] mod tests`（仓库无 `tests/` 惯例，且门禁下 `cfg(test)` 才可见门面）。
4. **门禁不回退**：`cargo fmt --all --check` 干净；`cargo clippy --workspace --all-targets --features embedded-database-core/std,embedded-database-embed/std -- -D warnings` 干净；`cargo test --workspace --features embedded-database-core/std,embedded-database-embed/std` 通过且**总数 ≥ 301 且原 301 项无删除/改动**（如实测数目写入报告）；embed 的 doctest 在该命令下真实执行（报告需给出 doctest 计数）。
5. **CI 一致**：`host` job 的 clippy/test 步骤与 §4.3 一致；`nostd` job 未改；无新增 job。
6. **稳定性承诺落纸**：embed crate 文档含稳定面清单与「变更需 minor/major + CHANGELOG」表述；`README.md` 有对应小节。仓库当前无 `CHANGELOG.md`——**本次不新建**，承诺表述为「未来引入 CHANGELOG 前，以 minor/major 版本号与提交信息记录」。
7. **文档一致**：§4.4 的四处当前状态表述与实现一致；历史 plans 未改且账本记录了不改的理由；账本 Spec 3a 记账行与 M15 核销注记存在。
8. **零 unsafe / 零 core 改动**：`git diff` 不触及 `crates/embedded-database-core/`（embed 只用 core 既有公开 API）；CI unsafe 扫描所用的 `find crates -path '*/src/*' -name '*.rs'` 范围内仍无 `unsafe`。
9. **提交序列**遵循「每逻辑单元完成即提交」，且 `git status` 干净。

## 7. 风险与错误处理

- **doctest 的双重事实**：doctest 在 `cfg(test)` 为假的编译下运行，而 embed 门面受 `cfg(any(test, feature = "std"))` 门控——不加 `embedded-database-embed/std` 则 doctest 看不到门面。处置：门禁命令显式带上 `embedded-database-embed/std`（§4.3），并在报告中给出 doctest 计数；若某 doctest 因链接或示例代码问题无法通过，**不得**降级为 `no_run`/`ignore`——应修正示例或删除该示例并在报告说明。
- **dev-dependency 造成特性统一**：`embedded-database-embed` 的 dev-dep 打开 `embedded-database-core/std`，在 `cargo test` 下会统一到所有 crate（resolver v2 下 dev-only 特性不会传导为 embed 自己的 `std`）。处置：不依赖该统一——门面可见性只由 `cfg(any(test, feature = "std"))` 决定，两种路径（`cargo check -p embedded-database-embed` / `--features std`）都必须单独验证。
- **`Db::get` 的可见性陷阱**：未 flush 的提交不可见。处置：§3.4 写入 rustdoc；单元测试覆盖「commit 后未 flush 不可见、flush 后可见」。
- **`Txn` 不实现 `Drop` 的隐含契约**：写者锁释放完全依赖 core `Txn::Drop`；若将来 core 改变该行为，embed 的「未提交即释放锁」语义会静默失效。处置：写一条显式测试（drop 后 `begin` 成功），并在 rustdoc 标注该依赖。
- **`Db::put` 的写放大**：每次 put 都 `begin + commit + flush`（各自一次锁与一次段写）。处置：rustdoc 注明「批量写入应使用一次 `begin` + 多次 `Txn::put` + 一次 `flush`」，不做额外批量 API（范围外）。
- **公开面溢出**：新增 `pub` 项会永久进入稳定面。处置：§3.6 明确不做项；实施时逐个 `pub` 项与 §3.2/§3.3 清单比对（以 `cargo doc` 或逐行核对为证）。

## 8. 与其它 spec 的关系

- 本 spec 完成后，路线图上「embed = 占位」的错位消除：文档状态与代码一致。
- 与 Spec 3b（Flash 介质后端）：**无依赖**。3b 的缝在 `crates/embedded-database-os/src/file.rs`（后端）与 `crates/embedded-database-core/src/device.rs`（`BlockDevice`），embed 只消费 `BlockDevice` trait，3b 不需要改 embed。
- 与 Spec 1/2：本 spec 只新增 `crates/embedded-database-embed/` 与 CI `host` job 命令、文档与账本；不触碰 `supply-chain/`、`deny.toml`、`scripts/`。新增的 dev-dependency 仅 `embedded-database-os`（workspace 内），**不引入新的第三方依赖**——因此不影响 Spec 2 的审计范围（`Cargo.lock` 的第三方集合不变）。
- 本轮明确不做的 `delete`/迭代器/FFI，如将来需要，应各自另立 spec。

## 9. 实施状态与勘误（2026-10-19 实施后追加）

- **状态**：本文档描述的 Spec 3a 已实施完成（头部状态行同步由「待实施」改为「已实施」）。
- **有据偏差（更强，有意为之）**：§4.2 与 §6.3 写作 `#[cfg(all(test, feature = "std"))] mod tests`，而实施计划与最终实现用的是 `#[cfg(test)] mod tests`（`crates/embedded-database-embed/src/lib.rs`）。后者约束更强：即使在无 `std` 特性的 `cargo test -p embedded-database-embed` 下，测试模块也可见（`imp` 经 `cfg(any(test, feature = "std"))` 同时对 `cfg(test)` 开放），使「无特性路径下门面仍可测」成为真实不变式，而非依赖 dev-dependency 特性统一的巧合。此为**有意的、更强的**偏差，不回改代码。
- **勘误（§3.4 `Db::begin` 一行与 core 实际顺序相反）**：§3.4 原文称「先 `auto_flush` 再取写者锁」。core 实际顺序是**先取写者锁、再 `auto_flush`**：
  - `crates/embedded-database-core/src/db.rs:623-625`：先校验 deadline（`dl.remaining()?`，未过期才继续）；
  - `crates/embedded-database-core/src/db.rs:626`：`self.try_lock_writer()?;`（抢锁，占用即 `Err(Busy)`）；
  - `crates/embedded-database-core/src/db.rs:628-632`：抢锁成功后才调 `self.auto_flush()`，且 flush 失败会 `self.unlock_writer()` 释放锁后返回错误；
  - core 自身注释亦写明「M7：抢锁后调用 `auto_flush()`」（`db.rs:620`）。
  即：**写者锁在前，`auto_flush` 在后**；§3.4 表格该行的顺序描述作废，语义其余部分（Busy 不等待、失败释放锁）与代码一致。
- **§1.1 证据已过期（快照性质）**：§1.1 是**实施前**的实测快照，不是当前状态。当前：测试基线为 **313**（原记 301；`cargo test --workspace --features embedded-database-core/std,embedded-database-embed/std` 的 `test result: ok` 汇总，含 embed 2 个 doctest），门禁命令为 `cargo test --workspace --features embedded-database-core/std,embedded-database-embed/std`（clippy 同款，原记 `--features embedded-database-core/std`）。§1.1 各行按实施前证据保留不改。
- **再导出清单 13 → 14**：§3.3 与实现的 `pub use embedded_database_core::{...}` 现为 14 个类型，新增 `Sm4Key`——它是 `EncryptionKey.sm4` 的字段类型，core 只给 `pub struct Sm4Key { pub bytes: [u8; 16] }`（`crates/embedded-database-core/src/sm4.rs:40-42`，无 `Default`、无构造函数）。不导出则仅凭 embed 再导出类型无法构造 `EncryptionKey`，与 §3.3 导出 `EncryptionKey` 的理由自相矛盾。已由用例 `门面_配置与活跃段可访问_加密配置仅凭再导出类型构造`（`crates/embedded-database-embed/src/lib.rs`）以编译 + 运行验证。
