# embedded-database M32 实施计划：可扩展架构

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 设计 embedded-database 可扩展架构——SQL 函数/类型 trait 扩展点 + 存储后端文档化。

**Architecture:** 受 embedded-database 约束限制（C1 no_std、C2 零 unsafe、C7 确定性时延），扩展机制必须编译期静态分发、trait 约束、无动态注册。3 个扩展点：SQL 函数层（`ScalarFunction`/`AggregateFunction`）、SQL 类型层（`SqlType`）、存储层（`BlockDevice` 文档化）。

**Tech Stack:** Rust 1.81、`embedded-database-sql`（alloc）、`embedded-database-core`（no_std）。

## Global Constraints

- **C1** `embedded-database-core` 保持 `#![no_std]`；`embedded-database-sql` 允许 `alloc`
- **C2** 顶层 `#![forbid(unsafe_code)]`——不允许任何局部 `unsafe`
- **C3** 运行时依赖仅 `zerocopy`、`bitflags`，不新增
- **C4** 库代码禁止 `unwrap()` / `expect()` / `panic!()` / 数组越界（`unwrap()` 仅允许 `#[cfg(test)]`；bench bin 可用 `unwrap`）
- **C11** 不执行任何 git 提交（本项目零提交策略）
- **工具链**：每个 bash 调用前 `export PATH="/opt/homebrew/opt/rustup/bin:$PATH"`
- **验收基线**：M30 交付 291 passed（core 216 + kv 8 + os 5 + sql 62）、fmt/clippy 全过

## 用户裁决记录

- **M32 范围**：可扩展架构——SQL 函数/类型 trait + 存储后端文档化。
- **M32-0 方案**：`ScalarFunction`/`AggregateFunction` trait，`Executor` 泛型参数。
- **M32-1 方案**：`SqlType` trait。
- **M32-2 方案**：`BlockDevice` 实现指南。

---

### Task M32-0: SQL 函数扩展 trait

**Files:**
- Create: `crates/embedded-database-sql/src/function.rs`
- Modify: `crates/embedded-database-sql/src/executor.rs`
- Modify: `crates/embedded-database-sql/src/lib.rs`

**Interfaces:**
- Consumes: `Value`（M18-0）、`SqlError`（M17-0）
- Produces:
  - `trait ScalarFunction`：`name()`/`arity()`/`eval(args) -> Value`
  - `trait AggregateFunction`：`name()`/`init()`/`step(state, arg)`/`finalize(state) -> Value`
  - `BuiltinFunctions`：内置函数实现

**设计原则**：
- 编译期静态分发——不用 `dyn Fn`/`Box<dyn>`
- trait 约束——扩展实现者必须满足永不 panic、预算化
- 无动态注册——不用全局注册表

- [ ] **Step 1: 定义 function.rs**

