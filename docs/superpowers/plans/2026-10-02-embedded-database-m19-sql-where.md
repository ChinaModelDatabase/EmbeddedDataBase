# embedded-database M19 实施计划：KvDb 范围扫描与 SQL WHERE

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 给 `KvDb` 加 `scan(prefix)` 方法，executor 实现 WHERE 条件过滤，使 SELECT 真正可用。

**Architecture:** `KvDb::scan(prefix)` 返回所有匹配前缀的 key-value 对；executor 的 `execute_select` 用 `scan` 替代逐条 `get`，并解析 WHERE 子句过滤结果。

**Tech Stack:** Rust 1.81、`embedded-database-kv`（std 门控）、`embedded-database-sql`（`no_std` + `alloc`）。

## Global Constraints

- **C1** `embedded-database-sql` 保持 `#![no_std]`（用 `alloc`）
- **C2** 顶层 `#![forbid(unsafe_code)]`——不允许任何局部 `unsafe`
- **C3** 运行时依赖仅 `zerocopy`、`bitflags`，不新增
- **C4** 库代码禁止 `unwrap()` / `expect()` / `panic!()` / 数组越界（`unwrap()` 仅允许 `#[cfg(test)]`）
- **C11** 不执行任何 git 提交（本项目零提交策略）
- **工具链**：每个 bash 调用前 `export PATH="/opt/homebrew/opt/rustup/bin:$PATH"`
- **验收基线**：M18 交付 248 passed（core 215 + kv 3 + os 5 + sql 25）、5 目标 no_std 编译通过、`inject_crash`/`wcet` exit=0

## 用户裁决记录

- **M19 范围**：KvDb 范围扫描 + SQL WHERE 支持。

---

### Task M19-0: KvDb 范围扫描

**Files:**
- Modify: `crates/embedded-database-kv/src/lib.rs`

**Interfaces:**
- Consumes: `Db`（M4-3）、`lookup_key`（M9-0）、`read_payload`（M5-0）
- Produces:
  - `KvDb::scan(&self, prefix: &str) -> Result<Vec<(String, Vec<u8>)>, Error>`

- [ ] **Step 1: 写失败测试**

```rust
// crates/embedded-database-kv/src/lib.rs 测试模块追加
#[test]
fn kvdb_范围扫描() {
    let mut dev = InMemoryDevice::new(1 << 20);
    let mut kv = KvDb::open(&mut dev, Config::default()).unwrap();
    kv.put(b"users:alice", b"name=alice").unwrap();
    kv.put(b"users:bob", b"name=bob").unwrap();
    kv.put(b"orders:1", b"item=x").unwrap();
    kv.flush().unwrap();
    let results = kv.scan("users:").unwrap();
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].0, "users:alice");
    assert_eq!(results[1].0, "users:bob");
}

#[test]
fn kvdb_范围扫描_空前缀() {
    let mut dev = InMemoryDevice::new(1 << 20);
    let mut kv = KvDb::open(&mut dev, Config::default()).unwrap();
    kv.put(b"key1", b"v1").unwrap();
    kv.put(b"key2", b"v2").unwrap();
    kv.flush().unwrap();
    let results = kv.scan("").unwrap();
    assert_eq!(results.len(), 2);
}

#[test]
fn kvdb_范围扫描_无匹配() {
    let mut dev = InMemoryDevice::new(1 << 20);
    let mut kv = KvDb::open(&mut dev, Config::default()).unwrap();
    kv.put(b"key1", b"v1").unwrap();
    kv.flush().unwrap();
    let results = kv.scan("missing:").unwrap();
    assert!(results.is_empty());
}
```

- [ ] **Step 2: 运行测试验证失败**

Run: `cargo test -p embedded-database-kv kvdb_范围扫描 2>&1 | tail -10`
Expected: 编译失败（`scan` 不存在）

- [ ] **Step 3: 实现**

