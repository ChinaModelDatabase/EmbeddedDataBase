# embedded-database M16 实施计划：SQL 词法分析器

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 新建 `embedded-database-sql` crate，实现 `SELECT`/`INSERT`/`CREATE TABLE`/`DROP TABLE` 的 SQL 词法分析（Token 流输出）。

**Architecture:** `embedded-database-sql` crate（`no_std` + `forbid(unsafe_code)`），词法分析器将 SQL 字符串解析为 Token 流。Token 类型：关键字、标识符、字符串字面量、数字字面量、运算符、分隔符。分析器输出 `Vec<Token>`（`std` 门控）。

**Tech Stack:** Rust 1.81、`embedded-database-sql` crate（`no_std`）、零外部依赖。

## Global Constraints

- **C1** `embedded-database-sql` 保持 `#![no_std]`
- **C2** 顶层 `#![forbid(unsafe_code)]`——不允许任何局部 `unsafe`
- **C3** 运行时依赖仅 `zerocopy`、`bitflags`，不新增
- **C4** 库代码禁止 `unwrap()` / `expect()` / `panic!()` / 数组越界（`unwrap()` 仅允许 `#[cfg(test)]`）
- **C11** 不执行任何 git 提交（本项目零提交策略）
- **工具链**：每个 bash 调用前 `export PATH="/opt/homebrew/opt/rustup/bin:$PATH"`
- **验收基线**：M15 交付 223 passed（core 215 + kv 3 + os 5）、5 目标 no_std 编译通过、`inject_crash`/`wcet` exit=0

## 用户裁决记录

- **M16 范围**：SQL 词法分析器（Token 流输出）。

---

### Task M16-0: embedded-database-sql crate 骨架

**Files:**
- Create: `crates/embedded-database-sql/Cargo.toml`
- Create: `crates/embedded-database-sql/src/lib.rs`
- Modify: `Cargo.toml`（workspace members）

**Interfaces:**
- Consumes: 无
- Produces: `embedded-database-sql` crate 骨架

- [ ] **Step 1: 创建 Cargo.toml**

```toml
# crates/embedded-database-sql/Cargo.toml
[package]
name = "embedded-database-sql"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true
description = "embedded-database SQL 词法/语法/执行引擎"

[features]
default = []
std = []

[dev-dependencies]
embedded-database-core = { workspace = true, features = ["std"] }

[lints.rust]
unsafe_code = "forbid"
```

- [ ] **Step 2: 创建 lib.rs**

```rust
#![no_std]
#![forbid(unsafe_code)]

//! embedded-database SQL 词法/语法/执行引擎。
//!
//! M16 阶段仅交付词法分析器。

/// 占位类型，确保 crate 非空。
#[derive(Debug)]
pub struct Placeholder;
```

- [ ] **Step 3: workspace Cargo.toml 加 member**

```toml
members = [
    "crates/embedded-database-core",
    "crates/embedded-database-kv",
    "crates/embedded-database-embed",
    "crates/embedded-database-bench",
    "crates/embedded-database-os",
    "crates/embedded-database-sql",
]
```

- [ ] **Step 4: 验证编译**

Run: `cargo check -p embedded-database-sql 2>&1 | tail -5`
Expected: 通过

- [ ] **Step 5: 检查点**

Run: `ls crates/embedded-database-sql/`
Expected: `Cargo.toml` 和 `src/` 目录存在

---

### Task M16-1: Token 类型定义

**Files:**
- Create: `crates/embedded-database-sql/src/token.rs`
- Modify: `crates/embedded-database-sql/src/lib.rs`

**Interfaces:**
- Consumes: 无
- Produces:
  - `pub enum Keyword { Select, Insert, Into, Values, Create, Table, Drop, From, Where, And, Or, Not, Null, Primary, Key, Unique, Index, Order, By, Asc, Desc, Limit, Offset, Join, On, As, Delete, Update, Set }`
  - `pub enum TokenType { Keyword(Keyword), Identifier(String), Integer(i64), Float(f64), StringLit(String), Punct(char), Eof }`
  - `pub struct Token { pub ty: TokenType, pub pos: usize }`

- [ ] **Step 1: 写失败测试**

```rust
// crates/embedded-database-sql/src/token.rs 末尾
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_创建() {
        let t = Token { ty: TokenType::Keyword(Keyword::Select), pos: 0 };
        assert_eq!(t.pos, 0);
    }

    #[test]
    fn keyword_匹配() {
        assert!(matches!(Keyword::Select, Keyword::Select));
        assert!(!matches!(Keyword::Select, Keyword::Insert));
    }
}
```

