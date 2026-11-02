# embedded-database M17 实施计划：SQL 语法分析器与 AST

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 递归下降 SQL 语法分析器，支持 `SELECT`/`INSERT`/`CREATE TABLE`/`DROP TABLE`，输出 AST。

**Architecture:** `embedded-database-sql` crate 新增 `ast.rs`（AST 类型）和 `parser.rs`（递归下降解析器）。解析器消费 `tokenize()` 输出的 `Vec<Token>`，按语句类型分派解析函数，输出 `Statement` 枚举。

**Tech Stack:** Rust 1.81、`embedded-database-sql` crate（`no_std` + `alloc`）、零外部依赖。

## Global Constraints

- **C1** `embedded-database-sql` 保持 `#![no_std]`（用 `alloc`）
- **C2** 顶层 `#![forbid(unsafe_code)]`——不允许任何局部 `unsafe`
- **C3** 运行时依赖仅 `zerocopy`、`bitflags`，不新增
- **C4** 库代码禁止 `unwrap()` / `expect()` / `panic!()` / 数组越界（`unwrap()` 仅允许 `#[cfg(test)]`）
- **C11** 不执行任何 git 提交（本项目零提交策略）
- **工具链**：每个 bash 调用前 `export PATH="/opt/homebrew/opt/rustup/bin:$PATH"`
- **验收基线**：M16 交付 233 passed（core 215 + kv 3 + os 5 + sql 10）、5 目标 no_std 编译通过、`inject_crash`/`wcet` exit=0

## 用户裁决记录

- **M17 范围**：SQL 语法分析器 + AST 定义。

---

### Task M17-0: AST 类型定义

**Files:**
- Create: `crates/embedded-database-sql/src/ast.rs`
- Modify: `crates/embedded-database-sql/src/lib.rs`

**Interfaces:**
- Consumes: 无
- Produces:
  - `pub enum Statement { Select(SelectStmt), Insert(InsertStmt), CreateTable(CreateTableStmt), DropTable(DropTableStmt) }`
  - `pub struct SelectStmt { pub columns: Vec<SelectItem>, pub from: Option<String>, pub where_clause: Option<Expr>, pub order_by: Vec<OrderByItem>, pub limit: Option<i64> }`
  - `pub enum SelectItem { Wildcard, Column(String) }`
  - `pub struct InsertStmt { pub table: String, pub columns: Vec<String>, pub values: Vec<Vec<Expr>> }`
  - `pub struct CreateTableStmt { pub table: String, pub columns: Vec<ColumnDef> }`
  - `pub struct ColumnDef { pub name: String, pub col_type: String, pub primary_key: bool, pub not_null: bool }`
  - `pub struct DropTableStmt { pub table: String }`
  - `pub enum Expr { Literal(Literal), Column(String), BinaryOp(Box<Expr>, BinOp, Box<Expr>), UnaryOp(UnOp, Box<Expr>), IsNull(Box<Expr>, bool) }`
  - `pub enum Literal { Integer(i64), Float(f64), StringLit(String), Null }`
  - `pub enum BinOp { Add, Sub, Mul, Div, Eq, Ne, Lt, Le, Gt, Ge, And, Or, Like }`
  - `pub enum UnOp { Not, Neg }`
  - `pub struct OrderByItem { pub expr: Expr, pub asc: bool }`

- [ ] **Step 1: 写失败测试**

```rust
// crates/embedded-database-sql/src/ast.rs 末尾
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ast_创建_statement() {
        let stmt = Statement::DropTable(DropTableStmt { table: "users".into() });
        assert!(matches!(stmt, Statement::DropTable(_)));
    }

    #[test]
    fn ast_表达式() {
        let e = Expr::BinaryOp(
            Box::new(Expr::Column("x".into())),
            BinOp::Add,
            Box::new(Expr::Literal(Literal::Integer(1))),
        );
        assert!(matches!(e, Expr::BinaryOp(_, BinOp::Add, _)));
    }
}
```

- [ ] **Step 2: 运行测试验证失败**

Run: `cargo test -p embedded-database-sql ast 2>&1 | tail -10`
Expected: 编译失败（`Statement` 不存在）