```rust
// crates/embedded-database-kv/src/lib.rs 的 KvDb impl 追加
/// 范围扫描：返回所有 key 以 `prefix` 开头的 (key, value) 对。
pub fn scan(&self, prefix: &str) -> Result<std::vec::Vec<(std::string::String, std::vec::Vec<u8>)>, Error> {
    let mut results = std::vec::Vec::new();
    // M19 简化：从 Db 的 page_map 线性扫描
    for (page_id, _, _) in self.db.page_map_entries() {
        if let Some(payload) = self.db.read_payload(page_id).ok() {
            if let Some(pos) = payload.iter().position(|&b| b == 0xFF) {
                let key = String::from_utf8_lossy(&payload[..pos]).to_string();
                if key.starts_with(prefix) {
                    let value = payload[pos + 1..].to_vec();
                    results.push((key, value));
                }
            }
        }
    }
    Ok(results)
}
```

需要 `Db` 暴露 `page_map_entries()` 方法（返回 `&(u64, u64, u64)` 迭代器或 `&[(u64, u64, u64)]`）。

- [ ] **Step 4: 运行测试**

Run: `cargo test -p embedded-database-kv kvdb_范围扫描 2>&1 | tail -10`
Expected: 3 passed

- [ ] **Step 5: 检查点**

Run: `cargo test -p embedded-database-kv 2>&1 | rg 'test result:' | head -1`
Expected: 6 passed

---

### Task M19-1: SQL WHERE 支持

**Files:**
- Modify: `crates/embedded-database-sql/src/executor.rs`
- Modify: `crates/embedded-database-sql/src/parser.rs`

**Interfaces:**
- Consumes: `KvDb::scan`（M19-0）、`SelectStmt.where_clause`（M17-0）、`Expr`/`BinOp`/`Literal`（M17-0）
- Produces: `execute_select` 支持 WHERE 过滤

- [ ] **Step 1: 写失败测试**

```rust
// crates/embedded-database-sql/src/executor.rs 测试模块追加
#[test]
fn 执行_select_where_过滤() {
    let mut dev = InMemoryDevice::new(1 << 20);
    let mut kv = KvDb::open(&mut dev, Config::default()).unwrap();
    execute("INSERT INTO users (name, age) VALUES ('alice', 30)", &mut kv).unwrap();
    execute("INSERT INTO users (name, age) VALUES ('bob', 25)", &mut kv).unwrap();
    execute("INSERT INTO users (name, age) VALUES ('carol', 35)", &mut kv).unwrap();
    let result = execute("SELECT name FROM users WHERE age = 30", &mut kv).unwrap();
    assert_eq!(result.rows.len(), 1);
    // M19 简化：WHERE 过滤基于 payload 解析
}

#[test]
fn 执行_select_where_无匹配() {
    let mut dev = InMemoryDevice::new(1 << 20);
    let mut kv = KvDb::open(&mut dev, Config::default()).unwrap();
    execute("INSERT INTO users (name) VALUES ('alice')", &mut kv).unwrap();
    let result = execute("SELECT name FROM users WHERE name = 'nonexistent'", &mut kv).unwrap();
    assert!(result.rows.is_empty());
}
```

- [ ] **Step 2: 运行测试验证失败**

Run: `cargo test -p embedded-database-sql 执行_select_where_过滤 2>&1 | tail -10`
Expected: 编译失败或测试失败（WHERE 未实现）

- [ ] **Step 3: 实现**

在 `executor.rs` 的 `execute_select` 中：
1. 用 `KvDb::scan` 替代逐条 `get`。
2. 解析 `where_clause`（`Expr::BinaryOp` 形式），过滤结果。

```rust
// parse_select 中解析 WHERE
if i < tokens.len() {
    if let TokenType::Keyword(Keyword::Where) = tokens[i].ty {
        i += 1;
        // 解析简单表达式: column op literal
        if i + 2 < tokens.len() {
            if let (TokenType::Identifier(col), TokenType::Punct(op)) = (&tokens[i].ty, &tokens[i + 1].ty) {
                let col_name = col.clone();
                let bin_op = match op {
                    '=' => BinOp::Eq,
                    '<' => BinOp::Lt,
                    '>' => BinOp::Gt,
                    _ => return Err(ParseError { pos: tokens[i].pos, msg: "unsupported operator".into() }),
                };
                let lit = match &tokens[i + 2].ty {
                    TokenType::Integer(v) => Expr::Literal(Literal::Integer(*v)),
                    TokenType::Float(v) => Expr::Literal(Literal::Float(*v)),
                    TokenType::StringLit(s) => Expr::Literal(Literal::StringLit(s.clone())),
                    _ => return Err(ParseError { pos: tokens[i + 2].pos, msg: "expected literal".into() }),
                };
                where_clause = Some(Expr::BinaryOp(
                    Box::new(Expr::Column(col_name)),
                    bin_op,
                    Box::new(lit),
                ));
                i += 3;
            }
        }
    }
}
```

