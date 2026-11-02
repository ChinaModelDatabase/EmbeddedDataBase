# embedded-database M22 实施计划：WHERE 运算符补齐与多列 ORDER BY

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** lexer/parser 支持 `!=`/`<=`/`>=` 运算符，executor 支持多列 ORDER BY 排序。

**Architecture:** `lexer.rs` 补充 `!=`/`<=`/`>=` 的词法识别；`parser.rs` 的 WHERE 解析支持这三种运算符；`executor.rs` 的排序改为逐列比较（多列 ORDER BY）。

**Tech Stack:** Rust 1.81、`embedded-database-sql`（`no_std` + `alloc`）、零外部依赖。

## Global Constraints

- **C1** `embedded-database-sql` 保持 `#![no_std]`（用 `alloc`）
- **C2** 顶层 `#![forbid(unsafe_code)]`——不允许任何局部 `unsafe`
- **C3** 运行时依赖仅 `zerocopy`、`bitflags`，不新增
- **C4** 库代码禁止 `unwrap()` / `expect()` / `panic!()` / 数组越界（`unwrap()` 仅允许 `#[cfg(test)]`）
- **C11** 不执行任何 git 提交（本项目零提交策略）
- **工具链**：每个 bash 调用前 `export PATH="/opt/homebrew/opt/rustup/bin:$PATH"`
- **验收基线**：M21 交付 260 passed（core 215 + kv 6 + os 5 + sql 34）、5 目标 no_std 编译通过、`inject_crash`/`wcet` exit=0

## 用户裁决记录

- **M22 范围**：WHERE 运算符补齐（`!=`/`<=`/`>=`）+ 多列 ORDER BY。

---

### Task M22-0: lexer 支持 `!=`/`<=`/`>=`

**Files:**
- Modify: `crates/embedded-database-sql/src/lexer.rs`

**Interfaces:**
- Consumes: `Token`/`TokenType`（M16-1）
- Produces: `tokenize` 输出 `Punct('!')` 后跟 `Punct('=')` 或 `Punct('<')`/`Punct('>')` 后跟 `Punct('=')`

- [ ] **Step 1: 写失败测试**

```rust
// crates/embedded-database-sql/src/lexer.rs 测试模块追加
#[test]
fn 词法_不等运算符() {
    let tokens = tokenize("SELECT * FROM t WHERE a != 1").unwrap();
    // 应输出 Punct('!') Punct('=') 而非单个 token
    assert!(tokens.iter().any(|t| t.ty == TokenType::Punct('!')));
    assert!(tokens.iter().any(|t| t.ty == TokenType::Punct('=')));
}

#[test]
fn 词法_小于等于() {
    let tokens = tokenize("SELECT * FROM t WHERE a <= 1").unwrap();
    assert!(tokens.iter().any(|t| t.ty == TokenType::Punct('<')));
    assert!(tokens.iter().any(|t| t.ty == TokenType::Punct('=')));
}

#[test]
fn 词法_大于等于() {
    let tokens = tokenize("SELECT * FROM t WHERE a >= 1").unwrap();
    assert!(tokens.iter().any(|t| t.ty == TokenType::Punct('>')));
    assert!(tokens.iter().any(|t| t.ty == TokenType::Punct('=')));
}
```

- [ ] **Step 2: 运行测试验证失败**

Run: `cargo test -p embedded-database-sql lexer 2>&1 | tail -10`
Expected: 编译失败（`!` 不是合法标点）

- [ ] **Step 3: 实现**

在 `lexer.rs` 的标点处理中追加 `!`：

```rust
// 标点符号
if "()*,;!".contains(c) {
    tokens.push(Token { ty: TokenType::Punct(c), pos });
    i += 1;
    continue;
}
```

- [ ] **Step 4: 运行测试**

Run: `cargo test -p embedded-database-sql lexer 2>&1 | tail -10`
Expected: 11 passed（8 已有 + 3 新增）

- [ ] **Step 5: 检查点**

Run: `cargo test -p embedded-database-sql lexer 2>&1 | rg '^test result:' | head -1`
Expected: 11 passed

---

### Task M22-1: parser 支持 `!=`/`<=`/`>=`

**Files:**
- Modify: `crates/embedded-database-sql/src/parser.rs`

**Interfaces:**
- Consumes: `Punct('!')`/`Punct('<')`/`Punct('>')`/`Punct('=')`（M22-0）
- Produces: WHERE 解析支持 `BinOp::Ne`/`BinOp::Le`/`BinOp::Ge`

- [ ] **Step 1: 写失败测试**

```rust
// crates/embedded-database-sql/src/parser.rs 测试模块追加
#[test]
fn 解析_where_ne() {
    let tokens = tokenize("SELECT name FROM users WHERE age != 30").unwrap();
    let stmt = parse(&tokens).unwrap();
    if let Statement::Select(s) = stmt {
        assert!(s.where_clause.is_some());
    }
}

#[test]
fn 解析_where_le() {
    let tokens = tokenize("SELECT name FROM users WHERE age <= 30").unwrap();
    let stmt = parse(&tokens).unwrap();
    if let Statement::Select(s) = stmt {
        assert!(s.where_clause.is_some());
    }
}

#[test]
fn 解析_where_ge() {
    let tokens = tokenize("SELECT name FROM users WHERE age >= 30").unwrap();
    let stmt = parse(&tokens).unwrap();
    if let Statement::Select(s) = stmt {
        assert!(s.where_clause.is_some());
    }
}
```

- [ ] **Step 2: 运行测试验证失败**