- [ ] **Step 2: 运行测试验证失败**

Run: `cargo test -p embedded-database-sql token 2>&1 | tail -10`
Expected: 编译失败（`Token` 不存在）

- [ ] **Step 3: 写实现**

```rust
// crates/embedded-database-sql/src/token.rs
//! SQL Token 类型定义。

/// SQL 关键字。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Keyword {
    Select,
    Insert,
    Into,
    Values,
    Create,
    Table,
    Drop,
    From,
    Where,
    And,
    Or,
    Not,
    Null,
    Primary,
    Key,
    Unique,
    Index,
    Order,
    By,
    Asc,
    Desc,
    Limit,
    Offset,
    Join,
    On,
    As,
    Delete,
    Update,
    Set,
}

/// Token 类型。
#[derive(Debug, Clone, PartialEq)]
pub enum TokenType {
    Keyword(Keyword),
    Identifier(std::string::String),
    Integer(i64),
    Float(f64),
    StringLit(std::string::String),
    Punct(char),
    Eof,
}

/// SQL Token。
#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub ty: TokenType,
    pub pos: usize,
}
```

- [ ] **Step 4: 运行测试**

Run: `cargo test -p embedded-database-sql token 2>&1 | tail -10`
Expected: 2 passed

- [ ] **Step 5: 检查点**

Run: `cargo test -p embedded-database-sql token 2>&1 | rg 'test result'`
Expected: 2 passed

---

### Task M16-2: 词法分析器

**Files:**
- Create: `crates/embedded-database-sql/src/lexer.rs`
- Modify: `crates/embedded-database-sql/src/lib.rs`

**Interfaces:**
- Consumes: `Token`/`TokenType`/`Keyword`（M16-1）
- Produces:
  - `pub fn tokenize(input: &str) -> Result<Vec<Token>, LexError>`
  - `pub struct LexError { pub pos: usize, pub msg: std::string::String }`

- [ ] **Step 1: 写失败测试**

```rust
// crates/embedded-database-sql/src/lexer.rs 末尾
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 词法_select_基本() {
        let tokens = tokenize("SELECT * FROM users").unwrap();
        assert_eq!(tokens.len(), 5);
        assert_eq!(tokens[0].ty, TokenType::Keyword(Keyword::Select));
        assert_eq!(tokens[1].ty, TokenType::Punct('*'));
        assert_eq!(tokens[2].ty, TokenType::Keyword(Keyword::From));
        assert_eq!(tokens[3].ty, TokenType::Identifier("users".into()));
        assert_eq!(tokens[4].ty, TokenType::Eof);
    }

    #[test]
    fn 词法_insert_基本() {
        let tokens = tokenize("INSERT INTO t VALUES (1, 'hello')").unwrap();
        assert_eq!(tokens[0].ty, TokenType::Keyword(Keyword::Insert));
        assert_eq!(tokens[1].ty, TokenType::Keyword(Keyword::Into));
        assert_eq!(tokens[2].ty, TokenType::Identifier("t".into()));
        assert_eq!(tokens[3].ty, TokenType::Keyword(Keyword::Values));
        assert_eq!(tokens[4].ty, TokenType::Punct('('));
        assert_eq!(tokens[5].ty, TokenType::Integer(1));
        assert_eq!(tokens[6].ty, TokenType::Punct(','));
        assert_eq!(tokens[7].ty, TokenType::StringLit("hello".into()));
        assert_eq!(tokens[8].ty, TokenType::Punct(')'));
    }

    #[test]
    fn 词法_create_table() {
        let tokens = tokenize("CREATE TABLE users (id INTEGER PRIMARY KEY)").unwrap();
        assert_eq!(tokens[0].ty, TokenType::Keyword(Keyword::Create));
        assert_eq!(tokens[1].ty, TokenType::Keyword(Keyword::Table));
        assert_eq!(tokens[2].ty, TokenType::Identifier("users".into()));
        assert_eq!(tokens[3].ty, TokenType::Punct('('));
        assert_eq!(tokens[4].ty, TokenType::Identifier("id".into()));
        assert_eq!(tokens[5].ty, TokenType::Identifier("INTEGER".into()));
        assert_eq!(tokens[6].ty, TokenType::Keyword(Keyword::Primary));
        assert_eq!(tokens[7].ty, TokenType::Keyword(Keyword::Key));
        assert_eq!(tokens[8].ty, TokenType::Punct(')'));
    }

    #[test]
    fn 词法_drop_table() {
        let tokens = tokenize("DROP TABLE users").unwrap();
        assert_eq!(tokens[0].ty, TokenType::Keyword(Keyword::Drop));
        assert_eq!(tokens[1].ty, TokenType::Keyword(Keyword::Table));
        assert_eq!(tokens[2].ty, TokenType::Identifier("users".into()));
    }

    #[test]
    fn 词法_数字() {
        let tokens = tokenize("SELECT 42, 3.14").unwrap();
        assert_eq!(tokens[1].ty, TokenType::Integer(42));
        assert_eq!(tokens[3].ty, TokenType::Float(3.14));
    }

    #[test]
    fn 词法_字符串() {
        let tokens = tokenize("SELECT 'hello world'").unwrap();
        assert_eq!(tokens[1].ty, TokenType::StringLit("hello world".into()));
    }

    #[test]
    fn 词法_空输入() {
        let tokens = tokenize("").unwrap();
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].ty, TokenType::Eof);
    }

    #[test]
    fn 词法_非法字符() {
        assert!(tokenize("SELECT # FROM").is_err());
    }
}
```

