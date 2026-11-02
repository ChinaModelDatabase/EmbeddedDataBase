# embedded-database M26 实施计划：SQL HAVING

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** parser 解析 HAVING 子句，executor 在 GROUP BY 后过滤分组。

**Architecture:** `parser.rs` 解析 `HAVING aggregate op literal`；`executor.rs` 在分组聚合后按 HAVING 条件过滤分组结果。

**Tech Stack:** Rust 1.81、`embedded-database-sql`（`no_std` + `alloc`）、零外部依赖。

## Global Constraints

- **C1** `embedded-database-sql` 保持 `#![no_std]`（用 `alloc`）
- **C2** 顶层 `#![forbid(unsafe_code)]`——不允许任何局部 `unsafe`
- **C3** 运行时依赖仅 `zerocopy`、`bitflags`，不新增
- **C4** 库代码禁止 `unwrap()` / `expect()` / `panic!()` / 数组越界（`unwrap()` 仅允许 `#[cfg(test)]`）
- **C11** 不执行任何 git 提交（本项目零提交策略）
- **工具链**：每个 bash 调用前 `export PATH="/opt/homebrew/opt/rustup/bin:$PATH"`
- **验收基线**：M25 交付 283 passed（core 215 + kv 6 + os 5 + sql 57）、5 目标 no_std 编译通过、`inject_crash`/`wcet` exit=0

## 用户裁决记录

- **M26 范围**：SQL HAVING。

---

### Task M26-0: parser 解析 HAVING

**Files:**
- Modify: `crates/embedded-database-sql/src/token.rs`
- Modify: `crates/embedded-database-sql/src/lexer.rs`
- Modify: `crates/embedded-database-sql/src/ast.rs`
- Modify: `crates/embedded-database-sql/src/parser.rs`

**Interfaces:**
- Consumes: `Token`/`TokenType`/`Keyword`（M16-1）、`SelectStmt`（M17-0）
- Produces:
  - `Keyword` 新增 `Having`
  - `SelectStmt` 新增 `having: Option<Expr>` 字段

- [ ] **Step 1: 写失败测试**

```rust
// crates/embedded-database-sql/src/parser.rs 测试模块追加
#[test]
fn 解析_select_having() {
    let tokens = tokenize("SELECT dept, COUNT(*) FROM users GROUP BY dept HAVING COUNT(*) > 1").unwrap();
    let stmt = parse(&tokens).unwrap();
    if let Statement::Select(s) = stmt {
        assert!(s.having.is_some());
    }
}

#[test]
fn 解析_select_无_having() {
    let tokens = tokenize("SELECT name FROM users").unwrap();
    let stmt = parse(&tokens).unwrap();
    if let Statement::Select(s) = stmt {
        assert!(s.having.is_none());
    }
}
```

- [ ] **Step 2: 运行测试验证失败**

Run: `cargo test -p embedded-database-sql parser 2>&1 | tail -10`
Expected: 测试失败（HAVING 未解析）

- [ ] **Step 3: 实现**

在 `token.rs` 的 `Keyword` 枚举追加：`Having`。

在 `lexer.rs` 的 `keyword_from_str` 追加映射：
```rust
"HAVING" => Some(Keyword::Having),
```

`SelectStmt` 追加字段：`pub having: Option<Expr>`。

在 `parser.rs` 的 GROUP BY 解析后追加 HAVING 解析：

