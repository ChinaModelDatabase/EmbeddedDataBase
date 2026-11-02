# embedded-database M20 实施计划：SQL CLI

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 新建 `embedded-database-cli` crate，实现交互式 SQL shell（对齐 sqlite3 CLI 使用习惯），读写文件后端。

**Architecture:** `embedded-database-cli` crate（std 门控），`main()` 打开文件后端（`FileDevice`），进入 REPL 循环读取 SQL 语句，调用 `execute(sql, &mut KvDb)` 执行并格式化输出。

**Tech Stack:** Rust 1.81、`embedded-database-cli`（std）、`embedded-database-os`（file 后端）、`embedded-database-kv`、`embedded-database-sql`。

## Global Constraints

- **C1** `embedded-database-cli` 是 std crate（CLI 场景）
- **C2** 顶层 `#![forbid(unsafe_code)]`——不允许任何局部 `unsafe`
- **C3** 运行时依赖仅 `zerocopy`、`bitflags`，不新增
- **C4** 库代码禁止 `unwrap()` / `expect()` / `panic!()`（`unwrap()` 仅允许 `#[cfg(test)]`；CLI bin 可用 `unwrap`）
- **C11** 不执行任何 git 提交（本项目零提交策略）
- **工具链**：每个 bash 调用前 `export PATH="/opt/homebrew/opt/rustup/bin:$PATH"`
- **验收基线**：M19 交付 253 passed（core 215 + kv 6 + os 5 + sql 27）、5 目标 no_std 编译通过、`inject_crash`/`wcet` exit=0

## 用户裁决记录

- **M20 范围**：SQL CLI。

---

### Task M20-0: embedded-database-cli crate 骨架

**Files:**
- Create: `crates/embedded-database-cli/Cargo.toml`
- Create: `crates/embedded-database-cli/src/main.rs`
- Modify: `Cargo.toml`（workspace members）

**Interfaces:**
- Consumes: 无
- Produces: `embedded-database-cli` crate 骨架

- [ ] **Step 1: 创建 Cargo.toml**

```toml
# crates/embedded-database-cli/Cargo.toml
[package]
name = "embedded-database-cli"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true
description = "embedded-database 交互式 SQL shell"

[[bin]]
name = "embedded-database-cli"
path = "src/main.rs"

[dependencies]
embedded-database-core = { workspace = true }
embedded-database-kv = { workspace = true, features = ["std"] }
embedded-database-sql = { workspace = true, features = ["std"] }
embedded-database-os = { workspace = true }

[lints.rust]
unsafe_code = "forbid"
```

- [ ] **Step 2: 创建 main.rs**

```rust
// crates/embedded-database-cli/src/main.rs
//! embedded-database 交互式 SQL shell。

fn main() {
    println!("embedded-database CLI — 输入 SQL 语句，输入 .exit 退出");
    // M20-1 实现 REPL
}
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
    "crates/embedded-database-cli",
]
```

- [ ] **Step 4: 验证编译**

Run: `cargo check -p embedded-database-cli 2>&1 | tail -5`
Expected: 通过

- [ ] **Step 5: 检查点**

Run: `ls crates/embedded-database-cli/`
Expected: `Cargo.toml` 和 `src/` 目录存在

---

### Task M20-1: REPL 实现

**Files:**
- Modify: `crates/embedded-database-cli/src/main.rs`

**Interfaces:**
- Consumes: `FileDevice`（embedded-database-os）、`KvDb`（embedded-database-kv）、`execute`（embedded-database-sql）、`QueryResult`（embedded-database-sql）
- Produces: 交互式 REPL

- [ ] **Step 1: 写实现**

