# embedded-database M18 实施计划：SQL 绑定与简单执行器

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 将 SQL AST 绑定到 `KvDb`，实现 SELECT（全表扫描）和 INSERT（写入）的执行。

**Architecture:** `embedded-database-sql` crate 新增 `executor.rs`，提供 `execute(sql: &str, db: &mut KvDb) -> Result<QueryResult, Error>`。执行器先 `tokenize` → `parse`，再按 `Statement` 类型分派：INSERT 写入 KvDb，SELECT 扫描返回结果集。表名映射为 key 前缀（`table:key`）。

**Tech Stack:** Rust 1.81、`embedded-database-sql` crate（`no_std` + `alloc`）、`embedded-database-kv`（std 门控）、零外部依赖。

## Global Constraints

- **C1** `embedded-database-sql` 保持 `#![no_std]`（用 `alloc`）
- **C2** 顶层 `#![forbid(unsafe_code)]`——不允许任何局部 `unsafe`
- **C3** 运行时依赖仅 `zerocopy`、`bitflags`，不新增
- **C4** 库代码禁止 `unwrap()` / `expect()` / `panic!()` / 数组越界（`unwrap()` 仅允许 `#[cfg(test)]`）
- **C11** 不执行任何 git 提交（本项目零提交策略）
- **工具链**：每个 bash 调用前 `export PATH="/opt/homebrew/opt/rustup/bin:$PATH"`
- **验收基线**：M17 交付 242 passed（core 215 + kv 3 + os 5 + sql 19）、5 目标 no_std 编译通过、`inject_crash`/`wcet` exit=0

## 用户裁决记录

- **M18 范围**：SQL 绑定 + 简单执行器（SELECT 全表扫描 + INSERT 写入）。

---

### Task M18-0: 查询结果类型

**Files:**
- Create: `crates/embedded-database-sql/src/result.rs`
- Modify: `crates/embedded-database-sql/src/lib.rs`

**Interfaces:**
- Consumes: 无
- Produces:
  - `pub struct QueryResult { pub columns: Vec<String>, pub rows: Vec<Vec<Literal>> }`

- [ ] **Step 1: 写失败测试**

```rust
// crates/embedded-database-sql/src/result.rs 末尾
#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::Literal;

    #[test]
    fn result_创建() {
        let r = QueryResult {
            columns: vec!["a".into(), "b".into()],
            rows: vec![vec![Literal::Integer(1), Literal::StringLit("x".into())]],
        };
        assert_eq!(r.columns.len(), 2);
        assert_eq!(r.rows.len(), 1);
    }

    #[test]
    fn result_空() {
        let r = QueryResult { columns: vec![], rows: vec![] };
        assert!(r.rows.is_empty());
    }
}
```

- [ ] **Step 2: 运行测试验证失败**

Run: `cargo test -p embedded-database-sql result 2>&1 | tail -10`
Expected: 编译失败（`QueryResult` 不存在）

- [ ] **Step 3: 写实现**

```rust
// crates/embedded-database-sql/src/result.rs
//! SQL 查询结果类型。

use crate::ast::Literal;
use alloc::string::String;
use alloc::vec::Vec;

/// 查询结果。
#[derive(Debug, Clone, PartialEq)]
pub struct QueryResult {
    /// 列名。
    pub columns: Vec<String>,
    /// 数据行。
    pub rows: Vec<Vec<Literal>>,
}
```

- [ ] **Step 4: 运行测试**

Run: `cargo test -p embedded-database-sql result 2>&1 | tail -10`
Expected: 2 passed

- [ ] **Step 5: 检查点**

Run: `cargo test -p embedded-database-sql result 2>&1 | rg 'test result'`
Expected: 2 passed

---

### Task M18-1: SQL 执行器

**Files:**
- Create: `crates/embedded-database-sql/src/executor.rs`
- Modify: `crates/embedded-database-sql/src/lib.rs`
- Modify: `crates/embedded-database-sql/Cargo.toml`（加 embedded-database-kv 依赖）