Run: `cargo test -p embedded-database-sql parser 2>&1 | tail -10`
Expected: 测试失败（WHERE 不等运算符未解析）

- [ ] **Step 3: 实现**

在 `parser.rs` 的 WHERE 解析中追加运算符匹配：

```rust
let bin_op = match op {
    '=' => BinOp::Eq,
    '!' => BinOp::Ne,  // != 序列
    '<' => BinOp::Lt,
    '>' => BinOp::Gt,
    _ => return Err(ParseError { pos: tokens[i].pos, msg: "unsupported operator".into() }),
};
// 处理 <= / >= / != 的第二个字符
if matches!(op, '<' | '>' | '!') && i + 2 < tokens.len() {
    if let TokenType::Punct('=') = tokens[i + 2].ty {
        // <= / >= / !=
        i += 2;
    }
}
```

- [ ] **Step 4: 运行测试**

Run: `cargo test -p embedded-database-sql parser 2>&1 | tail -10`
Expected: 14 passed（11 已有 + 3 新增）

- [ ] **Step 5: 检查点**

Run: `cargo test -p embedded-database-sql parser 2>&1 | rg '^test result:' | head -1`
Expected: 14 passed

---

### Task M22-2: executor 多列 ORDER BY

**Files:**
- Modify: `crates/embedded-database-sql/src/executor.rs`

**Interfaces:**
- Consumes: `SelectStmt.order_by`（M17-0）
- Produces: `execute_select` 支持多列排序

- [ ] **Step 1: 写失败测试**

```rust
// crates/embedded-database-sql/src/executor.rs 测试模块追加
#[test]
fn 执行_select_order_by_多列() {
    let mut dev = InMemoryDevice::new(1 << 20);
    let mut kv = KvDb::open(&mut dev, Config::default()).unwrap();
    execute("INSERT INTO users (name, age) VALUES ('alice', 30)", &mut kv).unwrap();
    execute("INSERT INTO users (name, age) VALUES ('alice', 25)", &mut kv).unwrap();
    execute("INSERT INTO users (name, age) VALUES ('bob', 30)", &mut kv).unwrap();
    let result = execute("SELECT name, age FROM users ORDER BY name ASC, age DESC", &mut kv).unwrap();
    assert_eq!(result.rows.len(), 3);
    // M22 简化：多列排序基于字符串比较
}
```

- [ ] **Step 2: 运行测试验证失败**

Run: `cargo test -p embedded-database-sql 执行_select_order_by_多列 2>&1 | tail -10`
Expected: 测试失败（多列排序未实现）

- [ ] **Step 3: 实现**

在 `executor.rs` 的排序逻辑中改为逐列比较：

```rust
// ORDER BY（多列）
if !stmt.order_by.is_empty() {
    rows.sort_by(|a, b| {
        for order in &stmt.order_by {
            let col_name = match &order.expr {
                Expr::Column(name) => name.clone(),
                _ => String::new(),
            };
            let a_val = a.get(&col_name).map(|s| s.as_str()).unwrap_or("");
            let b_val = b.get(&col_name).map(|s| s.as_str()).unwrap_or("");
            let cmp = if order.asc { a_val.cmp(b_val) } else { b_val.cmp(a_val) };
            if cmp != std::cmp::Ordering::Equal {
                return cmp;
            }
        }
        std::cmp::Ordering::Equal
    });
}
```

- [ ] **Step 4: 运行测试**

Run: `cargo test -p embedded-database-sql executor 2>&1 | tail -10`
Expected: 10 passed（9 已有 + 1 新增）

- [ ] **Step 5: 检查点**

Run: `cargo test -p embedded-database-sql 2>&1 | rg '^test result:' | head -1`
Expected: 38 passed

---

### Task M22-3: M22 收尾验收

**Files:**
- Modify: `docs/superpowers/specs/2026-10-02-embedded-database-design.md`

**Interfaces:**
- Consumes: 全部 M22 Task
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

在 `2026-10-02-embedded-database-design.md` 的「实施状态」小节追加 M22 交付内容：

```markdown
## 实施状态（M22 完成）

- M22 交付：WHERE 运算符补齐（`!=`/`<=`/`>=`）、多列 ORDER BY 排序。
  - lexer 补充 `!` 标点识别。
  - parser 支持 `!=`/`<=`/`>=` 二字符运算符。
  - executor 支持多列排序（逐列比较）。
- **M22 简化**：
  - WHERE 过滤仍基于字符串比较（非类型感知）。
  - JOIN 支持留待 M23+。
```

- [ ] **Step 6: 检查点**

Run: `rg -n 'M22 完成' docs/superpowers/specs/2026-10-02-embedded-database-design.md`
Expected: 有输出

---

## M22 验收标准

| # | 标准 | 验证方式 |
|---|------|----------|
| 1 | lexer 支持 `!=`/`<=`/`>=` | M22-0 测试 |
| 2 | parser 支持 `!=`/`<=`/`>=` | M22-1 测试 |
| 3 | executor 多列 ORDER BY | M22-2 测试 |
| 4 | 全部测试通过 | M22-3 Step 1 |
| 5 | CLI 可执行 | M22-3 Step 3 |
| 6 | `inject_crash` exit=0 且 `wcet` exit=0 | M22-3 Step 4 |

## 后续里程碑

M22 完成后，WHERE 运算符完整（`=`/`!=`/`<`/`<=`/`>`/`>=`），ORDER BY 支持多列。后续：

- **M23**：SQL JOIN 支持
- **M24**：SQL 优化器
- **M25**：SQL 聚合函数（COUNT/SUM/AVG/MIN/MAX）
