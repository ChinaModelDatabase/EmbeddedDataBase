# embedded-database M23 实施计划：SQL JOIN 支持

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** parser 解析 JOIN 子句，executor 实现嵌套循环连接。

**Architecture:** `parser.rs` 解析 `FROM t1 JOIN t2 ON t1.col = t2.col`；`executor.rs` 实现嵌套循环连接（外层表逐行 × 内层表逐行，匹配条件则拼接行）。

**Tech Stack:** Rust 1.81、`embedded-database-sql`（`no_std` + `alloc`）、零外部依赖。

## Global Constraints

- **C1** `embedded-database-sql` 保持 `#![no_std]`（用 `alloc`）
- **C2** 顶层 `#![forbid(unsafe_code)]`——不允许任何局部 `unsafe`
- **C3** 运行时依赖仅 `zerocopy`、`bitflags`，不新增
- **C4** 库代码禁止 `unwrap()` / `expect()` / `panic!()` / 数组越界（`unwrap()` 仅允许 `#[cfg(test)]`）
- **C11** 不执行任何 git 提交（本项目零提交策略）
- **工具链**：每个 bash 调用前 `export PATH="/opt/homebrew/opt/rustup/bin:$PATH"`
- **验收基线**：M22 交付 267 passed（core 215 + kv 6 + os 5 + sql 41）、5 目标 no_std 编译通过、`inject_crash`/`wcet` exit=0

## 用户裁决记录

- **M23 范围**：SQL JOIN 支持。

---

### Task M23-0: parser 解析 JOIN

**Files:**
- Modify: `crates/embedded-database-sql/src/ast.rs`
- Modify: `crates/embedded-database-sql/src/parser.rs`

**Interfaces:**
- Consumes: `Token`/`TokenType`/`Keyword`（M16-1）、`SelectStmt`（M17-0）
- Produces:
  - `SelectStmt` 新增 `joins: Vec<JoinClause>` 字段
  - `pub struct JoinClause { pub table: String, pub alias: Option<String>, pub on_left: String, pub on_right: String }`

- [ ] **Step 1: 写失败测试**

```rust
// crates/embedded-database-sql/src/parser.rs 测试模块追加
#[test]
fn 解析_select_join() {
    let tokens = tokenize("SELECT users.name FROM users JOIN orders ON users.id = orders.user_id").unwrap();
    let stmt = parse(&tokens).unwrap();
    if let Statement::Select(s) = stmt {
        assert_eq!(s.joins.len(), 1);
        assert_eq!(s.joins[0].table, "orders");
        assert_eq!(s.joins[0].on_left, "users.id");
        assert_eq!(s.joins[0].on_right, "orders.user_id");
    }
}

#[test]
fn 解析_select_无join() {
    let tokens = tokenize("SELECT name FROM users").unwrap();
    let stmt = parse(&tokens).unwrap();
    if let Statement::Select(s) = stmt {
        assert!(s.joins.is_empty());
    }
}
```

- [ ] **Step 2: 运行测试验证失败**

Run: `cargo test -p embedded-database-sql parser 2>&1 | tail -10`
Expected: 测试失败（JOIN 未解析）

- [ ] **Step 3: 实现**

在 `ast.rs` 追加：

```rust
/// JOIN 子句。
#[derive(Debug, Clone, PartialEq)]
pub struct JoinClause {
    pub table: String,
    pub alias: Option<String>,
    pub on_left: String,
    pub on_right: String,
}
```

`SelectStmt` 追加字段：`pub joins: Vec<JoinClause>`。

在 `parser.rs` 的 FROM 解析后追加 JOIN 解析：

```rust
// JOIN
let mut joins = Vec::new();
while i < tokens.len() {
    if let TokenType::Keyword(Keyword::Join) = tokens[i].ty {
        i += 1;
        // 表名
        if i >= tokens.len() || !matches!(tokens[i].ty, TokenType::Identifier(_)) {
            return Err(ParseError { pos: tokens.get(i).map(|t| t.pos).unwrap_or(0), msg: "expected table name".into() });
        }
        let table = match &tokens[i].ty {
            TokenType::Identifier(n) => n.clone(),
            _ => unreachable!(),
        };
        i += 1;
        // ON 条件
        if i >= tokens.len() || !matches!(tokens[i].ty, TokenType::Keyword(Keyword::On)) {
            return Err(ParseError { pos: tokens.get(i).map(|t| t.pos).unwrap_or(0), msg: "expected ON".into() });
        }
        i += 1;
        // left = right
        if i + 2 >= tokens.len() {
            return Err(ParseError { pos: 0, msg: "unexpected end".into() });
        }
        let on_left = match &tokens[i].ty {
            TokenType::Identifier(n) => n.clone(),
            _ => return Err(ParseError { pos: tokens[i].pos, msg: "expected column".into() }),
        };
        i += 1;
        if !matches!(tokens[i].ty, TokenType::Punct('=')) {
            return Err(ParseError { pos: tokens[i].pos, msg: "expected =".into() });
        }
        i += 1;
        let on_right = match &tokens[i].ty {
            TokenType::Identifier(n) => n.clone(),
            _ => return Err(ParseError { pos: tokens[i].pos, msg: "expected column".into() }),
        };
        i += 1;
        joins.push(JoinClause { table, alias: None, on_left, on_right });
    } else {
        break;
    }
}
```

- [ ] **Step 4: 运行测试**

Run: `cargo test -p embedded-database-sql parser 2>&1 | tail -10`
Expected: 16 passed（14 已有 + 2 新增）

- [ ] **Step 5: 检查点**

Run: `cargo test -p embedded-database-sql parser 2>&1 | rg '^test result:' | head -1`
Expected: 16 passed