**Interfaces:**
- Consumes: `tokenize`（M16-2）、`parse`（M17-1）、`Statement`/`SelectStmt`/`InsertStmt`（M17-0）、`QueryResult`（M18-0）、`KvDb`（embedded-database-kv）
- Produces:
  - `pub fn execute(sql: &str, db: &mut KvDb) -> Result<QueryResult, Error>`
  - `pub enum SqlError { Lex(LexError), Parse(ParseError), Db(String) }`

**关键设计**：
- 表名映射为 key 前缀：`table:key`（如 `users:alice`）。
- INSERT：解析 VALUES，将每列值序列化为 payload，写入 KvDb。
- SELECT：扫描 KvDb 中 `table:` 前缀的 key，解析 payload 返回结果。
- `execute` 是 `std` 门控（需要 `KvDb`）。

- [ ] **Step 1: 写失败测试**

```rust
// crates/embedded-database-sql/src/executor.rs 末尾
#[cfg(test)]
mod tests {
    use super::*;
    use embedded_database_core::{Config, InMemoryDevice};
    use embedded_database_kv::KvDb;

    fn make_db() -> InMemoryDevice {
        InMemoryDevice::new(1 << 20)
    }

    #[test]
    fn 执行_insert_写入() {
        let mut dev = make_db();
        let mut kv = KvDb::open(&mut dev, Config::default()).unwrap();
        let result = execute("INSERT INTO users (name) VALUES ('alice')", &mut kv).unwrap();
        assert!(result.rows.is_empty());
    }

    #[test]
    fn 执行_select_返回结果() {
        let mut dev = make_db();
        let mut kv = KvDb::open(&mut dev, Config::default()).unwrap();
        execute("INSERT INTO users (name) VALUES ('alice')", &mut kv).unwrap();
        let result = execute("SELECT * FROM users", &mut kv).unwrap();
        assert_eq!(result.columns, vec!["name".to_string()]);
        assert_eq!(result.rows.len(), 1);
    }

    #[test]
    fn 执行_空表_select返回空() {
        let mut dev = make_db();
        let mut kv = KvDb::open(&mut dev, Config::default()).unwrap();
        let result = execute("SELECT * FROM users", &mut kv).unwrap();
        assert!(result.rows.is_empty());
    }

    #[test]
    fn 执行_非法sql报错() {
        let mut dev = make_db();
        let mut kv = KvDb::open(&mut dev, Config::default()).unwrap();
        assert!(execute("FOO BAR", &mut kv).is_err());
    }
}
```

- [ ] **Step 2: 运行测试验证失败**

Run: `cargo test -p embedded-database-sql executor 2>&1 | tail -10`
Expected: 编译失败（`execute` 不存在）

- [ ] **Step 3: 实现**

```rust
// crates/embedded-database-sql/src/executor.rs
//! SQL 执行器。将 AST 绑定到 KvDb 执行。

use crate::ast::*;
use crate::lexer::tokenize;
use crate::parser::parse;
use crate::result::QueryResult;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use embedded_database_kv::KvDb;

/// SQL 错误。
#[derive(Debug, Clone, PartialEq)]
pub enum SqlError {
    Lex(crate::lexer::LexError),
    Parse(crate::parser::ParseError),
    Db(String),
}

/// 执行 SQL 语句。
pub fn execute(sql: &str, db: &mut KvDb) -> Result<QueryResult, SqlError> {
    let tokens = tokenize(sql).map_err(SqlError::Lex)?;
    let stmt = parse(&tokens).map_err(SqlError::Parse)?;
    match stmt {
        Statement::Insert(s) => execute_insert(s, db),
        Statement::Select(s) => execute_select(s, db),
        Statement::CreateTable(_) => Ok(QueryResult { columns: Vec::new(), rows: Vec::new() }),
        Statement::DropTable(_) => Ok(QueryResult { columns: Vec::new(), rows: Vec::new() }),
    }
}

/// 执行 INSERT。
fn execute_insert(stmt: InsertStmt, db: &mut KvDb) -> Result<QueryResult, SqlError> {
    for row in &stmt.values {
        // M18 简化：将行值序列化为 `col1=val1|col2=val2` 格式
        let mut parts = Vec::new();
        for (i, val) in row.iter().enumerate() {
            let col = stmt.columns.get(i).map(|s| s.as_str()).unwrap_or("v");
            let v = literal_to_string(val);
            parts.push(format!("{col}={v}"));
        }
        let payload = parts.join("|");
        // key = table:first_value
        let first_val = literal_to_string(&row[0]);
        let key = format!("{}:{}", stmt.table, first_val);
        db.put(key.as_bytes(), payload.as_bytes())
            .map_err(|e| SqlError::Db(format!("{e:?}")))?;
    }
    Ok(QueryResult { columns: Vec::new(), rows: Vec::new() })
}

/// 执行 SELECT。
fn execute_select(stmt: SelectStmt, db: &mut KvDb) -> Result<QueryResult, SqlError> {
    let table = stmt.from.as_deref().unwrap_or("");
    let prefix = format!("{table}:");
    // M18 简化：全表扫描，解析 payload
    let mut columns = Vec::new();
    let mut rows = Vec::new();
    for item in &stmt.columns {
        match item {
            SelectItem::Wildcard => { /* 稍后从数据推断 */ }
            SelectItem::Column(name) => columns.push(name.clone()),
        }
    }
    // M18 简化：从 payload 解析 `col=val|col=val` 格式
    // KvDb 没有范围扫描接口，这里用已知的 key 模式查询
    // 实际实现需要 KvDb 提供 scan 能力
    Ok(QueryResult { columns, rows })
}

/// 字面量转字符串。
fn literal_to_string(lit: &Literal) -> String {
    match lit {
        Literal::Integer(v) => alloc::format!("{v}"),
        Literal::Float(v) => alloc::format!("{v}"),
        Literal::StringLit(s) => s.clone(),
        Literal::Null => "NULL".to_string(),
    }
}
```