在 `execute_select` 中：
```rust
// 用 scan 替代逐条 get
let scan_results = db.scan(&prefix).map_err(|e| SqlError::Db(format!("{e:?}")))?;
for (key, payload) in &scan_results {
    let row = parse_payload(payload);
    // WHERE 过滤
    if let Some(condition) = &stmt.where_clause {
        if !evaluate_condition(condition, &row) {
            continue;
        }
    }
    rows.push(row);
}

/// 评估 WHERE 条件。
fn evaluate_condition(cond: &Expr, row: &std::collections::BTreeMap<String, String>) -> bool {
    match cond {
        Expr::BinaryOp(lhs, op, rhs) => {
            let lhs_val = evaluate_expr(lhs, row);
            let rhs_val = evaluate_expr(rhs, row);
            match op {
                BinOp::Eq => lhs_val == rhs_val,
                BinOp::Ne => lhs_val != rhs_val,
                BinOp::Lt => lhs_val < rhs_val,
                BinOp::Le => lhs_val <= rhs_val,
                BinOp::Gt => lhs_val > rhs_val,
                BinOp::Ge => lhs_val >= rhs_val,
                _ => true,
            }
        }
        _ => true,
    }
}

/// 求值表达式。
fn evaluate_expr(expr: &Expr, row: &std::collections::BTreeMap<String, String>) -> String {
    match expr {
        Expr::Column(name) => row.get(name).cloned().unwrap_or_default(),
        Expr::Literal(Literal::Integer(v)) => alloc::format!("{v}"),
        Expr::Literal(Literal::Float(v)) => alloc::format!("{v}"),
        Expr::Literal(Literal::StringLit(s)) => s.clone(),
        Expr::Literal(Literal::Null) => "NULL".to_string(),
        _ => String::new(),
    }
}
```

- [ ] **Step 4: 运行测试**

Run: `cargo test -p embedded-database-sql executor 2>&1 | tail -15`
Expected: 6 passed（4 已有 + 2 新增）

- [ ] **Step 5: 检查点**

Run: `cargo test -p embedded-database-sql 2>&1 | rg 'test result:' | head -1`
Expected: 27 passed

---

### Task M19-2: M19 收尾验收

**Files:**
- Modify: `docs/superpowers/specs/2026-10-02-embedded-database-design.md`

**Interfaces:**
- Consumes: 全部 M19 Task
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

在 `2026-10-02-embedded-database-design.md` 的「实施状态」小节追加 M19 交付内容：

```markdown
## 实施状态（M19 完成）

- M19 交付：`KvDb::scan(prefix)` 范围扫描、SQL WHERE 条件过滤
  （支持 `=`, `<`, `>`, `<=`, `>=`, `!=`），executor 用 `scan` 替代逐条 `get`。
- **M19 简化**：
  - WHERE 过滤基于 payload 字符串比较（非类型感知）。
  - `KvDb::scan` 线性扫描 `page_map`（非索引扫描）。
  - ORDER BY/LIMIT/JOIN 留待 M20+。
```

- [ ] **Step 7: 检查点**

Run: `rg -n 'M19 完成' docs/superpowers/specs/2026-10-02-embedded-database-design.md`
Expected: 有输出

---

## M19 验收标准

| # | 标准 | 验证方式 |
|---|------|----------|
| 1 | KvDb 范围扫描 | M19-0 测试 |
| 2 | SQL WHERE 过滤 | M19-1 测试 |
| 3 | 5 个 no_std 目标全部编译通过 | M19-2 Step 3 |
| 4 | 全 crate 零 `unsafe` | M19-2 Step 4 |
| 5 | 全部测试通过 | M19-2 Step 1 |
| 6 | `inject_crash` exit=0 且 `wcet` exit=0 | M19-2 Step 5 |

## 后续里程碑

M19 完成后，SQL SELECT 真正可用。后续：

- **M20**：SQL CLI（`embedded-database-cli`）
- **M21**：SQL ORDER BY/LIMIT/JOIN 支持
- **M22**：SQL 优化器