```rust
//! SQL 函数扩展 trait。
//!
//! 设计原则：编译期静态分发、trait 约束、无动态注册。
//! 扩展实现者必须满足：永不 panic、时延有界。

use crate::result::{SqlError, Value};

/// 标量函数 trait。
///
/// 实现者必须满足：
/// - `eval` 永不 panic，错误通过 `Err` 返回
/// - `eval` 时延有界（输入规模固定时）
pub trait ScalarFunction {
    /// 函数名（小写，如 "upper"）。
    fn name(&self) -> &str;

    /// 参数个数。`None` 表示变参。
    fn arity(&self) -> Option<usize>;

    /// 求值。`args.len()` 必须与 `arity()` 一致（变参时任意）。
    fn eval(&self, args: &[Value]) -> Result<Value, SqlError>;
}

/// 聚合函数 trait。
///
/// 实现者必须满足：
/// - `step`/`finalize` 永不 panic
/// - 时延有界
pub trait AggregateFunction {
    /// 函数名（小写，如 "sum"）。
    fn name(&self) -> &str;

    /// 初始化聚合状态。
    fn init(&self) -> AggregateState;

    /// 累加一个值到状态。
    fn step(&self, state: &mut AggregateState, arg: Value) -> Result<(), SqlError>;

    /// 从状态计算最终值。
    fn finalize(&self, state: &AggregateState) -> Value;
}

/// 聚合状态。用 enum 避免动态分配。
#[derive(Debug, Clone)]
pub enum AggregateState {
    /// 无状态（如 COUNT）。
    None,
    /// 数值累加（如 SUM/AVG）。
    Sum { count: u64, total: f64 },
    /// 最小值（如 MIN）。
    Min(f64),
    /// 最大值（如 MAX）。
    Max(f64),
    /// 自定义状态（扩展用）。
    Custom(u64),
}

/// 内置标量函数。
pub struct BuiltinScalar;

impl ScalarFunction for BuiltinScalar {
    fn name(&self) -> &str { "upper" }
    fn arity(&self) -> Option<usize> { Some(1) }
    fn eval(&self, args: &[Value]) -> Result<Value, SqlError> {
        match args.first() {
            Some(Value::Text(s)) => Ok(Value::Text(s.to_uppercase())),
            _ => Err(SqlError::InvalidArgument),
        }
    }
}

/// 内置聚合函数注册表。
pub struct BuiltinAggregates;

impl BuiltinAggregates {
    /// 按名称获取聚合函数。
    pub fn get(name: &str) -> Option<&'static dyn AggregateFunction> {
        match name {
            "count" => Some(&CountFn),
            "sum" => Some(&SumFn),
            "avg" => Some(&AvgFn),
            "min" => Some(&MinFn),
            "max" => Some(&MaxFn),
            _ => None,
        }
    }
}

struct CountFn;
struct SumFn;
struct AvgFn;
struct MinFn;
struct MaxFn;

impl AggregateFunction for CountFn {
    fn name(&self) -> &str { "count" }
    fn init(&self) -> AggregateState { AggregateState::Sum { count: 0, total: 0.0 } }
    fn step(&self, state: &mut AggregateState, _arg: Value) -> Result<(), SqlError> {
        if let AggregateState::Sum { count, .. } = state { *count += 1; }
        Ok(())
    }
    fn finalize(&self, state: &AggregateState) -> Value {
        match state {
            AggregateState::Sum { count, .. } => Value::Int(*count as i64),
            _ => Value::Int(0),
        }
    }
}

impl AggregateFunction for SumFn {
    fn name(&self) -> &str { "sum" }
    fn init(&self) -> AggregateState { AggregateState::Sum { count: 0, total: 0.0 } }
    fn step(&self, state: &mut AggregateState, arg: Value) -> Result<(), SqlError> {
        if let (AggregateState::Sum { count, total }, Value::Int(v)) = (state, arg) {
            *count += 1; *total += v as f64;
        }
        Ok(())
    }
    fn finalize(&self, state: &AggregateState) -> Value {
        match state {
            AggregateState::Sum { total, .. } => Value::Int(*total as i64),
            _ => Value::Int(0),
        }
    }
}

impl AggregateFunction for AvgFn {
    fn name(&self) -> &str { "avg" }
    fn init(&self) -> AggregateState { AggregateState::Sum { count: 0, total: 0.0 } }
    fn step(&self, state: &mut AggregateState, arg: Value) -> Result<(), SqlError> {
        if let (AggregateState::Sum { count, total }, Value::Int(v)) = (state, arg) {
            *count += 1; *total += v as f64;
        }
        Ok(())
    }
    fn finalize(&self, state: &AggregateState) -> Value {
        match state {
            AggregateState::Sum { count, total } if *count > 0 => Value::Float(*total / *count as f64),
            _ => Value::Int(0),
        }
    }
}

impl AggregateFunction for MinFn {
    fn name(&self) -> &str { "min" }
    fn init(&self) -> AggregateState { AggregateState::Min(f64::MAX) }
    fn step(&self, state: &mut AggregateState, arg: Value) -> Result<(), SqlError> {
        if let (AggregateState::Min(m), Value::Int(v)) = (state, arg) {
            if (*v as f64) < *m { *m = *v as f64; }
        }
        Ok(())
    }
    fn finalize(&self, state: &AggregateState) -> Value {
        match state {
            AggregateState::Min(m) if *m < f64::MAX => Value::Int(*m as i64),
            _ => Value::Null,
        }
    }
}

impl AggregateFunction for MaxFn {
    fn name(&self) -> &str { "max" }
    fn init(&self) -> AggregateState { AggregateState::Max(f64::MIN) }
    fn step(&self, state: &mut AggregateState, arg: Value) -> Result<(), SqlError> {
        if let (AggregateState::Max(m), Value::Int(v)) = (state, arg) {
            if (*v as f64) > *m { *m = *v as f64; }
        }
        Ok(())
    }
    fn finalize(&self, state: &AggregateState) -> Value {
        match state {
            AggregateState::Max(m) if *m > f64::MIN => Value::Int(*m as i64),
            _ => Value::Null,
        }
    }
}
```

- [ ] **Step 2: 修改 executor.rs 使用 BuiltinAggregates**

将 `compute_aggregate_value` 改为调用 `BuiltinAggregates::get(name)` + `init/step/finalize`。

- [ ] **Step 3: 运行 SQL 测试**