- [ ] **Step 3: 写实现**

```rust
// crates/embedded-database-sql/src/ast.rs
//! SQL 抽象语法树（AST）类型定义。

use alloc::string::String;
use alloc::boxed::Box;
use alloc::vec::Vec;

/// SQL 语句。
#[derive(Debug, Clone, PartialEq)]
pub enum Statement {
    Select(SelectStmt),
    Insert(InsertStmt),
    CreateTable(CreateTableStmt),
    DropTable(DropTableStmt),
}

/// SELECT 语句。
#[derive(Debug, Clone, PartialEq)]
pub struct SelectStmt {
    pub columns: Vec<SelectItem>,
    pub from: Option<String>,
    pub where_clause: Option<Expr>,
    pub order_by: Vec<OrderByItem>,
    pub limit: Option<i64>,
}

/// SELECT 列项。
#[derive(Debug, Clone, PartialEq)]
pub enum SelectItem {
    Wildcard,
    Column(String),
}

/// INSERT 语句。
#[derive(Debug, Clone, PartialEq)]
pub struct InsertStmt {
    pub table: String,
    pub columns: Vec<String>,
    pub values: Vec<Vec<Expr>>,
}

/// CREATE TABLE 语句。
#[derive(Debug, Clone, PartialEq)]
pub struct CreateTableStmt {
    pub table: String,
    pub columns: Vec<ColumnDef>,
}

/// 列定义。
#[derive(Debug, Clone, PartialEq)]
pub struct ColumnDef {
    pub name: String,
    pub col_type: String,
    pub primary_key: bool,
    pub not_null: bool,
}

/// DROP TABLE 语句。
#[derive(Debug, Clone, PartialEq)]
pub struct DropTableStmt {
    pub table: String,
}

/// 表达式。
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Literal(Literal),
    Column(String),
    BinaryOp(Box<Expr>, BinOp, Box<Expr>),
    UnaryOp(UnOp, Box<Expr>),
    IsNull(Box<Expr>, bool),
}

/// 字面量。
#[derive(Debug, Clone, PartialEq)]
pub enum Literal {
    Integer(i64),
    Float(f64),
    StringLit(String),
    Null,
}

/// 二元运算符。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
    Like,
}

/// 一元运算符。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnOp {
    Not,
    Neg,
}

/// ORDER BY 项。
#[derive(Debug, Clone, PartialEq)]
pub struct OrderByItem {
    pub expr: Expr,
    pub asc: bool,
}
```

- [ ] **Step 4: 运行测试**

Run: `cargo test -p embedded-database-sql ast 2>&1 | tail -10`
Expected: 2 passed

- [ ] **Step 5: 检查点**

Run: `cargo test -p embedded-database-sql ast 2>&1 | rg 'test result'`
Expected: 2 passed

---

### Task M17-1: 语法分析器（SELECT）

**Files:**
- Create: `crates/embedded-database-sql/src/parser.rs`
- Modify: `crates/embedded-database-sql/src/lib.rs`

**Interfaces:**
- Consumes: `Token`/`TokenType`/`Keyword`（M16-1）、`Statement`/`SelectStmt` 等（M17-0）
- Produces:
  - `pub fn parse(tokens: &[Token]) -> Result<Statement, ParseError>`
  - `pub struct ParseError { pub pos: usize, pub msg: String }`

- [ ] **Step 1: 写失败测试**

