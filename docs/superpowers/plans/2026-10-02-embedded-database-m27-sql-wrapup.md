# embedded-database M27 实施计划：SQL 项目收尾

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 修复 SQL 遗留问题、更新交付文档、生成最终报告。

**Architecture:** 纯文档 + 测试任务。修复已知遗留（如 HAVING 聚合列不在 SELECT 列表时过滤被跳过），更新设计文档与交付清单。

**Tech Stack:** Markdown 文档 + Rust 测试。

## Global Constraints

- **C11** 不执行任何 git 提交（本项目零提交策略）
- **工具链**：每个 bash 调用前 `export PATH="/opt/homebrew/opt/rustup/bin:$PATH"`
- **验收基线**：M26 交付 287 passed（core 215 + kv 6 + os 5 + sql 61）、5 目标 no_std 编译通过、`inject_crash`/`wcet` exit=0

## 用户裁决记录

- **M27 范围**：SQL 项目收尾。

---

### Task M27-0: SQL 遗留修复

**Files:**
- Modify: `crates/embedded-database-sql/src/executor.rs`
- Modify: `crates/embedded-database-sql/src/parser.rs`

**Interfaces:**
- Consumes: `SelectStmt.having`（M26-0）、`AggregateFn`（M24-0）
- Produces: HAVING 聚合列不在 SELECT 列表时仍能过滤

- [ ] **Step 1: 写失败测试**

```rust
// crates/embedded-database-sql/src/executor.rs 测试模块追加
#[test]
fn 执行_select_having_聚合列不在select列表() {
    let mut dev = InMemoryDevice::new(1 << 20);
    let mut kv = KvDb::open(&mut dev, Config::default()).unwrap();
    execute("INSERT INTO users (dept, name) VALUES ('eng', 'alice')", &mut kv).unwrap();
    execute("INSERT INTO users (dept, name) VALUES ('eng', 'bob')", &mut kv).unwrap();
    // HAVING 引用 COUNT(*)，但 SELECT 列表只有 dept
    let result = execute(
        "SELECT dept FROM users GROUP BY dept HAVING COUNT(*) > 1",
        &mut kv,
    ).unwrap();
    // M27 修复：HAVING 聚合列不在 SELECT 列表时仍应过滤
    // eng 组有 2 行（>1），应保留
    assert_eq!(result.rows.len(), 1);
}
```

- [ ] **Step 2: 运行测试验证失败**

Run: `cargo test -p embedded-database-sql 执行_select_having_聚合列不在select列表 2>&1 | tail -10`
Expected: 测试失败（HAVING 聚合列不在 SELECT 列表时过滤被跳过）

- [ ] **Step 3: 实现**

在 `executor.rs` 的 HAVING 过滤逻辑中，不依赖 `stmt.columns` 查找聚合列下标，而是直接从 `having` 表达式中提取聚合函数并计算。

- [ ] **Step 4: 运行测试**

Run: `cargo test -p embedded-database-sql executor 2>&1 | tail -10`
Expected: 20 passed（19 已有 + 1 新增）

- [ ] **Step 5: 检查点**

Run: `cargo test -p embedded-database-sql 2>&1 | rg '^test result:' | head -1`
Expected: 62 passed

---

### Task M27-1: 更新交付文档

**Files:**
- Modify: `docs/superpowers/specs/2026-10-02-embedded-database-design.md`
- Modify: `docs/superpowers/specs/2026-10-02-embedded-database-project-summary.md`
- Modify: `docs/superpowers/specs/2026-10-02-embedded-database-delivery-checklist.md`

**Interfaces:**
- Consumes: 全部 M16–M26 交付
- Produces: 文档更新

- [ ] **Step 1: 更新设计文档 SQL 部分**

在设计文档的 SQL 相关章节补充 M16–M26 的实际交付内容。

- [ ] **Step 2: 更新项目总结报告**

更新测试数、代码规模、SQL 引擎功能清单。

- [ ] **Step 3: 更新最终交付清单**

补充 SQL 引擎相关交付物。

- [ ] **Step 4: 检查点**

Run: `rg -n 'M27 完成' docs/superpowers/specs/2026-10-02-embedded-database-design.md`
Expected: 有输出

---

### Task M27-2: M27 收尾验收

**Files:**
- Modify: `docs/superpowers/specs/2026-10-02-embedded-database-design.md`

**Interfaces:**
- Consumes: 全部 M27 Task
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

在 `2026-10-02-embedded-database-design.md` 的「实施状态」小节追加 M27 交付内容：

```markdown
## 实施状态（M27 完成）

- M27 交付：SQL 遗留修复（HAVING 聚合列不在 SELECT 列表时仍能过滤）、
  交付文档更新（设计文档、项目总结报告、最终交付清单）。
- **项目状态**：M1–M27 全部完成，SQL 引擎功能完整（词法+语法+WHERE+ORDER BY+LIMIT+JOIN+聚合+GROUP BY+HAVING+CLI），0 提交。
```

- [ ] **Step 6: 检查点**

Run: `rg -n 'M27 完成' docs/superpowers/specs/2026-10-02-embedded-database-design.md`
Expected: 有输出

---

## M27 验收标准

| # | 标准 | 验证方式 |
|---|------|----------|
| 1 | HAVING 聚合列不在 SELECT 列表时仍能过滤 | M27-0 测试 |
| 2 | 交付文档已更新 | M27-1 |
| 3 | 设计文档含「实施状态（M27 完成）」小节 | M27-2 Step 5 |
| 4 | 全部测试通过 | M27-2 Step 1 |
| 5 | `inject_crash` exit=0 且 `wcet` exit=0 | M27-2 Step 4 |

## 后续里程碑

M27 完成后，SQL 项目收尾。剩余工作为**非代码交付**：

- LoongArch 实机验证（需龙芯硬件）
- 认证申报（需 CSTC 提交）
- 试点部署（需现场环境）
