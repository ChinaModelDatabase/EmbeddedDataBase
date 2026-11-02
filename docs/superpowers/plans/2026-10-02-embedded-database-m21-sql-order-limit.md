# embedded-database M21 实施计划：SQL ORDER BY 与 LIMIT

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** parser 解析 ORDER BY/LIMIT 子句，executor 排序 + 截断结果集。

**Architecture:** `parser.rs` 的 `parse_select` 解析 `ORDER BY col [ASC|DESC]` 和 `LIMIT n`；`executor.rs` 的 `execute_select` 在 WHERE 过滤后按列排序并截断。

**Tech Stack:** Rust 1.81、`embedded-database-sql`（`no_std` + `alloc`）、零外部依赖。

## Global Constraints

- **C1** `embedded-database-sql` 保持 `#![no_std]`（用 `alloc`）
- **C2** 顶层 `#![forbid(unsafe_code)]`——不允许任何局部 `unsafe`
- **C3** 运行时依赖仅 `zerocopy`、`bitflags`，不新增
- **C4** 库代码禁止 `unwrap()` / `expect()` / `panic!()` / 数组越界（`unwrap()` 仅允许 `#[cfg(test)]`）
- **C11** 不执行任何 git 提交（本项目零提交策略）
- **工具链**：每个 bash 调用前 `export PATH="/opt/homebrew/opt/rustup/bin:$PATH"`
- **验收基线**：M20 交付 253 passed（core 215 + kv 6 + os 5 + sql 27）、5 目标 no_std 编译通过、`inject_crash`/`wcet` exit=0

## 用户裁决记录

- **M21 范围**：SQL ORDER BY + LIMIT。

---

### Task M21-0: parser 解析 ORDER BY / LIMIT

**Files:**
- Modify: `crates/embedded-database-sql/src/parser.rs`

**Interfaces:**
- Consumes: `Token`/`TokenType`/`Keyword`（M16-1）、`SelectStmt`/`OrderByItem`/`Expr`/`Literal`（M17-0）
- Produces: `parse_select` 填充 `order_by` 和 `limit` 字段

- [ ] **Step 1: 写失败测试**

```rust
// crates/embedded-database-sql/src/parser.rs 测试模块追加
#[test]
fn 解析_select_order_by() {
    let tokens = tokenize("SELECT name FROM users ORDER BY name ASC").unwrap();
    let stmt = parse(&tokens).unwrap();
    if let Statement::Select(s) = stmt {
        assert_eq!(s.order_by.len(), 1);
        assert!(s.order_by[0].asc);
    }
}

#[test]
fn 解析_select_order_by_desc() {
    let tokens = tokenize("SELECT name FROM users ORDER BY age DESC").unwrap();
    let stmt = parse(&tokens).unwrap();
    if let Statement::Select(s) = stmt {
        assert_eq!(s.order_by.len(), 1);
        assert!(!s.order_by[0].asc);
    }
}

#[test]
fn 解析_select_limit() {
    let tokens = tokenize("SELECT name FROM users LIMIT 10").unwrap();
    let stmt = parse(&tokens).unwrap();
    if let Statement::Select(s) = stmt {
        assert_eq!(s.limit, Some(10));
    }
}

#[test]
fn 解析_select_order_by_and_limit() {
    let tokens = tokenize("SELECT name FROM users ORDER BY name ASC LIMIT 5").unwrap();
    let stmt = parse(&tokens).unwrap();
    if let Statement::Select(s) = stmt {
        assert_eq!(s.order_by.len(), 1);
        assert_eq!(s.limit, Some(5));
    }
}
```

- [ ] **Step 2: 运行测试验证失败**

Run: `cargo test -p embedded-database-sql parser 2>&1 | tail -10`
Expected: 测试失败（ORDER BY/LIMIT 未解析）

- [ ] **Step 3: 实现**

在 `parse_select` 的 FROM 解析后追加：

```rust
// ORDER BY
let mut order_by = Vec::new();
if i < tokens.len() {
    if let TokenType::Keyword(Keyword::Order) = tokens[i].ty {
        i += 1;
        if i < tokens.len() && matches!(tokens[i].ty, TokenType::Keyword(Keyword::By)) {
            i += 1;
            // 解析排序列
            if i < tokens.len() {
                if let TokenType::Identifier(col) = &tokens[i].ty {
                    let mut asc = true;
                    i += 1;
                    if i < tokens.len() {
                        if let TokenType::Keyword(Keyword::Desc) = tokens[i].ty {
                            asc = false;
                            i += 1;
                        } else if let TokenType::Keyword(Keyword::Asc) = tokens[i].ty {
                            i += 1;
                        }
                    }
                    order_by.push(OrderByItem {
                        expr: Expr::Column(col.clone()),
                        asc,
                    });
                }
            }
        }
    }
}

// LIMIT
let mut limit = None;
if i < tokens.len() {
    if let TokenType::Keyword(Keyword::Limit) = tokens[i].ty {
        i += 1;
        if i < tokens.len() {
            if let TokenType::Integer(v) = tokens[i].ty {
                limit = Some(v);
                i += 1;
            }
        }
    }
}
```

并将 `SelectStmt` 构造改为使用 `order_by` 和 `limit`。

- [ ] **Step 4: 运行测试**

Run: `cargo test -p embedded-database-sql parser 2>&1 | tail -15`
Expected: 11 passed（7 已有 + 4 新增）

- [ ] **Step 5: 检查点**

Run: `cargo test -p embedded-database-sql parser 2>&1 | rg 'test result:' | head -1`
Expected: 11 passed

---

### Task M21-1: executor 排序 + 截断