```rust
// crates/embedded-database-sql/src/parser.rs 末尾
#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::tokenize;

    #[test]
    fn 解析_select_基本() {
        let tokens = tokenize("SELECT * FROM users").unwrap();
        let stmt = parse(&tokens).unwrap();
        assert!(matches!(stmt, Statement::Select(_)));
        if let Statement::Select(s) = stmt {
            assert_eq!(s.columns, vec![SelectItem::Wildcard]);
            assert_eq!(s.from, Some("users".into()));
            assert!(s.where_clause.is_none());
        }
    }

    #[test]
    fn 解析_select_列名() {
        let tokens = tokenize("SELECT name, age FROM users").unwrap();
        let stmt = parse(&tokens).unwrap();
        if let Statement::Select(s) = stmt {
            assert_eq!(s.columns, vec![SelectItem::Column("name".into()), SelectItem::Column("age".into())]);
        }
    }

    #[test]
    fn 解析_insert_基本() {
        let tokens = tokenize("INSERT INTO users (name, age) VALUES ('alice', 30)").unwrap();
        let stmt = parse(&tokens).unwrap();
        if let Statement::Insert(s) = stmt {
            assert_eq!(s.table, "users");
            assert_eq!(s.columns, vec!["name".into(), "age".into()]);
            assert_eq!(s.values.len(), 1);
            assert_eq!(s.values[0].len(), 2);
        }
    }

    #[test]
    fn 解析_create_table() {
        let tokens = tokenize("CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT NOT NULL)").unwrap();
        let stmt = parse(&tokens).unwrap();
        if let Statement::CreateTable(s) = stmt {
            assert_eq!(s.table, "users");
            assert_eq!(s.columns.len(), 2);
            assert!(s.columns[0].primary_key);
            assert!(s.columns[1].not_null);
        }
    }

    #[test]
    fn 解析_drop_table() {
        let tokens = tokenize("DROP TABLE users").unwrap();
        let stmt = parse(&tokens).unwrap();
        assert!(matches!(stmt, Statement::DropTable(DropTableStmt { table }) if table == "users"));
    }

    #[test]
    fn 解析_空输入报错() {
        let tokens = tokenize("").unwrap();
        assert!(parse(&tokens).is_err());
    }

    #[test]
    fn 解析_未知语句报错() {
        let tokens = tokenize("FOO BAR").unwrap();
        assert!(parse(&tokens).is_err());
    }
}
```

- [ ] **Step 2: 运行测试验证失败**

Run: `cargo test -p embedded-database-sql parser 2>&1 | tail -10`
Expected: 编译失败（`parse` 不存在）

- [ ] **Step 3: 写实现**

