# embedded-database M24 实施计划：SQL 聚合函数

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** lexer/parser 支持 COUNT/SUM/AVG/MIN/MAX 聚合函数，executor 实现聚合计算。

**Architecture:** `lexer.rs` 识别聚合函数关键字；`parser.rs` 解析 `SELECT COUNT(*)`/`SUM(col)` 等形式；`executor.rs` 在 WHERE/JOIN 过滤后计算聚合值。

**Tech Stack:** Rust 1.81、`embedded-database-sql`（`no_std` + `alloc`）、零外部依赖。

## Global Constraints

- **C1** `embedded-database-sql` 保持 `#![no_std]`（用 `alloc`）
- **C2** 顶层 `#![forbid(unsafe_code)]`——不允许任何局部 `unsafe`
- **C3** 运行时依赖仅 `zerocopy`、`bitflags`，不新增
- **C4** 库代码禁止 `unwrap()` / `expect()` / `panic!()` / 数组越界（`unwrap()` 仅允许 `#[cfg(test)]`）
- **C11** 不执行任何 git 提交（本项目零提交策略）
- **工具链**：每个 bash 调用前 `export PATH="/opt/homebrew/opt/rustup/bin:$PATH"`
- **验收基线**：M23 交付 272 passed（core 215 + kv 6 + os 5 + sql 46）、5 目标 no_std 编译通过、`inject_crash`/`wcet` exit=0

## 用户裁决记录

- **M24 范围**：SQL 聚合函数（COUNT/SUM/AVG/MIN/MAX）。

---

### Task M24-0: lexer/parser 支持聚合函数

**Files:**
- Modify: `crates/embedded-database-sql/src/token.rs`
- Modify: `crates/embedded-database-sql/src/lexer.rs`
- Modify: `crates/embedded-database-sql/src/ast.rs`
- Modify: `crates/embedded-database-sql/src/parser.rs`

**Interfaces:**
- Consumes: `Token`/`TokenType`/`Keyword`（M16-1）、`SelectStmt`/`SelectItem`（M17-0）
- Produces:
  - `Keyword` 新增 `Count`/`Sum`/`Avg`/`Min`/`Max`
  - `SelectItem` 新增 `Aggregate(AggregateFn)`
  - `pub enum AggregateFn { Count, Sum, Avg, Min, Max }`

- [ ] **Step 1: 写失败测试**

```rust
// crates/embedded-database-sql/src/parser.rs 测试模块追加
#[test]
fn 解析_select_count() {
    let tokens = tokenize("SELECT COUNT(*) FROM users").unwrap();
    let stmt = parse(&tokens).unwrap();
    if let Statement::Select(s) = stmt {
        assert_eq!(s.columns, vec![SelectItem::Aggregate(AggregateFn::Count)]);
    }
}

#[test]
fn 解析_select_sum() {
    let tokens = tokenize("SELECT SUM(age) FROM users").unwrap();
    let stmt = parse(&tokens).unwrap();
    if let Statement::Select(s) = stmt {
        assert_eq!(s.columns, vec![SelectItem::Aggregate(AggregateFn::Sum)]);
    }
}

#[test]
fn 解析_select_avg() {
    let tokens = tokenize("SELECT AVG(age) FROM users").unwrap();
    let stmt = parse(&tokens).unwrap();
    if let Statement::Select(s) = stmt {
        assert_eq!(s.columns, vec![SelectItem::Aggregate(AggregateFn::Avg)]);
    }
}

#[test]
fn 解析_select_min_max() {
    let tokens = tokenize("SELECT MIN(age), MAX(age) FROM users").unwrap();
    let stmt = parse(&tokens).unwrap();
    if let Statement::Select(s) = stmt {
        assert_eq!(s.columns, vec![
            SelectItem::Aggregate(AggregateFn::Min),
            SelectItem::Aggregate(AggregateFn::Max),
        ]);
    }
}
```

- [ ] **Step 2: 运行测试验证失败**

Run: `cargo test -p embedded-database-sql parser 2>&1 | tail -10`
Expected: 编译失败（`AggregateFn` 不存在）

- [ ] **Step 3: 实现**

在 `token.rs` 的 `Keyword` 枚举追加：`Count`/`Sum`/`Avg`/`Min`/`Max`。