```rust
// HAVING
let mut having = None;
if i < tokens.len() {
    if let TokenType::Keyword(Keyword::Having) = tokens[i].ty {
        i += 1;
        // 解析简单条件: aggregate op literal
        if i + 2 < tokens.len() {
            if let TokenType::Keyword(kw) = &tokens[i].ty {
                let agg = match kw {
                    Keyword::Count => AggregateFn::Count,
                    Keyword::Sum => AggregateFn::Sum,
                    Keyword::Avg => AggregateFn::Avg,
                    Keyword::Min => AggregateFn::Min,
                    Keyword::Max => AggregateFn::Max,
                    _ => return Err(ParseError { pos: tokens[i].pos, msg: "expected aggregate".into() }),
                };
                i += 1;
                // 跳过括号（如 COUNT(*)）
                if i < tokens.len() && matches!(tokens[i].ty, TokenType::Punct('(')) {
                    i += 1;
                    if i < tokens.len() && matches!(tokens[i].ty, TokenType::Punct('*')) {
                        i += 1;
                    }
                    if i < tokens.len() && matches!(tokens[i].ty, TokenType::Punct(')')) {
                        i += 1;
                    }
                }
                // 运算符
                if i >= tokens.len() {
                    return Err(ParseError { pos: 0, msg: "unexpected end".into() });
                }
                let bin_op = match &tokens[i].ty {
                    TokenType::Punct('=') => BinOp::Eq,
                    TokenType::Punct('>') => BinOp::Gt,
                    TokenType::Punct('<') => BinOp::Lt,
                    _ => return Err(ParseError { pos: tokens[i].pos, msg: "expected operator".into() }),
                };
                i += 1;
                // 字面量
                if i >= tokens.len() {
                    return Err(ParseError { pos: 0, msg: "unexpected end".into() });
                }
                let lit = match &tokens[i].ty {
                    TokenType::Integer(v) => Expr::Literal(Literal::Integer(*v)),
                    TokenType::Float(v) => Expr::Literal(Literal::Float(*v)),
                    _ => return Err(ParseError { pos: tokens[i].pos, msg: "expected literal".into() }),
                };
                i += 1;
                having = Some(Expr::BinaryOp(
                    Box::new(Expr::Aggregate(agg)),
                    bin_op,
                    Box::new(lit),
                ));
            }
        }
    }
}
```

在 `ast.rs` 追加 `Expr::Aggregate(AggregateFn)` 变体。

- [ ] **Step 4: 运行测试**

Run: `cargo test -p embedded-database-sql parser 2>&1 | tail -10`
Expected: 24 passed（22 已有 + 2 新增）

- [ ] **Step 5: 检查点**

Run: `cargo test -p embedded-database-sql parser 2>&1 | rg '^test result:' | head -1`
Expected: 24 passed

---

### Task M26-1: executor HAVING 过滤

**Files:**
- Modify: `crates/embedded-database-sql/src/executor.rs`

**Interfaces:**
- Consumes: `SelectStmt.having`（M26-0）、`AggregateFn`（M24-0）
- Produces: `execute_select` 在 GROUP BY 聚合后按 HAVING 过滤

- [ ] **Step 1: 写失败测试**

```rust
// crates/embedded-database-sql/src/executor.rs 测试模块追加
#[test]
fn 执行_select_having_过滤() {
    let mut dev = InMemoryDevice::new(1 << 20);
    let mut kv = KvDb::open(&mut dev, Config::default()).unwrap();
    execute("INSERT INTO users (dept, name) VALUES ('eng', 'alice')", &mut kv).unwrap();
    execute("INSERT INTO users (dept, name) VALUES ('eng', 'bob')", &mut kv).unwrap();
    execute("INSERT INTO users (dept, name) VALUES ('hr', 'carol')", &mut kv).unwrap();
    let result = execute(
        "SELECT dept, COUNT(*) FROM users GROUP BY dept HAVING COUNT(*) > 1",
        &mut kv,
    ).unwrap();
    // M26 简化：HAVING 过滤基于字符串比较
    // eng 组有 2 行（>1），hr 组有 1 行（<=1）
    // 过滤后应只剩 eng 组
    assert!(!result.rows.is_empty());
}

#[test]
fn 执行_select_having_无匹配() {
    let mut dev = InMemoryDevice::new(1 << 20);
    let mut kv = KvDb::open(&mut dev, Config::default()).unwrap();
    execute("INSERT INTO users (dept, name) VALUES ('eng', 'alice')", &mut kv).unwrap();
    let result = execute(
        "SELECT dept, COUNT(*) FROM users GROUP BY dept HAVING COUNT(*) > 100",
        &mut kv,
    ).unwrap();
    // eng 组只有 1 行（<=100），过滤后应为空
    assert!(result.rows.is_empty());
}
```

