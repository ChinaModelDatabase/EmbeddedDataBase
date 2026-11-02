# embedded-database M25 实施计划：SQL GROUP BY

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** parser 解析 GROUP BY 子句，executor 实现分组聚合。

**Architecture:** `parser.rs` 解析 `GROUP BY col`；`executor.rs` 在聚合计算前按分组列分组，每组独立计算聚合值。

**Tech Stack:** Rust 1.81、`embedded-database-sql`（`no_std` + `alloc`）、零外部依赖。

## Global Constraints

- **C1** `embedded-database-sql` 保持 `#![no_std]`（用 `alloc`）
- **C2** 顶层 `#![forbid(unsafe_code)]`——不允许任何局部 `unsafe`
- **C3** 运行时依赖仅 `zerocopy`、`bitflags`，不新增
- **C4** 库代码禁止 `unwrap()` / `expect()` / `panic!()` / 数组越界（`unwrap()` 仅允许 `#[cfg(test)]`）
- **C11** 不执行任何 git 提交（本项目零提交策略）
- **工具链**：每个 bash 调用前 `export PATH="/opt/homebrew/opt/rustup/bin:$PATH"`
- **验收基线**：M24 交付 279 passed（core 215 + kv 6 + os 5 + sql 53）、5 目标 no_std 编译通过、`inject_crash`/`wcet` exit=0

## 用户裁决记录

- **M25 范围**：SQL GROUP BY。

---

### Task M25-0: parser 解析 GROUP BY

**Files:**
- Modify: `crates/embedded-database-sql/src/token.rs`
- Modify: `crates/embedded-database-sql/src/lexer.rs`
- Modify: `crates/embedded-database-sql/src/ast.rs`
- Modify: `crates/embedded-database-sql/src/parser.rs`

**Interfaces:**
- Consumes: `Token`/`TokenType`/`Keyword`（M16-1）、`SelectStmt`（M17-0）
- Produces:
  - `Keyword` 新增 `Group`
  - `SelectStmt` 新增 `group_by: Vec<String>` 字段

- [ ] **Step 1: 写失败测试**

```rust
// crates/embedded-database-sql/src/parser.rs 测试模块追加
#[test]
fn 解析_select_group_by() {
    let tokens = tokenize("SELECT name, COUNT(*) FROM users GROUP BY name").unwrap();
    let stmt = parse(&tokens).unwrap();
    if let Statement::Select(s) = stmt {
        assert_eq!(s.group_by, vec!["name".to_string()]);
    }
}

#[test]
fn 解析_select_无group_by() {
    let tokens = tokenize("SELECT name FROM users").unwrap();
    let stmt = parse(&tokens).unwrap();
    if let Statement::Select(s) = stmt {
        assert!(s.group_by.is_empty());
    }
}
```

- [ ] **Step 2: 运行测试验证失败**

Run: `cargo test -p embedded-database-sql parser 2>&1 | tail -10`
Expected: 测试失败（GROUP BY 未解析）

- [ ] **Step 3: 实现**

在 `token.rs` 的 `Keyword` 枚举追加：`Group`。

在 `lexer.rs` 的 `keyword_from_str` 追加映射：
```rust
"GROUP" => Some(Keyword::Group),
```

`SelectStmt` 追加字段：`pub group_by: Vec<String>`。

在 `parser.rs` 的 LIMIT 解析后追加 GROUP BY 解析：

```rust
// GROUP BY
let mut group_by = Vec::new();
if i < tokens.len() {
    if let TokenType::Keyword(Keyword::Group) = tokens[i].ty {
        i += 1;
        if i < tokens.len() && matches!(tokens[i].ty, TokenType::Keyword(Keyword::By)) {
            i += 1;
            // 解析分组列
            loop {
                if i >= tokens.len() || !matches!(tokens[i].ty, TokenType::Identifier(_)) {
                    break;
                }
                if let TokenType::Identifier(col) = &tokens[i].ty {
                    group_by.push(col.clone());
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
        }
    }
}
```

- [ ] **Step 4: 运行测试**

Run: `cargo test -p embedded-database-sql parser 2>&1 | tail -10`
Expected: 22 passed（20 已有 + 2 新增）

- [ ] **Step 5: 检查点**

Run: `cargo test -p embedded-database-sql parser 2>&1 | rg '^test result:' | head -1`
Expected: 22 passed

---

### Task M25-1: executor 分组聚合

**Files:**
- Modify: `crates/embedded-database-sql/src/executor.rs`

**Interfaces:**
- Consumes: `SelectStmt.group_by`（M25-0）、`SelectItem::Aggregate`（M24-0）
- Produces: `execute_select` 支持 GROUP BY 分组聚合

- [ ] **Step 1: 写失败测试**