---

### Task M23-1: executor 嵌套循环连接

**Files:**
- Modify: `crates/embedded-database-sql/src/executor.rs`

**Interfaces:**
- Consumes: `SelectStmt.joins`（M23-0）、`KvDb::scan`（M19-0）
- Produces: `execute_select` 支持 JOIN

- [ ] **Step 1: 写失败测试**

```rust
// crates/embedded-database-sql/src/executor.rs 测试模块追加
#[test]
fn 执行_select_join_嵌套循环() {
    let mut dev = InMemoryDevice::new(1 << 20);
    let mut kv = KvDb::open(&mut dev, Config::default()).unwrap();
    execute("INSERT INTO users (id, name) VALUES (1, 'alice')", &mut kv).unwrap();
    execute("INSERT INTO users (id, name) VALUES (2, 'bob')", &mut kv).unwrap();
    execute("INSERT INTO orders (user_id, item) VALUES (1, 'book')", &mut kv).unwrap();
    execute("INSERT INTO orders (user_id, item) VALUES (2, 'pen')", &mut kv).unwrap();
    execute("INSERT INTO orders (user_id, item) VALUES (3, 'lamp')", &mut kv).unwrap();
    let result = execute(
        "SELECT users.name FROM users JOIN orders ON users.id = orders.user_id",
        &mut kv,
    ).unwrap();
    // M23 简化：嵌套循环连接，匹配行数 2（id=1 和 id=2）
    assert_eq!(result.rows.len(), 2);
}

#[test]
fn 执行_select_join_无匹配() {
    let mut dev = InMemoryDevice::new(1 << 20);
    let mut kv = KvDb::open(&mut dev, Config::default()).unwrap();
    execute("INSERT INTO users (id, name) VALUES (1, 'alice')", &mut kv).unwrap();
    execute("INSERT INTO orders (user_id, item) VALUES (99, 'lamp')", &mut kv).unwrap();
    let result = execute(
        "SELECT users.name FROM users JOIN orders ON users.id = orders.user_id",
        &mut kv,
    ).unwrap();
    assert!(result.rows.is_empty());
}
```

- [ ] **Step 2: 运行测试验证失败**

Run: `cargo test -p embedded-database-sql 执行_select_join_嵌套循环 2>&1 | tail -10`
Expected: 测试失败（JOIN 未实现）

- [ ] **Step 3: 实现**

在 `executor.rs` 的 `execute_select` 中，FROM 扫描后追加 JOIN 逻辑：

```rust
// JOIN（嵌套循环）
for join in &stmt.joins {
    let left_rows = rows;
    let right_prefix = format!("{}:", join.table);
    let right_results = db.scan(&right_prefix).map_err(|e| SqlError::Db(format!("{e:?}")))?;
    let mut joined = Vec::new();
    for left_row in &left_rows {
        for (right_key, right_payload) in &right_results {
            let right_row = parse_payload(right_payload);
            // 匹配条件：left.on_left == right.on_right（简化：按列名匹配）
            let left_col = join.on_left.split('.').last().unwrap_or("");
            let right_col = join.on_right.split('.').last().unwrap_or("");
            let left_val = left_row.get(left_col).map(|s| s.as_str()).unwrap_or("");
            let right_val = right_row.get(right_col).map(|s| s.as_str()).unwrap_or("");
            if left_val == right_val {
                let mut combined = left_row.clone();
                for (k, v) in &right_row {
                    combined.entry(k.clone()).or_insert(v.clone());
                }
                joined.push(combined);
            }
        }
    }
    rows = joined;
}
```

- [ ] **Step 4: 运行测试**

Run: `cargo test -p embedded-database-sql executor 2>&1 | tail -10`
Expected: 12 passed（10 已有 + 2 新增）

- [ ] **Step 5: 检查点**

Run: `cargo test -p embedded-database-sql 2>&1 | rg '^test result:' | head -1`
Expected: 43 passed

---

### Task M23-2: M23 收尾验收

**Files:**
- Modify: `docs/superpowers/specs/2026-10-02-embedded-database-design.md`

**Interfaces:**
- Consumes: 全部 M23 Task
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

在 `2026-10-02-embedded-database-design.md` 的「实施状态」小节追加 M23 交付内容：

```markdown
## 实施状态（M23 完成）

- M23 交付：SQL JOIN 支持（`FROM t1 JOIN t2 ON t1.col = t2.col`），
  parser 解析 JOIN 子句，executor 实现嵌套循环连接。
- **M23 简化**：
  - 仅支持 INNER JOIN（`JOIN` 默认为内连接）。
  - 嵌套循环连接（外层表逐行 × 内层表逐行）。
  - ON 条件仅支持 `=`（等值连接）。
  - 聚合函数、子查询留待 M24+。
```

- [ ] **Step 6: 检查点**

Run: `rg -n 'M23 完成' docs/superpowers/specs/2026-10-02-embedded-database-design.md`
Expected: 有输出

---

## M23 验收标准

| # | 标准 | 验证方式 |
|---|------|----------|
| 1 | parser 解析 JOIN | M23-0 测试 |
| 2 | executor 嵌套循环连接 | M23-1 测试 |
| 3 | 全部测试通过 | M23-2 Step 1 |
| 4 | CLI 可执行 | M23-2 Step 3 |
| 5 | `inject_crash` exit=0 且 `wcet` exit=0 | M23-2 Step 4 |

## 后续里程碑

M23 完成后，SQL JOIN 支持交付。后续：

- **M24**：SQL 聚合函数（COUNT/SUM/AVG/MIN/MAX）
- **M25**：SQL 优化器
- **M26**：SQL 子查询支持