```rust
// crates/embedded-database-sql/src/parser.rs
//! SQL 递归下降语法分析器。

use crate::ast::*;
use crate::token::{Keyword, Token, TokenType};
use alloc::string::String;
use alloc::vec::Vec;

/// 语法错误。
#[derive(Debug, Clone, PartialEq)]
pub struct ParseError {
    pub pos: usize,
    pub msg: String,
}

/// 解析 SQL 语句。
pub fn parse(tokens: &[Token]) -> Result<Statement, ParseError> {
    if tokens.is_empty() {
        return Err(ParseError { pos: 0, msg: "empty input".into() });
    }
    match &tokens[0].ty {
        TokenType::Keyword(Keyword::Select) => parse_select(tokens),
        TokenType::Keyword(Keyword::Insert) => parse_insert(tokens),
        TokenType::Keyword(Keyword::Create) => parse_create_table(tokens),
        TokenType::Keyword(Keyword::Drop) => parse_drop_table(tokens),
        _ => Err(ParseError { pos: tokens[0].pos, msg: "unexpected token".into() }),
    }
}

/// 解析 SELECT 语句。
fn parse_select(tokens: &[Token]) -> Result<Statement, ParseError> {
    let mut i = 1; // 跳过 SELECT
    let mut columns = Vec::new();

    // 解析列列表
    loop {
        if i >= tokens.len() {
            return Err(ParseError { pos: 0, msg: "unexpected end".into() });
        }
        match &tokens[i].ty {
            TokenType::Punct('*') => {
                columns.push(SelectItem::Wildcard);
                i += 1;
            }
            TokenType::Identifier(name) => {
                columns.push(SelectItem::Column(name.clone()));
                i += 1;
            }
            _ => return Err(ParseError { pos: tokens[i].pos, msg: "expected column".into() }),
        }
        if i < tokens.len() {
            if let TokenType::Punct(',') = tokens[i].ty {
                i += 1;
                continue;
            }
        }
        break;
    }

    // FROM
    let mut from = None;
    if i < tokens.len() {
        if let TokenType::Keyword(Keyword::From) = tokens[i].ty {
            i += 1;
            if i < tokens.len() {
                if let TokenType::Identifier(name) = &tokens[i].ty {
                    from = Some(name.clone());
                    i += 1;
                }
            }
        }
    }

    Ok(Statement::Select(SelectStmt {
        columns,
        from,
        where_clause: None,
        order_by: Vec::new(),
        limit: None,
    }))
}

/// 解析 INSERT 语句。
fn parse_insert(tokens: &[Token]) -> Result<Statement, ParseError> {
    let mut i = 1; // 跳过 INSERT
    // INTO
    if i >= tokens.len() || !matches!(tokens[i].ty, TokenType::Keyword(Keyword::Into)) {
        return Err(ParseError { pos: tokens.get(i).map(|t| t.pos).unwrap_or(0), msg: "expected INTO".into() });
    }
    i += 1;
    // 表名
    if i >= tokens.len() || !matches!(tokens[i].ty, TokenType::Identifier(_)) {
        return Err(ParseError { pos: tokens.get(i).map(|t| t.pos).unwrap_or(0), msg: "expected table name".into() });
    }
    let table = match &tokens[i].ty {
        TokenType::Identifier(name) => name.clone(),
        _ => unreachable!(),
    };
    i += 1;

    // 列列表
    let mut columns = Vec::new();
    if i < tokens.len() && matches!(tokens[i].ty, TokenType::Punct('(')) {
        i += 1;
        loop {
            if i >= tokens.len() || !matches!(tokens[i].ty, TokenType::Identifier(_)) {
                return Err(ParseError { pos: tokens.get(i).map(|t| t.pos).unwrap_or(0), msg: "expected column".into() });
            }
            if let TokenType::Identifier(name) = &tokens[i].ty {
                columns.push(name.clone());
            }
            i += 1;
            if i < tokens.len() {
                if let TokenType::Punct(',') = tokens[i].ty {
                    i += 1;
                    continue;
                }
            }
            break;
        }
        if i >= tokens.len() || !matches!(tokens[i].ty, TokenType::Punct(')')) {
            return Err(ParseError { pos: tokens.get(i).map(|t| t.pos).unwrap_or(0), msg: "expected )".into() });
        }
        i += 1;
    }

    // VALUES
    if i >= tokens.len() || !matches!(tokens[i].ty, TokenType::Keyword(Keyword::Values)) {
        return Err(ParseError { pos: tokens.get(i).map(|t| t.pos).unwrap_or(0), msg: "expected VALUES".into() });
    }
    i += 1;

    // 值列表
    let mut values = Vec::new();
    loop {
        if i >= tokens.len() || !matches!(tokens[i].ty, TokenType::Punct('(')) {
            return Err(ParseError { pos: tokens.get(i).map(|t| t.pos).unwrap_or(0), msg: "expected (".into() });
        }
        i += 1;
        let mut row = Vec::new();
        loop {
            if i >= tokens.len() {
                return Err(ParseError { pos: 0, msg: "unexpected end".into() });
            }
            match &tokens[i].ty {
                TokenType::Integer(v) => { row.push(Expr::Literal(Literal::Integer(*v))); i += 1; }
                TokenType::Float(v) => { row.push(Expr::Literal(Literal::Float(*v))); i += 1; }
                TokenType::StringLit(s) => { row.push(Expr::Literal(Literal::StringLit(s.clone()))); i += 1; }
                TokenType::Keyword(Keyword::Null) => { row.push(Expr::Literal(Literal::Null)); i += 1; }
                _ => return Err(ParseError { pos: tokens[i].pos, msg: "expected literal".into() }),
            }
            if i < tokens.len() {
                if let TokenType::Punct(',') = tokens[i].ty {
                    i += 1;
                    continue;
                }
            }
            break;
        }
        if i >= tokens.len() || !matches!(tokens[i].ty, TokenType::Punct(')')) {
            return Err(ParseError { pos: tokens.get(i).map(|t| t.pos).unwrap_or(0), msg: "expected )".into() });
        }
        i += 1;
        values.push(row);
        if i < tokens.len() {
            if let TokenType::Punct(',') = tokens[i].ty {
                i += 1;
                continue;
            }
        }
        break;
    }

    Ok(Statement::Insert(InsertStmt { table, columns, values }))
}

/// 解析 CREATE TABLE 语句。
fn parse_create_table(tokens: &[Token]) -> Result<Statement, ParseError> {
    let mut i = 1; // 跳过 CREATE
    if i >= tokens.len() || !matches!(tokens[i].ty, TokenType::Keyword(Keyword::Table)) {
        return Err(ParseError { pos: tokens.get(i).map(|t| t.pos).unwrap_or(0), msg: "expected TABLE".into() });
    }
    i += 1;
    if i >= tokens.len() || !matches!(tokens[i].ty, TokenType::Identifier(_)) {
        return Err(ParseError { pos: tokens.get(i).map(|t| t.pos).unwrap_or(0), msg: "expected table name".into() });
    }
    let table = match &tokens[i].ty {
        TokenType::Identifier(name) => name.clone(),
        _ => unreachable!(),
    };
    i += 1;

    // 列定义
    let mut columns = Vec::new();
    if i >= tokens.len() || !matches!(tokens[i].ty, TokenType::Punct('(')) {
        return Err(ParseError { pos: tokens.get(i).map(|t| t.pos).unwrap_or(0), msg: "expected (".into() });
    }
    i += 1;
    loop {
        if i >= tokens.len() || !matches!(tokens[i].ty, TokenType::Identifier(_)) {
            return Err(ParseError { pos: tokens.get(i).map(|t| t.pos).unwrap_or(0), msg: "expected column name".into() });
        }
        let name = match &tokens[i].ty {
            TokenType::Identifier(n) => n.clone(),
            _ => unreachable!(),
        };
        i += 1;

        // 类型
        if i >= tokens.len() {
            return Err(ParseError { pos: 0, msg: "unexpected end".into() });
        }
        let col_type = match &tokens[i].ty {
            TokenType::Identifier(t) => t.clone(),
            TokenType::Keyword(k) => alloc::format!("{:?}", k),
            _ => return Err(ParseError { pos: tokens[i].pos, msg: "expected type".into() }),
        };
        i += 1;

        let mut primary_key = false;
        let mut not_null = false;

        // 约束
        while i < tokens.len() {
            match &tokens[i].ty {
                TokenType::Keyword(Keyword::Primary) => {
                    i += 1;
                    if i < tokens.len() && matches!(tokens[i].ty, TokenType::Keyword(Keyword::Key)) {
                        primary_key = true;
                        i += 1;
                    }
                }
                TokenType::Keyword(Keyword::Not) => {
                    i += 1;
                    if i < tokens.len() && matches!(tokens[i].ty, TokenType::Keyword(Keyword::Null)) {
                        not_null = true;
                        i += 1;
                    }
                }
                _ => break,
            }
        }

        columns.push(ColumnDef { name, col_type, primary_key, not_null });

        if i < tokens.len() {
            if let TokenType::Punct(',') = tokens[i].ty {
                i += 1;
                continue;
            }
        }
        break;
    }
    if i >= tokens.len() || !matches!(tokens[i].ty, TokenType::Punct(')')) {
        return Err(ParseError { pos: tokens.get(i).map(|t| t.pos).unwrap_or(0), msg: "expected )".into() });
    }

    Ok(Statement::CreateTable(CreateTableStmt { table, columns }))
}

/// 解析 DROP TABLE 语句。
fn parse_drop_table(tokens: &[Token]) -> Result<Statement, ParseError> {
    let mut i = 1; // 跳过 DROP
    if i >= tokens.len() || !matches!(tokens[i].ty, TokenType::Keyword(Keyword::Table)) {
        return Err(ParseError { pos: tokens.get(i).map(|t| t.pos).unwrap_or(0), msg: "expected TABLE".into() });
    }
    i += 1;
    if i >= tokens.len() || !matches!(tokens[i].ty, TokenType::Identifier(_)) {
        return Err(ParseError { pos: tokens.get(i).map(|t| t.pos).unwrap_or(0), msg: "expected table name".into() });
    }
    let table = match &tokens[i].ty {
        TokenType::Identifier(name) => name.clone(),
        _ => unreachable!(),
    };
    Ok(Statement::DropTable(DropTableStmt { table }))
}
```