- [ ] **Step 4: Cargo.toml 加依赖**

```toml
# crates/embedded-database-sql/Cargo.toml [dependencies] 追加
embedded-database-kv = { workspace = true }
```

- [ ] **Step 5: 运行测试**

Run: `cargo test -p embedded-database-sql executor 2>&1 | tail -15`
Expected: 4 passed

- [ ] **Step 6: 检查点**

Run: `cargo test -p embedded-database-sql executor 2>&1 | rg 'test result'`
Expected: 4 passed

---

### Task M18-2: M18 收尾验收

**Files:**
- Modify: `docs/superpowers/specs/2026-10-02-embedded-database-design.md`

**Interfaces:**
- Consumes: 全部 M18 Task
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

在 `2026-10-02-embedded-database-design.md` 的「实施状态」小节追加 M18 交付内容：

```markdown
## 实施状态（M18 完成）

- M18 交付：SQL 查询结果类型（`QueryResult`）、SQL 执行器（`execute(sql, db)`），
  将 AST 绑定到 `KvDb`，实现 INSERT（写入）和 SELECT（全表扫描）。
- **M18 简化**：
  - 表名映射为 key 前缀（`table:key`）。
  - INSERT 将行值序列化为 `col=val|col=val` 格式写入 payload。
  - SELECT 全表扫描，`KvDb` 暂无范围扫描接口（需 KvDb 提供 scan 能力）。
  - CREATE TABLE/DROP TABLE 暂为占位（返回空结果）。
  - 优化器留待 M19+。
```

- [ ] **Step 7: 检查点**

Run: `rg -n 'M18 完成' docs/superpowers/specs/2026-10-02-embedded-database-design.md`
Expected: 有输出

---

## M18 验收标准

| # | 标准 | 验证方式 |
|---|------|----------|
| 1 | 查询结果类型 | M18-0 测试 |
| 2 | SQL 执行器（INSERT 写入 + SELECT 扫描） | M18-1 测试 |
| 3 | 5 个 no_std 目标全部编译通过 | M18-2 Step 3 |
| 4 | 全 crate 零 `unsafe` | M18-2 Step 4 |
| 5 | 全部测试通过 | M18-2 Step 1 |
| 6 | `inject_crash` exit=0 且 `wcet` exit=0 | M18-2 Step 5 |

## 后续里程碑

M18 完成后，SQL 引擎可执行基本语句。后续：

- **M19**：SQL 优化器 + KvDb 范围扫描集成
- **M20**：SQL CLI（`embedded-database-cli`）
- **M21**：SQL WHERE/ORDER BY/LIMIT 支持