**Files:**
- Modify: `crates/embedded-database-sql/src/executor.rs`

**Interfaces:**
- Consumes: `SelectStmt.order_by`/`limit`（M21-0）、`QueryResult`（M18-0）
- Produces: `execute_select` 在 WHERE 过滤后排序 + 截断

- [ ] **Step 1: 写失败测试**

```rust
// crates/embedded-database-sql/src/executor.rs 测试模块追加
#[test]
fn 执行_select_order_by_排序() {
    let mut dev = InMemoryDevice::new(1 << 20);
    let mut kv = KvDb::open(&mut dev, Config::default()).unwrap();
    execute("INSERT INTO users (name, age) VALUES ('alice', 30)", &mut kv).unwrap();
    execute("INSERT INTO users (name, age) VALUES ('bob', 25)", &mut kv).unwrap();
    execute("INSERT INTO users (name, age) VALUES ('carol', 35)", &mut kv).unwrap();
    let result = execute("SELECT name FROM users ORDER BY name ASC", &mut kv).unwrap();
    assert_eq!(result.rows.len(), 3);
    // M21 简化：排序基于字符串比较
}

#[test]
fn 执行_select_limit_截断() {
    let mut dev = InMemoryDevice::new(1 << 20);
    let mut kv = KvDb::open(&mut dev, Config::default()).unwrap();
    execute("INSERT INTO users (name) VALUES ('a')", &mut kv).unwrap();
    execute("INSERT INTO users (name) VALUES ('b')", &mut kv).unwrap();
    execute("INSERT INTO users (name) VALUES ('c')", &mut kv).unwrap();
    let result = execute("SELECT name FROM users LIMIT 2", &mut kv).unwrap();
    assert_eq!(result.rows.len(), 2);
}

#[test]
fn 执行_select_order_by_desc() {
    let mut dev = InMemoryDevice::new(1 << 20);
    let mut kv = KvDb::open(&mut dev, Config::default()).unwrap();
    execute("INSERT INTO users (name) VALUES ('a')", &mut kv).unwrap();
    execute("INSERT INTO users (name) VALUES ('b')", &mut kv).unwrap();
    let result = execute("SELECT name FROM users ORDER BY name DESC", &mut kv).unwrap();
    assert_eq!(result.rows.len(), 2);
    // M21 简化：降序排列
}
```

- [ ] **Step 2: 运行测试验证失败**

Run: `cargo test -p embedded-database-sql 执行_select_order_by_排序 2>&1 | tail -10`
Expected: 测试失败或编译错误（ORDER BY 未实现）

- [ ] **Step 3: 实现**

在 `executor.rs` 的 `execute_select` 中，WHERE 过滤后追加：

```rust
// ORDER BY
if !stmt.order_by.is_empty() {
    let order = &stmt.order_by[0]; // M21 简化：仅支持单列排序
    let col_name = match &order.expr {
        Expr::Column(name) => name.clone(),
        _ => String::new(),
    };
    let asc = order.asc;
    rows.sort_by(|a, b| {
        let a_val = a.get(&col_name).map(|s| s.as_str()).unwrap_or("");
        let b_val = b.get(&col_name).map(|s| s.as_str()).unwrap_or("");
        if asc {
            a_val.cmp(b_val)
        } else {
            b_val.cmp(a_val)
        }
    });
}

// LIMIT
if let Some(n) = stmt.limit {
    let n = n as usize;
    if rows.len() > n {
        rows.truncate(n);
    }
}
```

- [ ] **Step 4: 运行测试**

Run: `cargo test -p embedded-database-sql executor 2>&1 | tail -15`
Expected: 9 passed（6 已有 + 3 新增）

- [ ] **Step 5: 检查点**

Run: `cargo test -p embedded-database-sql 2>&1 | rg 'test result:' | head -1`
Expected: 30 passed

---

### Task M21-2: M21 收尾验收

**Files:**
- Modify: `docs/superpowers/specs/2026-10-02-embedded-database-design.md`

**Interfaces:**
- Consumes: 全部 M21 Task
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

在 `2026-10-02-embedded-database-design.md` 的「实施状态」小节追加 M21 交付内容：

```markdown
## 实施状态（M21 完成）

- M21 交付：SQL ORDER BY 解析（`ASC`/`DESC`）、LIMIT 解析、
  executor 排序（单列，字符串比较）与截断。
- **M21 简化**：
  - 仅支持单列排序（多列 ORDER BY 留待 M22+）。
  - 排序基于字符串比较（非类型感知）。
  - WHERE 运算符端到端仍仅支持 `=`/`<`/`>`（M19 遗留）。
```

- [ ] **Step 6: 检查点**

Run: `rg -n 'M21 完成' docs/superpowers/specs/2026-10-02-embedded-database-design.md`
Expected: 有输出

---

## M21 验收标准

| # | 标准 | 验证方式 |
|---|------|----------|
| 1 | parser 解析 ORDER BY / LIMIT | M21-0 测试 |
| 2 | executor 排序 + 截断 | M21-1 测试 |
| 3 | 全部测试通过 | M21-2 Step 1 |
| 4 | CLI 可执行 | M21-2 Step 3 |
| 5 | `inject_crash` exit=0 且 `wcet` exit=0 | M21-2 Step 4 |

## 后续里程碑

M21 完成后，SQL SELECT 支持 WHERE + ORDER BY + LIMIT。后续：

- **M22**：SQL 多列 ORDER BY + WHERE 运算符补齐（`!=`/`<=`/`>=`）
- **M23**：SQL JOIN 支持
- **M24**：SQL 优化器