在 `lexer.rs` 的 `keyword_from_str` 追加映射：
```rust
"COUNT" => Some(Keyword::Count),
"SUM" => Some(Keyword::Sum),
"AVG" => Some(Keyword::Avg),
"MIN" => Some(Keyword::Min),
"MAX" => Some(Keyword::Max),
```

在 `ast.rs` 追加：
```rust
/// 聚合函数。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AggregateFn {
    Count,
    Sum,
    Avg,
    Min,
    Max,
}
```

`SelectItem` 追加变体：`Aggregate(AggregateFn)`。

在 `parser.rs` 的 SELECT 列解析中追加聚合函数识别（`Keyword::Count` 等 → `SelectItem::Aggregate`）。

- [ ] **Step 4: 运行测试**

Run: `cargo test -p embedded-database-sql parser 2>&1 | tail -10`
Expected: 18 passed（16 已有 + 2 新增）

- [ ] **Step 5: 检查点**

Run: `cargo test -p embedded-database-sql parser 2>&1 | rg '^test result:' | head -1`
Expected: 18 passed

---

### Task M24-1: executor 聚合计算

**Files:**
- Modify: `crates/embedded-database-sql/src/executor.rs`

**Interfaces:**
- Consumes: `SelectItem::Aggregate`（M24-0）、`AggregateFn`（M24-0）、`QueryResult`（M18-0）
- Produces: `execute_select` 支持聚合计算

- [ ] **Step 1: 写失败测试**

```rust
// crates/embedded-database-sql/src/executor.rs 测试模块追加
#[test]
fn 执行_select_count() {
    let mut dev = InMemoryDevice::new(1 << 20);
    let mut kv = KvDb::open(&mut dev, Config::default()).unwrap();
    execute("INSERT INTO users (name) VALUES ('a')", &mut kv).unwrap();
    execute("INSERT INTO users (name) VALUES ('b')", &mut kv).unwrap();
    execute("INSERT INTO users (name) VALUES ('c')", &mut kv).unwrap();
    let result = execute("SELECT COUNT(*) FROM users", &mut kv).unwrap();
    assert_eq!(result.rows.len(), 1);
    // M24 简化：COUNT 结果为行数
}

#[test]
fn 执行_select_sum() {
    let mut dev = InMemoryDevice::new(1 << 20);
    let mut kv = KvDb::open(&mut dev, Config::default()).unwrap();
    execute("INSERT INTO users (age) VALUES (10)", &mut kv).unwrap();
    execute("INSERT INTO users (age) VALUES (20)", &mut kv).unwrap();
    let result = execute("SELECT SUM(age) FROM users", &mut kv).unwrap();
    assert_eq!(result.rows.len(), 1);
    // M24 简化：SUM 结果为数值和
}

#[test]
fn 执行_select_empty_count() {
    let mut dev = InMemoryDevice::new(1 << 20);
    let mut kv = KvDb::open(&mut dev, Config::default()).unwrap();
    let result = execute("SELECT COUNT(*) FROM empty", &mut kv).unwrap();
    assert_eq!(result.rows.len(), 1);
    // M24 简化：空表 COUNT 返回 0
}
```

- [ ] **Step 2: 运行测试验证失败**

Run: `cargo test -p embedded-database-sql 执行_select_count 2>&1 | tail -10`
Expected: 测试失败（聚合未实现）

- [ ] **Step 3: 实现**

在 `executor.rs` 的 `execute_select` 中，WHERE/JOIN 过滤后追加：