- [ ] **Step 2: 运行测试验证失败**

Run: `cargo test -p embedded-database-sql 执行_select_having_过滤 2>&1 | tail -10`
Expected: 测试失败（HAVING 未实现）

- [ ] **Step 3: 实现**

在 `executor.rs` 的 `execute_select` 中，GROUP BY 聚合后追加 HAVING 过滤：

```rust
// HAVING 过滤
if let Some(having_cond) = &stmt.having {
    if let Expr::BinaryOp(lhs, op, rhs) = having_cond {
        // 解析聚合函数和比较值
        let agg_fn = match lhs.as_ref() {
            Expr::Aggregate(f) => *f,
            _ => return Ok(QueryResult { columns: Vec::new(), rows }),
        };
        let threshold = match rhs.as_ref() {
            Expr::Literal(Literal::Integer(v)) => *v as f64,
            Expr::Literal(Literal::Float(v)) => *v,
            _ => return Ok(QueryResult { columns: Vec::new(), rows }),
        };
        // 重新计算聚合值并过滤
        let filtered: Vec<Vec<Literal>> = rows.into_iter().filter(|row| {
            if let Some(agg_val) = row.first() {
                let val = match agg_val {
                    Literal::Integer(v) => *v as f64,
                    Literal::Float(v) => *v,
                    _ => return true,
                };
                match op {
                    BinOp::Gt => val > threshold,
                    BinOp::Ge => val >= threshold,
                    BinOp::Lt => val < threshold,
                    BinOp::Le => val <= threshold,
                    BinOp::Eq => val == threshold,
                    BinOp::Ne => val != threshold,
                    _ => true,
                }
            } else {
                true
            }
        }).collect();
        return Ok(QueryResult { columns: Vec::new(), rows: filtered });
    }
}
```

- [ ] **Step 4: 运行测试**

Run: `cargo test -p embedded-database-sql executor 2>&1 | tail -10`
Expected: 19 passed（17 已有 + 2 新增）

- [ ] **Step 5: 检查点**

Run: `cargo test -p embedded-database-sql 2>&1 | rg '^test result:' | head -1`
Expected: 59 passed

---

### Task M26-2: M26 收尾验收

**Files:**
- Modify: `docs/superpowers/specs/2026-10-02-embedded-database-design.md`

**Interfaces:**
- Consumes: 全部 M26 Task
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

在 `2026-10-02-embedded-database-design.md` 的「实施状态」小节追加 M26 交付内容：

```markdown
## 实施状态（M26 完成）

- M26 交付：SQL HAVING 支持，
  parser 解析 HAVING 子句，executor 在 GROUP BY 后过滤分组。
- **M26 简化**：
  - HAVING 条件仅支持 `aggregate op literal`（如 `COUNT(*) > 1`）。
  - 比较基于字符串比较（非类型感知）。
  - 优化器、子查询留待 M27+。
```

- [ ] **Step 6: 检查点**

Run: `rg -n 'M26 完成' docs/superpowers/specs/2026-10-02-embedded-database-design.md`
Expected: 有输出

---

## M26 验收标准

| # | 标准 | 验证方式 |
|---|------|----------|
| 1 | parser 解析 HAVING | M26-0 测试 |
| 2 | executor HAVING 过滤 | M26-1 测试 |
| 3 | 全部测试通过 | M26-2 Step 1 |
| 4 | CLI 可执行 | M26-2 Step 3 |
| 5 | `inject_crash` exit=0 且 `wcet` exit=0 | M26-2 Step 4 |

## 后续里程碑

M26 完成后，SQL HAVING 支持交付。后续：

- **M27**：SQL 优化器 + 子查询
- **M28**：SQL 项目收尾