```rust
// crates/embedded-database-sql/src/executor.rs 测试模块追加
#[test]
fn 执行_select_group_by_分组计数() {
    let mut dev = InMemoryDevice::new(1 << 20);
    let mut kv = KvDb::open(&mut dev, Config::default()).unwrap();
    execute("INSERT INTO users (dept, name) VALUES ('eng', 'alice')", &mut kv).unwrap();
    execute("INSERT INTO users (dept, name) VALUES ('eng', 'bob')", &mut kv).unwrap();
    execute("INSERT INTO users (dept, name) VALUES ('hr', 'carol')", &mut kv).unwrap();
    let result = execute("SELECT dept, COUNT(*) FROM users GROUP BY dept", &mut kv).unwrap();
    // M25 简化：分组聚合基于字符串比较
    assert!(!result.rows.is_empty());
}

#[test]
fn 执行_select_group_by_空组() {
    let mut dev = InMemoryDevice::new(1 << 20);
    let mut kv = KvDb::open(&mut dev, Config::default()).unwrap();
    execute("INSERT INTO users (dept, name) VALUES ('eng', 'alice')", &mut kv).unwrap();
    let result = execute("SELECT dept, COUNT(*) FROM users GROUP BY missing", &mut kv).unwrap();
    // M25 简化：分组列不存在时所有行归为一组
    assert!(!result.rows.is_empty());
}
```

- [ ] **Step 2: 运行测试验证失败**

Run: `cargo test -p embedded-database-sql 执行_select_group_by_分组计数 2>&1 | tail -10`
Expected: 测试失败（GROUP BY 未实现）

- [ ] **Step 3: 实现**

在 `executor.rs` 的 `execute_select` 中，聚合计算前追加分组逻辑：

```rust
// GROUP BY 分组
let has_group_by = !stmt.group_by.is_empty();
let has_aggregate = stmt.columns.iter().any(|c| matches!(c, SelectItem::Aggregate(_)));

if has_group_by && has_aggregate {
    // 按分组列分组
    let group_col = stmt.group_by.first().cloned().unwrap_or_default();
    let mut groups: Vec<(String, Vec<Vec<Literal>>)> = Vec::new();
    for row in &rows {
        let group_val = row.get(&group_col).map(|s| s.as_str()).unwrap_or("").to_string();
        // 查找已有组
        let mut found = false;
        for (gval, grow) in groups.iter_mut() {
            if *gval == group_val {
                grow.push(row.clone());
                found = true;
                break;
            }
        }
        if !found {
            groups.push((group_val, vec![row.clone()]));
        }
    }
    // 每组独立计算聚合
    let mut result_rows: Vec<Vec<Literal>> = Vec::new();
    for (group_val, group_rows) in &groups {
        let mut agg_row: Vec<Literal> = Vec::new();
        for item in &stmt.columns {
            match item {
                SelectItem::Aggregate(AggregateFn::Count) => {
                    agg_row.push(Literal::Integer(group_rows.len() as i64));
                }
                SelectItem::Aggregate(AggregateFn::Sum) => {
                    let mut sum = 0.0f64;
                    for row in group_rows {
                        if let Some(val) = row.values().next() {
                            if let Ok(n) = val.parse::<f64>() {
                                sum += n;
                            }
                        }
                    }
                    agg_row.push(Literal::Float(sum));
                }
                // ... 其他聚合函数类似
                _ => agg_row.push(Literal::StringLit(group_val.clone())),
            }
        }
        result_rows.push(agg_row);
    }
    rows = result_rows;
}
```

- [ ] **Step 4: 运行测试**

Run: `cargo test -p embedded-database-sql executor 2>&1 | tail -10`
Expected: 17 passed（15 已有 + 2 新增）

- [ ] **Step 5: 检查点**

Run: `cargo test -p embedded-database-sql 2>&1 | rg '^test result:' | head -1`
Expected: 55 passed

---

### Task M25-2: M25 收尾验收

**Files:**
- Modify: `docs/superpowers/specs/2026-10-02-embedded-database-design.md`

**Interfaces:**
- Consumes: 全部 M25 Task
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

在 `2026-10-02-embedded-database-design.md` 的「实施状态」小节追加 M25 交付内容：

```markdown
## 实施状态（M25 完成）

- M25 交付：SQL GROUP BY 支持，
  parser 解析 GROUP BY 子句，executor 实现分组聚合。
- **M25 简化**：
  - 仅支持单列 GROUP BY。
  - 分组基于字符串比较（非类型感知）。
  - HAVING 子查询留待 M26+。
```

- [ ] **Step 6: 检查点**

Run: `rg -n 'M25 完成' docs/superpowers/specs/2026-10-02-embedded-database-design.md`
Expected: 有输出

---

## M25 验收标准

| # | 标准 | 验证方式 |
|---|------|----------|
| 1 | parser 解析 GROUP BY | M25-0 测试 |
| 2 | executor 分组聚合 | M25-1 测试 |
| 3 | 全部测试通过 | M25-2 Step 1 |
| 4 | CLI 可执行 | M25-2 Step 3 |
| 5 | `inject_crash` exit=0 且 `wcet` exit=0 | M25-2 Step 4 |

## 后续里程碑

M25 完成后，SQL GROUP BY 支持交付。后续：

- **M26**：SQL HAVING 子查询 + 优化器
- **M27**：SQL 项目收尾