- [ ] **Step 2: 运行测试验证失败**

Run: `cargo test -p embedded-database-sql lexer 2>&1 | tail -10`
Expected: 编译失败（`tokenize` 不存在）

- [ ] **Step 3: 写实现**

```rust
// crates/embedded-database-sql/src/lexer.rs
//! SQL 词法分析器。

use crate::token::{Keyword, Token, TokenType};
use std::string::String;
use std::vec::Vec;

/// 词法错误。
#[derive(Debug, Clone, PartialEq)]
pub struct LexError {
    pub pos: usize,
    pub msg: String,
}

/// 关键字映射。
fn keyword_from_str(s: &str) -> Option<Keyword> {
    match s.to_ascii_uppercase().as_str() {
        "SELECT" => Some(Keyword::Select),
        "INSERT" => Some(Keyword::Insert),
        "INTO" => Some(Keyword::Into),
        "VALUES" => Some(Keyword::Values),
        "CREATE" => Some(Keyword::Create),
        "TABLE" => Some(Keyword::Table),
        "DROP" => Some(Keyword::Drop),
        "FROM" => Some(Keyword::From),
        "WHERE" => Some(Keyword::Where),
        "AND" => Some(Keyword::And),
        "OR" => Some(Keyword::Or),
        "NOT" => Some(Keyword::Not),
        "NULL" => Some(Keyword::Null),
        "PRIMARY" => Some(Keyword::Primary),
        "KEY" => Some(Keyword::Key),
        "UNIQUE" => Some(Keyword::Unique),
        "INDEX" => Some(Keyword::Index),
        "ORDER" => Some(Keyword::Order),
        "BY" => Some(Keyword::By),
        "ASC" => Some(Keyword::Asc),
        "DESC" => Some(Keyword::Desc),
        "LIMIT" => Some(Keyword::Limit),
        "OFFSET" => Some(Keyword::Offset),
        "JOIN" => Some(Keyword::Join),
        "ON" => Some(Keyword::On),
        "AS" => Some(Keyword::As),
        "DELETE" => Some(Keyword::Delete),
        "UPDATE" => Some(Keyword::Update),
        "SET" => Some(Keyword::Set),
        _ => None,
    }
}

/// SQL 词法分析。
pub fn tokenize(input: &str) -> Result<Vec<Token>, LexError> {
    let bytes = input.as_bytes();
    let mut tokens = Vec::new();
    let mut i = 0usize;

    while i < bytes.len() {
        let c = bytes[i] as char;

        // 跳过空白
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }

        let pos = i;

        // 标识符或关键字
        if c.is_ascii_alphabetic() || c == '_' {
            let start = i;
            while i < bytes.len() && (bytes[i] as char).is_ascii_alphanumeric() || bytes[i] as char == '_' {
                i += 1;
            }
            let word = &input[start..i];
            if let Some(kw) = keyword_from_str(word) {
                tokens.push(Token { ty: TokenType::Keyword(kw), pos });
            } else {
                tokens.push(Token { ty: TokenType::Identifier(word.to_string()), pos });
            }
            continue;
        }

        // 数字
        if c.is_ascii_digit() {
            let start = i;
            let mut is_float = false;
            while i < bytes.len() {
                let b = bytes[i] as char;
                if b.is_ascii_digit() {
                    i += 1;
                } else if b == '.' && !is_float {
                    is_float = true;
                    i += 1;
                } else {
                    break;
                }
            }
            let num_str = &input[start..i];
            if is_float {
                let v: f64 = num_str.parse().map_err(|_| LexError { pos, msg: format!("invalid float: {num_str}") })?;
                tokens.push(Token { ty: TokenType::Float(v), pos });
            } else {
                let v: i64 = num_str.parse().map_err(|_| LexError { pos, msg: format!("invalid integer: {num_str}") })?;
                tokens.push(Token { ty: TokenType::Integer(v), pos });
            }
            continue;
        }

        // 字符串字面量
        if c == '\'' {
            i += 1;
            let start = i;
            while i < bytes.len() && bytes[i] as char != '\'' {
                i += 1;
            }
            if i >= bytes.len() {
                return Err(LexError { pos, msg: "unterminated string".into() });
            }
            let s = &input[start..i];
            i += 1; // 跳过结束引号
            tokens.push(Token { ty: TokenType::StringLit(s.to_string()), pos });
            continue;
        }

        // 标点符号
        if "()*,;".contains(c) {
            tokens.push(Token { ty: TokenType::Punct(c), pos });
            i += 1;
            continue;
        }

        // 其他字符
        return Err(LexError { pos, msg: format!("unexpected character: {c}") });
    }

    tokens.push(Token { ty: TokenType::Eof, pos: input.len() });
    Ok(tokens)
}
```