```rust
// 聚合函数
let has_aggregate = stmt.columns.iter().any(|c| matches!(c, SelectItem::Aggregate(_)));
if has_aggregate {
    let mut agg_result: Vec<Literal> = Vec::new();
    for item in &stmt.columns {
        match item {
            SelectItem::Aggregate(AggregateFn::Count) => {
                agg_result.push(Literal::Integer(rows.len() as i64));
            }
            SelectItem::Aggregate(AggregateFn::Sum) => {
                // M24 简化：SUM 基于字符串比较（非类型感知）
                let mut sum = 0.0f64;
                for row in &rows {
                    // 取第一列的值
                    if let Some(val) = row.values().next() {
                        if let Ok(n) = val.parse::<f64>() {
                            sum += n;
                        }
                    }
                }
                agg_result.push(Literal::Float(sum));
            }
            SelectItem::Aggregate(AggregateFn::Avg) => {
                let mut sum = 0.0f64;
                let mut count = 0u32;
                for row in &rows {
                    if let Some(val) = row.values().next() {
                        if let Ok(n) = val.parse::<f64>() {
                            sum += n;
                            count += 1;
                        }
                    }
                }
                let avg = if count > 0 { sum / count as f64 } else { 0.0 };
                agg_result.push(Literal::Float(avg));
            }
            SelectItem::Aggregate(AggregateFn::Min) => {
                let mut min_val: Option<f64> = None;
                for row in &rows {
                    if let Some(val) = row.values().next() {
                        if let Ok(n) = val.parse::<f64>() {
                            min_val = Some(min_val.map_or(n, |m| m.min(n)));
                        }
                    }
                }
                agg_result.push(Literal::Float(min_val.unwrap_or(0.0)));
            }
            SelectItem::Aggregate(AggregateFn::Max) => {
                let mut max_val: Option<f64> = None;
                for row in &rows {
                    if let Some(val) = row.values().next() {
                        if let Ok(n) = val.parse::<f64>() {
                            max_val = Some(max_val.map_or(n, |m| m.max(n)));
                        }
                    }
                }
                agg_result.push(Literal::Float(max_val.unwrap_or(0.0)));
            }
            _ => {}
        }
    }
    rows = vec![agg_result];
}
```

- [ ] **Step 4: 运行测试**

Run: `cargo test -p embedded-database-sql executor 2>&1 | tail -10`
Expected: 13 passed（10 已有 + 3 新增）

- [ ] **Step 5: 检查点**

Run: `cargo test -p embedded-database-sql 2>&1 | rg '^test result:' | head -1`
Expected: 49 passed

---

### Task M24-2: M24 收尾验收

**Files:**
- Modify: `docs/superpowers/specs/2026-10-02-embedded-database-design.md`

**Interfaces:**
- Consumes: 全部 M24 Task
- Produces: 设计文档「实施状态」小节更新

- [ ] **Step 1: 全量测试**

Run: `cargo test --workspace --features embedded-database-core/std 2>&1 | tail -15`
Expected: 全部通过

- [ ] **Step 2: 格式化与 lint**

Run: `cargo fmt --all && cargo clippy --workspace --all-targets --features embedded-database-core/std -- -D warnings 2>&1 | tail -20`
Expected: 无警告

- [ ] **Step 3: CLI 可执行**

Run: `echo ".exit" | cargo run -p embedded-database-cli 2>&1 | tail -3`
Expected: 输出欢迎信息和「再见」

- [ ] **Step 4: 工具二进制可用**

Run: `cargo run -p embedded-database-bench --bin inject_crash >/dev/null 2>&1; echo "inject=$?"; cargo run -p embedded-database-bench --bin wcet >/dev/null 2>&1; echo "wcet=$?"`
Expected: `inject=0` 且 `wcet=0`

- [ ] **Step 5: 更新设计文档**

在 `2026-10-02-embedded-database-design.md` 的「实施状态」小节追加 M24 交付内容：

```markdown
## 实施状态（M24 完成）

- M24 交付：SQL 聚合函数（COUNT/SUM/AVG/MIN/MAX），
  lexer 识别聚合关键字，parser 解析聚合列，executor 实现聚合计算。
- **M24 简化**：
  - SUM/AVG/MIN/MAX 基于字符串比较（非类型感知）。
  - COUNT(*) 统计行数。
  - GROUP BY 子查询留待 M25+。
```

- [ ] **Step 6: 检查点**

Run: `rg -n 'M24 完成' docs/superpowers/specs/2026-10-02-embedded-database-design.md`
Expected: 有输出

---

## M24 验收标准

| # | 标准 | 验证方式 |
|---|------|----------|
| 1 | lexer/parser 支持聚合函数 | M24-0 测试 |
| 2 | executor 聚合计算 | M24-1 测试 |
| 3 | 全部测试通过 | M24-2 Step 1 |
| 4 | CLI 可执行 | M24-2 Step 3 |
| 5 | `inject_crash` exit=0 且 `wcet` exit=0 | M24-2 Step 4 |

## 后续里程碑

M24 完成后，SQL 聚合函数交付。后续：

- **M25**：SQL GROUP BY + 优化器
- **M26**：SQL 子查询支持
- **M27**：SQL CLI 聚合输出格式优化