```rust
// crates/embedded-database-cli/src/main.rs
//! embedded-database 交互式 SQL shell。

use std::io::{self, Write};
use std::path::Path;
use embedded_database_core::Config;
use embedded_database_kv::KvDb;
use embedded_database_os::FileDevice;
use embedded_database_sql::{execute, Literal, QueryResult};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let db_path = if args.len() > 1 {
        args[1].clone()
    } else {
        "embedded-database.db".to_string()
    };

    println!("embedded-database CLI — 数据库: {db_path}");
    println!("输入 SQL 语句，输入 .exit 退出");

    let dev = match FileDevice::open(Path::new(&db_path), 64 << 20) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("无法打开数据库: {e:?}");
            std::process::exit(1);
        }
    };
    let mut dev = dev;
    let mut kv = match KvDb::open(&mut dev, Config::default()) {
        Ok(k) => k,
        Err(e) => {
            eprintln!("无法初始化数据库: {e:?}");
            std::process::exit(1);
        }
    };

    loop {
        print!("embedded-database> ");
        io::stdout().flush().unwrap();

        let mut line = String::new();
        if io::stdin().read_line(&mut line).unwrap() == 0 {
            break; // EOF
        }
        let sql = line.trim();
        if sql.is_empty() {
            continue;
        }
        if sql == ".exit" || sql == ".quit" {
            break;
        }
        if sql == ".help" {
            println!("可用命令:");
            println!("  SELECT ... FROM ... [WHERE ...]");
            println!("  INSERT INTO ... VALUES (...)");
            println!("  CREATE TABLE ... (...)");
            println!("  DROP TABLE ...");
            println!("  .exit / .quit  退出");
            println!("  .help          帮助");
            continue;
        }

        match execute(sql, &mut kv) {
            Ok(result) => print_result(&result),
            Err(e) => eprintln!("错误: {e:?}"),
        }
    }

    let _ = kv.flush();
    println!("再见");
}

/// 格式化输出查询结果。
fn print_result(result: &QueryResult) {
    if result.columns.is_empty() && result.rows.is_empty() {
        println!("OK");
        return;
    }
    // 表头
    if !result.columns.is_empty() {
        let header: Vec<String> = result.columns.iter().map(|c| format!("{c:>10}")).collect();
        println!("{}", header.join(" | "));
        let sep: Vec<String> = result.columns.iter().map(|_| "----------".to_string()).collect();
        println!("{}", sep.join("-+-"));
    }
    // 数据行
    for row in &result.rows {
        let cells: Vec<String> = row.iter().map(|lit| format!("{:>10}", literal_str(lit))).collect();
        println!("{}", cells.join(" | "));
    }
    println!("({} rows)", result.rows.len());
}

/// 字面量转显示字符串。
fn literal_str(lit: &Literal) -> String {
    match lit {
        Literal::Integer(v) => v.to_string(),
        Literal::Float(v) => v.to_string(),
        Literal::StringLit(s) => s.clone(),
        Literal::Null => "NULL".to_string(),
    }
}
```

- [ ] **Step 2: 验证编译**

Run: `cargo check -p embedded-database-cli 2>&1 | tail -5`
Expected: 通过

- [ ] **Step 3: 验证基本运行**

Run: `echo ".exit" | cargo run -p embedded-database-cli 2>&1 | tail -5`
Expected: 输出欢迎信息和「再见」

- [ ] **Step 4: 检查点**

Run: `cargo check -p embedded-database-cli 2>&1 | tail -3`
Expected: 通过

---

### Task M20-2: M20 收尾验收

**Files:**
- Modify: `docs/superpowers/specs/2026-10-02-embedded-database-design.md`

**Interfaces:**
- Consumes: 全部 M20 Task
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

在 `2026-10-02-embedded-database-design.md` 的「实施状态」小节追加 M20 交付内容：

```markdown
## 实施状态（M20 完成）

- M20 交付：`embedded-database-cli` crate（交互式 SQL shell），
  支持 `SELECT`/`INSERT`/`CREATE TABLE`/`DROP TABLE`，对齐 sqlite3 CLI 使用习惯。
- **M20 简化**：
  - REPL 从 stdin 读取，支持 `.exit`/`.quit`/`.help`。
  - 输出格式对齐 sqlite3（表头 + 分隔线 + 数据行 + 行数）。
  - 文件后端默认 64 MiB（`FileDevice`）。
  - WHERE 端到端仅支持 `=`/`<`/`>`（M19 遗留）。
```

- [ ] **Step 6: 检查点**

Run: `rg -n 'M20 完成' docs/superpowers/specs/2026-10-02-embedded-database-design.md`
Expected: 有输出

---

## M20 验收标准

| # | 标准 | 验证方式 |
|---|------|----------|
| 1 | `embedded-database-cli` crate 编译通过 | M20-0 |
| 2 | REPL 实现（stdin 循环 + SQL 执行） | M20-1 |
| 3 | CLI 可执行（`.exit` 输出欢迎信息） | M20-2 Step 3 |
| 4 | 全部测试通过 | M20-2 Step 1 |
| 5 | `inject_crash` exit=0 且 `wcet` exit=0 | M20-2 Step 4 |

## 后续里程碑

M20 完成后，SQL 引擎交付完整（词法+语法+执行+CLI）。后续：

- **M21**：SQL ORDER BY/LIMIT/JOIN 支持
- **M22**：SQL 优化器
- **M23**：WHERE 运算符补齐（`!=`/`<=`/`>=`）