- [ ] **Step 4: 运行测试**

Run: `cargo test -p embedded-database-sql lexer 2>&1 | tail -15`
Expected: 8 passed

- [ ] **Step 5: 检查点**

Run: `cargo test -p embedded-database-sql lexer 2>&1 | rg 'test result'`
Expected: 8 passed

---

### Task M16-3: M16 收尾验收

**Files:**
- Modify: `docs/superpowers/specs/2026-10-02-embedded-database-design.md`

**Interfaces:**
- Consumes: 全部 M16 Task
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

Run: `rg -n "extern crate alloc|use alloc::" crates/*/src -g '*.rs' || echo "clean"`
Expected: clean

- [ ] **Step 5: 工具二进制可用**

Run: `cargo run -p embedded-database-bench --bin inject_crash >/dev/null 2>&1; echo "inject=$?"; cargo run -p embedded-database-bench --bin wcet >/dev/null 2>&1; echo "wcet=$?"`
Expected: `inject=0` 且 `wcet=0`

- [ ] **Step 6: 更新设计文档**

在 `2026-10-02-embedded-database-design.md` 的「实施状态」小节追加 M16 交付内容：

```markdown
## 实施状态（M16 完成）

- M16 交付：`embedded-database-sql` crate 骨架、SQL Token 类型定义（关键字/标识符/数字/字符串/标点）、
  SQL 词法分析器（`SELECT`/`INSERT`/`CREATE TABLE`/`DROP TABLE`）。
- **M16 简化**：
  - 词法分析器输出 `Vec<Token>`（std 门控）。
  - 支持关键字：SELECT/INSERT/INTO/VALUES/CREATE/TABLE/DROP/FROM/WHERE/AND/OR/NOT/NULL/
    PRIMARY/KEY/UNIQUE/INDEX/ORDER/BY/ASC/DESC/LIMIT/OFFSET/JOIN/ON/AS/DELETE/UPDATE/SET。
  - 数字支持整数和浮点；字符串支持单引号字面量。
  - 标点符号：`(`, `)`, `*`, `,`, `;`。
  - 语法分析器、绑定、优化器、执行器留待 M17+。
```

- [ ] **Step 7: 检查点**

Run: `rg -n 'M16 完成' docs/superpowers/specs/2026-10-02-embedded-database-design.md`
Expected: 有输出

---

## M16 验收标准

| # | 标准 | 验证方式 |
|---|------|----------|
| 1 | `embedded-database-sql` crate 编译通过 | M16-0 |
| 2 | Token 类型定义 | M16-1 测试 |
| 3 | SQL 词法分析器（SELECT/INSERT/CREATE TABLE/DROP TABLE） | M16-2 测试 |
| 4 | 5 个 no_std 目标全部编译通过 | M16-3 Step 3 |
| 5 | 全 crate 零 `unsafe` | M16-3 Step 4 |
| 6 | 全部测试通过 | M16-3 Step 1 |
| 7 | `inject_crash` exit=0 且 `wcet` exit=0 | M16-3 Step 5 |

## 后续里程碑

M16 完成后，SQL 词法分析器交付。后续：

- **M17**：SQL 语法分析器（递归下降）+ AST 定义
- **M18**：SQL 绑定 + 简单执行器
- **M19**：SQL 优化器 + 范围扫描集成
- **M20**：SQL CLI（`embedded-database-cli`）
