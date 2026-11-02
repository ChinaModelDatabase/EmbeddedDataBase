# embedded-database M30 实施计划：BTreeMap key_map + 亿级规模验证

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 两步：(1) `key_map` 从 `Vec<(Vec<u8>, u64)>` 改为 `BTreeMap<Vec<u8>, u64>`，O(n) 线性扫描 → O(log n)；(2) 亿级规模验证——扩段数到 12800，`SCALE_N=100_000_000` 全链路验证。

**Architecture:** M30-0 改 `db.rs` 中 `key_map` 的数据结构与三个方法（`open` 冷启动重建、`update_key_map`、`lookup_key`）。M30-1 修改 `scale_test.rs` 的 `MAX_SEGMENTS`，控制者直接运行 100M 验证。

**Tech Stack:** Rust 1.81、`std::collections::BTreeMap`（`no_std` 下 `alloc` 已提供）、`embedded-database-bench`。

## Global Constraints

- **C1** `embedded-database-core` 保持 `#![no_std]`
- **C2** 顶层 `#![forbid(unsafe_code)]`——不允许任何局部 `unsafe`
- **C3** 运行时依赖仅 `zerocopy`、`bitflags`，不新增
- **C4** 库代码禁止 `unwrap()` / `expect()` / `panic!()` / 数组越界（`unwrap()` 仅允许 `#[cfg(test)]`；bench bin 可用 `unwrap`）
- **C11** 不执行任何 git 提交（本项目零提交策略）
- **工具链**：每个 bash 调用前 `export PATH="/opt/homebrew/opt/rustup/bin:$PATH"`
- **验收基线**：M29-1 交付 291 passed（core 216 + kv 8 + os 5 + sql 62）、fmt/clippy 全过、`inject_crash` exit=0、`wcet` exit=0（macOS 调度噪声偶发 exit=1，3 次全过即可）

## 用户裁决记录

- **M30 范围**：两步——BTreeMap key_map 重构 + 亿级规模验证。
- **M30-0 方案**：`key_map` 改为 `BTreeMap<Vec<u8>, u64>`。
- **M30-1 配置**：`MAX_SEGMENTS = 12800`，容量 102.4M，设备 100GiB。
- **M30-1 执行方式**：控制者直接跑（70 分钟运行时间，不用 subagent）。

---

### Task M30-0: BTreeMap key_map 重构

**Files:**
- Modify: `crates/embedded-database-core/src/db.rs`

**Interfaces:**
- Consumes: `Db`（M7-0）、`lookup_key`（M18）、`update_key_map`（M11）
- Produces: `key_map: RefCell<BTreeMap<Vec<u8>, u64>>`

**改动点**：

| 行 | 当前 | 改为 |
|----|------|------|
| 77 | `key_map: RefCell<Vec<(Vec<u8>, u64)>>` | `key_map: RefCell<BTreeMap<Vec<u8>, u64>>` |
| 192-246 | 冷启动重建（线性 push/find） | BTreeMap insert |
| 599-607 | `update_key_map`（iter().find()） | `map.insert(key.to_vec(), page_id)` |
| 609-614 | `lookup_key`（iter().find()） | `map.get(key)` |

**收益**：
- `lookup_key`：O(n) → O(log n)——亿级规模下 `get` 从不可用变为可用
- `update_key_map`：O(n) → O(log n)——写入路径同步加速
- 冷启动重建：O(n²) → O(n log n)——100M 条冷启动从不可用变为可行

**测试**：复用现有 `key_map_*` 测试（db.rs:1547/1929/1944/1992/2081）。

- [ ] **Step 1: 改 key_map 类型与三个方法**

- [ ] **Step 2: 运行 core 测试**

Run: `cargo test -p embedded-database-core --features std 2>&1 | rg 'test result:' | head -1`
Expected: 216 passed

- [ ] **Step 3: 运行 kv 测试**

Run: `cargo test -p embedded-database-kv 2>&1 | rg 'test result:' | head -1`
Expected: 8 passed

- [ ] **Step 4: fmt + clippy**

Run: `cargo fmt --all --check && cargo clippy -p embedded-database-core --all-targets --features std -- -D warnings 2>&1 | tail -5`
Expected: 通过

---

### Task M30-1: 亿级规模验证

**Files:**
- Modify: `crates/embedded-database-bench/src/bin/scale_test.rs`

**Interfaces:**
- Consumes: `KvDb::put_batch`（M29-1-1）、`FileDevice`（M4-1）
- Produces: `MAX_SEGMENTS = 12800`，容量 102.4M

- [ ] **Step 1: 修改 scale_test.rs 的 MAX_SEGMENTS**

```rust
const MAX_SEGMENTS: usize = 12800; // 1280 → 12800，容量 8000 × 12800 = 102.4M
```

同时更新注释中的设备大小：`8MiB × 12800 = 100GiB`。

- [ ] **Step 2: SCALE_N=50_000_000 探测内存**

Run: `SCALE_N=50000000 cargo run --release -p embedded-database-bench --bin scale_test 2>&1 | tail -10`
Expected: 全部通过，退出码 0

- [ ] **Step 3: SCALE_N=100_000_000 全链路验证**

Run: `SCALE_N=100000000 cargo run --release -p embedded-database-bench --bin scale_test 2>&1 | tail -30`
Expected: 全部通过，退出码 0，读取采样 1000 条全部命中

- [ ] **Step 4: 全量测试**

Run: `cargo test --workspace --features embedded-database-core/std 2>&1 | rg 'test result:' | rg -v '0 passed'`
Expected: 全部通过（291+）

- [ ] **Step 5: 更新设计文档**

在 `docs/superpowers/specs/2026-10-02-embedded-database-design.md` 追加：

```markdown
## 实施状态（M30 完成）

- M30 交付：BTreeMap key_map 重构 + 亿级规模验证。
- **M30-0**：`key_map` 从 `Vec<(Vec<u8>, u64)>` 改为 `BTreeMap<Vec<u8>, u64>`，
  `lookup_key`/`update_key_map` O(n) → O(log n)，冷启动重建 O(n²) → O(n log n)。
- **M30-1**：`MAX_SEGMENTS` 1280 → 12800，容量 102.4M，设备 100GiB。
  `SCALE_N=100_000_000` 全链路验证 exit=0。
- **M30 风险**：内存 ~9.2GB / 16GB 机器，运行 ~70 分钟。
```

- [ ] **Step 6: 检查点**

Run: `rg -n 'M30 完成' docs/superpowers/specs/2026-10-02-embedded-database-design.md`
Expected: 有输出

---

## M30 验收标准

| # | 标准 | 验证方式 |
|---|------|----------|
| 1 | `key_map` 改为 `BTreeMap`，core 216 passed | M30-0 Step 2 |
| 2 | `SCALE_N=50_000_000` 探测通过 | M30-1 Step 2 |
| 3 | `SCALE_N=100_000_000` 全链路 exit=0 | M30-1 Step 3 |
| 4 | 读取采样 1000 条全部命中 | M30-1 Step 3 输出 |
| 5 | workspace 291+ 测试全绿 | M30-1 Step 4 |
| 6 | 设计文档含「实施状态（M30 完成）」 | M30-1 Step 5 |

## 后续里程碑

M30 完成后：

- **M31**：十亿级规模验证（100000 万条记录）
- **M32**：LoongArch 实机验证（需硬件）