Run: `cargo test -p embedded-database-sql 2>&1 | rg 'test result:' | head -1`
Expected: 62 passed

- [ ] **Step 4: 新增自定义函数测试**

```rust
#[test]
fn 自定义标量函数_upper() {
    // 实现 ScalarFunction trait，调用 eval
}

#[test]
fn 内置聚合函数_sum() {
    // 调用 BuiltinAggregates::get("sum") + init/step/finalize
}
```

Run: `cargo test -p embedded-database-sql 2>&1 | rg 'test result:' | head -1`
Expected: 64 passed

- [ ] **Step 5: 检查点**

Run: `cargo fmt --all --check && cargo clippy -p embedded-database-sql --all-targets -- -D warnings 2>&1 | tail -3`
Expected: 通过

---

### Task M32-1: SQL 类型扩展 trait

**Files:**
- Create: `crates/embedded-database-sql/src/typext.rs`
- Modify: `crates/embedded-database-sql/src/lib.rs`

**Interfaces:**
- Consumes: `Value`（M18-0）
- Produces: `trait SqlType`：`type_name()`/`parse(s) -> Value`/`format(v) -> String`

- [ ] **Step 1: 定义 typext.rs**

```rust
//! SQL 类型扩展 trait。

use crate::result::Value;

/// SQL 类型 trait。
///
/// 扩展实现者必须满足：`parse`/`format` 永不 panic。
pub trait SqlType {
    /// 类型名（如 "date"、"uuid"）。
    fn type_name(&self) -> &str;

    /// 从字符串解析值。
    fn parse(&self, s: &str) -> Result<Value, crate::result::SqlError>;

    /// 格式化值为字符串。
    fn format(&self, v: &Value) -> alloc::string::String;
}
```

- [ ] **Step 2: 新增测试**

```rust
#[test]
fn 自定义类型_date() {
    // 实现 SqlType trait，parse "2026-10-05" 为 Value::Text
}
```

- [ ] **Step 3: 检查点**

Run: `cargo test -p embedded-database-sql 2>&1 | rg 'test result:' | head -1`
Expected: 65 passed

---

### Task M32-2: 存储后端文档化

**Files:**
- Modify: `docs/superpowers/specs/2026-10-02-embedded-database-design.md`

**Interfaces:**
- Consumes: `BlockDevice` trait（M4-0）
- Produces: 存储后端实现指南

- [ ] **Step 1: 在设计文档追加实现指南**

```markdown
## 存储后端实现指南

`BlockDevice` trait 是 embedded-database 的存储扩展点。实现者必须满足：

1. **越界安全**：`offset + buf.len() > size()` 时返回 `Error::InvalidArgument`，不得 panic。
2. **写穿透**：`write_at` 后数据对 `read_at` 立即可见。
3. **持久性边界**：`sync()` 返回后，此前写入已落到稳定介质。
4. **非阻塞**：设备忙时返回 `Error::Busy`，永不阻塞、永不自旋。

### 内存后端

`InMemoryDevice`（M4-1）是参考实现，支持故障注入。

### 文件后端

`FileDevice`（M4-1）是生产实现，用 `std::fs::File` + `seek`。

### 硬件后端（信创）

实现 `BlockDevice` trait，映射到硬件寄存器/DMA：
- `size()` 返回设备容量
- `read_at`/`write_at` 用 MMIO 或 DMA
- `sync()` 用内存屏障 + flush 指令

### 压缩后端

在 `BlockDevice` 之上包装压缩层：
- `write_at` 压缩后写入
- `read_at` 读取后解压
- 注意：压缩增加时延，需在 WCET 预算中考虑
```

- [ ] **Step 2: 检查点**

Run: `rg -n '存储后端实现指南' docs/superpowers/specs/2026-10-02-embedded-database-design.md`
Expected: 有输出

---

## M32 验收标准

| # | 标准 | 验证方式 |
|---|------|----------|
| 1 | `ScalarFunction`/`AggregateFunction` trait 定义 | M32-0 Step 1 |
| 2 | 现有 SQL 测试 62 passed | M32-0 Step 3 |
| 3 | 新增 2 个自定义函数测试（64 passed） | M32-0 Step 4 |
| 4 | `SqlType` trait 定义 + 测试（65 passed） | M32-1 |
| 5 | 存储后端实现指南 | M32-2 |
| 6 | workspace 全绿 + fmt/clippy 全过 | M32-2 收尾 |

## 后续里程碑

M32 完成后：

- **M33**：LoongArch 实机验证（需硬件）
- **M34**：项目收尾（交付文档更新）