- [ ] **Step 4: 运行测试**

Run: `cargo test -p embedded-database-sql parser 2>&1 | tail -15`
Expected: 7 passed

- [ ] **Step 5: 检查点**

Run: `cargo test -p embedded-database-sql parser 2>&1 | rg 'test result'`
Expected: 7 passed

---

### Task M17-2: M17 收尾验收

**Files:**
- Modify: `docs/superpowers/specs/2026-10-02-embedded-database-design.md`

**Interfaces:**
- Consumes: 全部 M17 Task
- Produces: 设计文档「实施状态」小节更新

- [ ] **Step 1: 全量测试**

Run: `cargo test --workspace --features embedded-database-core/std 2>&1 | tail -15`
Expected: 全部通过

- [ ] **Step 2: 格式化与 lint**

Run: `cargo fmt --all && cargo clippy --workspace --all-targets --features embedded-database-core/std -- -D warnings 2>&1 | tail -20`
Expected: 无警告

- [ ] **Step 3: 5 目标 no_std 验证**

Run:
```bash
for t in aarch64-unknown-linux-gnu loongarch64-unknown-linux-gnu \
         loongarch64-unknown-linux-musl x86_64-unknown-linux-gnu \
         loongarch64-unknown-none; do
  printf '%-36s ' "$t"
  cargo check -p embedded-database-sql --target "$t" 2>&1 | tail -1
done
```
Expected: 5 个目标全部无错误

