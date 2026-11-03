# embedded-database

面向信创国产架构与 PLC/嵌入式硬实时场景的纯 Rust 嵌入式数据库内核。

- 设计文档：`docs/superpowers/specs/2026-10-02-embedded-database-design.md`
- 许可证：Apache-2.0
- 状态：M1–M36 已核实完成，348 测试全绿（core 224 + kv 8 + os 7 + sql 75 + flash 19 + embed 10
  + bench/inject_crash 2），含 doctest 3 个（embed 2 + flash 1，Spec 3a/3b）；
  M37/M38 账本条目已核销为未实现（详见 `.superpowers/sdd/progress.md`）

## 构建

```bash
rustup toolchain install
cargo build
cargo test --features embedded-database-core/std,embedded-database-embed/std
```

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

## 闪存介质后端（Spec 3b）

`embedded-database-flash`：`RawFlash` trait + `SimFlash` 模拟设备（坏块/故障注入）+ 擦除块级 `Ftl`
（实现 core 的 `BlockDevice`），`no_std` + `alloc`、零第三方依赖，可让 `Db` 直接建立在
真实闪存介质之上（19 单测 + 1 doctest 全绿）。

## 架构约束

`embedded-database-core` 强制 `#![no_std]` + `#![forbid(unsafe_code)]`，不使用全局分配器。
容量启动时固定，失败显式返回，不静默降级。
生产构建必须 `default-features = false`。

## WCET 实测基线（Task 9）

机器相关，仅作相对基线。命令：`cargo run -p embedded-database-bench --bin wcet`（debug 构建）。

| 操作 | n | min | p50 | p99 | max | 预算 |
|------|---|-----|-----|-----|-----|------|
| append_page | 32 | 19208ns | 19458ns | 29458ns | 29458ns | 150000ns |
| btree_insert | 128 | 833ns | 1958ns | 7000ns | 7250ns | 200000ns |
| commit | 200 | 791ns | 834ns | 3500ns | 16667ns | 30000ns |

release 构建（`cargo run --release -p embedded-database-bench --bin wcet`）参考：

| 操作 | n | min | p50 | p99 | max | 预算 |
|------|---|-----|-----|-----|-----|------|
| append_page | 32 | 16708ns | 17083ns | 36459ns | 36459ns | 150000ns |
| btree_insert | 128 | 125ns | 209ns | 833ns | 1083ns | 200000ns |
| commit | 200 | 166ns | 167ns | 3292ns | 4292ns | 30000ns |

说明：

- `append_page` 每次写整页 4096B（页头+载荷+零填充）并对整槽做 CRC32c，是 M1 的真实路径；槽耗尽后的 `SlotFrozen` 快速失败不计入样本。
- `btree_insert` 受 `ROOT_CAPACITY * LEAF_CAPACITY = 128` 限制，容量耗尽后的失败不计入样本。
- macOS 共享开发机调度噪声可把 debug p99 推高一个数量级；预算按实测 p99 上界留余量，M2 应以 release 数值为优化目标。
- 退出码 0 = 全部在预算内；1 = 有超预算项。

## WCET 实测基线：Db 事务路径（Task M2-9）

命令同上（debug 构建）。M2 事务路径：`Db::begin` → `put` / `get` → `commit`。

| 操作 | n | min | p50 | p99 | max | 预算 |
|------|---|-----|-----|-----|-----|------|
| db_put | 200 | 125ns | 167ns | 209ns | 2125ns | 10000ns |
| db_get | 200 | 0ns | 42ns | 84ns | 125ns | 10000ns |
| db_commit | 200 | 83ns | 125ns | 125ns | 291ns | 10000ns |

说明：

- M2 事务变更暂存于内存 `Vec`，commit 不写入活跃段；此基线只覆盖该简化路径，M3 落地真实写路径后需重测。
- `db_get` 的 min=0ns 是 `Instant` 计时分辨率限制，不是零成本承诺。
- 预算统一 10μs（≈50× 实测 p99），覆盖共享开发机调度噪声。

## 离线构建

```bash
bash scripts/offline-build.sh
```

该脚本执行：
1. `cargo vendor vendor/` — 下载所有依赖到本地目录
2. 配置 `.cargo/config.toml` 指向 vendored sources
3. `cargo build --offline` — 验证零外网构建
4. `cargo cyclonedx` — 生成 SBOM（`sbom.json`）

说明：

- 脚本会把 SBOM 工具安装放在写入 vendored source 配置之前；若 `cargo-cyclonedx` 因 toolchain 版本无法用 1.81 安装，脚本回退 `cargo +stable install cargo-cyclonedx`，仍失败则跳过 SBOM，其余步骤照常。
- virtual workspace 下 `cargo cyclonedx` 0.5.x 逐 member 输出 `crates/*/*.cdx.json`，脚本聚合去重后写入根目录 `sbom.json`。
- 脚本运行后 `.cargo/config.toml` 指向 `vendor/`，后续 `cargo` 命令默认离线可用 vendored sources；如需恢复 crates.io，请移除或改写该文件。

### cargo-vet 审计

`supply-chain/` 现记录真实审计结论：deploy 图 7 个第三方依赖中，`zerocopy`/`zerocopy-derive`/`bitflags`/`proc-macro2`/`syn`/`unicode-ident`
为本地实质性审计（取证清单见 `docs/superpowers/specs/2026-10-18-supply-chain-audit-design.md` §3.3），
`quote` 由上游 import 覆盖；dev 树（proptest 及其传递依赖）为带据豁免，18 条均含 `notes`、
criteria 全部 `safe-to-run`（注：cargo-vet 0.10.2 的豁免条目不支持 `expires` 字段，写入后会被
下一次 `cargo vet` 重写配置时剥离，复审日期 2027-04-30 以约定文本写入每条豁免的 `notes`）。
公告/许可/来源门禁由 `deny.toml` + cargo-deny 提供；本地入口 `scripts/supply-chain-audit.sh`，
CI 由 `supply-chain` job 强制执行（`cargo vet check` 可离线，`cargo deny check advisories` 需网络）。

```bash
# 需网络；若 .cargo/config.toml 已指向 vendored sources，先移开再安装
cargo install cargo-vet
cargo vet
```

