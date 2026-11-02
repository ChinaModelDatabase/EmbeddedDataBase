# embedded-database M36 实施计划：SQLite 对齐

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 参考 SQLite 对比，补 embedded-database 短板：数据类型（BOOLEAN/BLOB）、SQL 函数扩充、跨平台验证。

**Architecture:** M36-0 扩展 `Value` 枚举 + 词法/解析/序列化；M36-1 基于 M32 trait 扩展内置函数；M36-2 验证 `FileDevice` 跨平台 + 操作手册更新。

**Tech Stack:** Rust 1.81、`embedded-database-sql`（alloc）、`embedded-database-os`（std）。

## Global Constraints

- **C1** `embedded-database-core` 保持 `#![no_std]`；`embedded-database-sql` 允许 `alloc`
- **C2** 顶层 `#![forbid(unsafe_code)]`——不允许任何局部 `unsafe`
- **C3** 运行时依赖仅 `zerocopy`、`bitflags`，不新增
- **C4** 库代码禁止 `unwrap()` / `expect()` / `panic!()` / 数组越界（`unwrap()` 仅允许 `#[cfg(test)]`；bench bin 可用 `unwrap`）
- **C11** 不执行任何 git 提交（本项目零提交策略）
- **工具链**：每个 bash 调用前 `export PATH="/opt/homebrew/opt/rustup/bin:$PATH"`
- **验收基线**：M35 交付 294 passed（core 216 + kv 8 + os 5 + sql 65）、fmt/clippy 全过

## 用户裁决记录

- **M36 范围**：数据类型扩展 + SQL 函数扩充 + 跨平台验证。
- **跳过**：UPDATE/DELETE（需改存储层）、参数绑定、Windows CI。

---

### Task M36-0: 数据类型扩展（BOOLEAN/BLOB）

**Files:**
- Modify: `crates/embedded-database-sql/src/result.rs`
- Modify: `crates/embedded-database-sql/src/lexer.rs`
- Modify: `crates/embedded-database-sql/src/token.rs`
- Modify: `crates/embedded-database-sql/src/parser.rs`

**Interfaces:**
- Consumes: `Value`（M18-0）、`Token`（M16-0）
- Produces: `Value::Boolean(bool)`、`Value::Blob(Vec<u8>)`

- [ ] **Step 1: 扩展 Value 枚举**

```rust
pub enum Value {
    Int(i64),
    Float(f64),
    Text(String),
    Boolean(bool),
    Blob(Vec<u8>),
    Null,
}
```

- [ ] **Step 2: 词法分析器支持 TRUE/FALSE**

在 `token.rs` 增加 `True`/`False` 关键字，在 `lexer.rs` 中识别。

- [ ] **Step 3: 解析器支持布尔字面量**

在 `parser.rs` 的表达式解析中处理 `True`/`False`。

- [ ] **Step 4: 新增测试**

```rust
#[test]
fn 布尔字面量_true_false() { ... }

#[test]
fn 值序列化_布尔() { ... }

#[test]
fn 值序列化_blob() { ... }
```

- [ ] **Step 5: 运行测试**

Run: `cargo test -p embedded-database-sql 2>&1 | rg 'test result: ok' | head -1`
Expected: 68 passed（65 + 3）

---

### Task M36-1: SQL 函数扩充

**Files:**
- Modify: `crates/embedded-database-sql/src/function.rs`

**Interfaces:**
- Consumes: `ScalarFunction` trait（M32-0）、`Value`（M36-0）
- Produces: `lower`/`length`/`substr`/`abs`/`round`/`coalesce`

- [ ] **Step 1: 新增标量函数**

```rust
// lower(s) -> Text
// length(s) -> Int
// substr(s, start, len) -> Text
// abs(n) -> Int/Float
// round(n, d) -> Float
// coalesce(a, b, ...) -> 非 Null 第一个
```

- [ ] **Step 2: 注册到 BuiltinScalar**

在 `BuiltinAggregates::get` 旁边增加标量函数注册。

- [ ] **Step 3: 新增测试**

```rust
#[test]
fn 标量函数_lower_upper() { ... }

#[test]
fn 标量函数_length_substr() { ... }

#[test]
fn 标量函数_abs_round() { ... }

#[test]
fn 标量函数_coalesce() { ... }
```

- [ ] **Step 4: 运行测试**

Run: `cargo test -p embedded-database-sql 2>&1 | rg 'test result: ok' | head -1`
Expected: 72 passed（68 + 4）

---

### Task M36-2: 跨平台验证 + 操作手册更新

**Files:**
- Modify: `docs/superpowers/specs/2026-10-02-embedded-database-operations-manual.md`

**Interfaces:**
- Consumes: `FileDevice`（M4-1）
- Produces: 操作手册跨平台章节

- [ ] **Step 1: 验证 FileDevice macOS 路径**

Run: `cargo test -p embedded-database-os 2>&1 | rg 'test result: ok' | head -1`
Expected: 5 passed

- [ ] **Step 2: 操作手册补充跨平台章节**

在操作手册追加：

```markdown
## 9. 跨平台支持

| 平台 | 状态 | 说明 |
|------|------|------|
| Linux | ✅ | 主要目标平台 |
| macOS | ✅ | 开发验证（FileDevice） |
| Windows | ✅ | 理论支持（std::fs 跨平台），未验证 |
| LoongArch | ✅ | 信创路线（需实机验证） |
| 裸机 | ✅ | no_std 路径 |

### 构建命令

# Linux/macOS
cargo build --release

# LoongArch 交叉编译
cargo build --release --target loongarch64-unknown-linux-gnu
```

- [ ] **Step 3: 检查点**

Run: `rg -n '跨平台支持' docs/superpowers/specs/2026-10-02-embedded-database-operations-manual.md`
Expected: 有输出

---

### Task M36-3: 收尾验收

**Files:**
- Modify: `docs/superpowers/specs/2026-10-02-embedded-database-design.md`
- Modify: `docs/superpowers/specs/2026-10-02-embedded-database-delivery-checklist.md`
- Modify: `docs/superpowers/specs/2026-10-02-embedded-database-project-summary.md`

- [ ] **Step 1: 全量测试**

Run: `cargo test --workspace --features embedded-database-core/std 2>&1 | rg 'test result:' | rg -v '0 passed'`
Expected: 全部通过（298+）

- [ ] **Step 2: fmt + clippy**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets --features embedded-database-core/std -- -D warnings 2>&1 | tail -3`
Expected: 通过

- [ ] **Step 3: 更新交付文档**

在设计文档/delivery-checklist/project-summary 追加 M36 实施状态。

- [ ] **Step 4: 检查点**

Run: `rg -n 'M36 完成' docs/superpowers/specs/2026-10-02-embedded-database-design.md`
Expected: 有输出

---

## M36 验收标准

| # | 标准 | 验证方式 |
|---|------|----------|
| 1 | `Value` 增加 BOOLEAN/BLOB，68 SQL 测试 | M36-0 Step 5 |
| 2 | SQL 函数扩充（6 个标量函数），72 SQL 测试 | M36-1 Step 4 |
| 3 | 操作手册含跨平台章节 | M36-2 Step 3 |
| 4 | workspace 全绿 + fmt/clippy 全过 | M36-3 Step 2 |
| 5 | 交付文档含 M36 实施状态 | M36-3 Step 4 |

## 后续里程碑

M36 完成后：

- **M37**：UPDATE/DELETE（需改存储层，架构级变更）
- **M38**：参数绑定/预编译语句
- **M39**：LoongArch 实机验证（需硬件）