- [ ] **Step 4: unsafe / alloc 扫描**

Run: `rg -n "unsafe" crates/*/src -g '*.rs' | rg -v "forbid\(unsafe_code\)" || echo "clean"`
Expected: clean（或仅注释命中）

- [ ] **Step 5: 工具二进制可用**

Run: `cargo run -p embedded-database-bench --bin inject_crash >/dev/null 2>&1; echo "inject=$?"; cargo run -p embedded-database-bench --bin wcet >/dev/null 2>&1; echo "wcet=$?"`
Expected: `inject=0` 且 `wcet=0`

- [ ] **Step 6: 更新设计文档**

在 `2026-10-02-embedded-database-design.md` 的「实施状态」小节追加 M17 交付内容：

```markdown
## 实施状态（M17 完成）

- M17 交付：SQL AST 类型定义（`Statement`/`SelectStmt`/`InsertStmt`/`CreateTableStmt`/
  `DropTableStmt`/`Expr`/`Literal`/`BinOp`/`UnOp`）、递归下降语法分析器
  （`parse(tokens) -> Result<Statement, ParseError>`，覆盖 SELECT/INSERT/CREATE TABLE/DROP TABLE）。
- **M17 简化**：
  - SELECT 暂不支持 WHERE/ORDER BY/LIMIT/JOIN（AST 类型已定义，解析器未实现）。
  - INSERT 支持单行和多行 VALUES。
  - CREATE TABLE 支持 PRIMARY KEY 和 NOT NULL 约束。
  - 绑定、优化器、执行器留待 M18+。
```

- [ ] **Step 7: 检查点**

Run: `rg -n 'M17 完成' docs/superpowers/specs/2026-10-02-embedded-database-design.md`
Expected: 有输出

---

## M17 验收标准

| # | 标准 | 验证方式 |
|---|------|----------|
| 1 | AST 类型定义 | M17-0 测试 |
| 2 | SQL 语法分析器（SELECT/INSERT/CREATE TABLE/DROP TABLE） | M17-1 测试 |
| 3 | 5 个 no_std 目标全部编译通过 | M17-2 Step 3 |
| 4 | 全 crate 零 `unsafe` | M17-2 Step 4 |
| 5 | 全部测试通过 | M17-2 Step 1 |
| 6 | `inject_crash` exit=0 且 `wcet` exit=0 | M17-2 Step 5 |

## 后续里程碑

M17 完成后，SQL 语法分析器交付。后续：

- **M18**：SQL 绑定 + 简单执行器
- **M19**：SQL 优化器 + 范围扫描集成
- **M20**：SQL CLI（`embedded-database-cli`）
