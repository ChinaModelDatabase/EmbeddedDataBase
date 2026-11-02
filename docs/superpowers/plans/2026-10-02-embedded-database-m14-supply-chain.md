# embedded-database M14 实施计划：cargo-vet 审计与供应链收尾

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 执行 `cargo-vet` 生成 `supply-chain/` 目录（M5-3 只写了 README 文档未实跑），验证离线构建可重复，更新项目总结报告。

**Architecture:** `cargo vet init` 生成审计配置，`cargo vet suggest` 生成建议，记录审计结果。验证 `scripts/offline-build.sh` 可重复执行。

**Tech Stack:** `cargo-vet`、`scripts/offline-build.sh`。

## Global Constraints

- **C11** 不执行任何 git 提交（本项目零提交策略）
- **工具链**：每个 bash 调用前 `export PATH="/opt/homebrew/opt/rustup/bin:$PATH"`
- **验收基线**：M13 交付 223 passed（core 215 + kv 3 + os 5）、5 目标 no_std 编译通过、`inject_crash`/`wcet` exit=0

## 用户裁决记录

- **M14 范围**：cargo-vet 审计 + 供应链收尾。

---

### Task M14-0: cargo-vet 审计

**Files:**
- Create: `supply-chain/`（cargo-vet 生成）
- Modify: `docs/superpowers/specs/2026-10-02-embedded-database-project-summary.md`

**Interfaces:**
- Consumes: `scripts/offline-build.sh`（M5-3）、`Cargo.lock`
- Produces: `supply-chain/` 目录 + 审计结果记录

- [ ] **Step 1: 安装 cargo-vet**

Run: `cargo install cargo-vet 2>&1 | tail -5`
Expected: 安装成功（或已安装）

- [ ] **Step 2: 初始化审计**

Run: `cargo vet init 2>&1 | tail -10`
Expected: 生成 `supply-chain/` 目录

- [ ] **Step 3: 生成建议**

Run: `cargo vet suggest 2>&1 | tail -20`
Expected: 输出依赖审计建议

- [ ] **Step 4: 验证离线构建可重复**

Run: `bash scripts/offline-build.sh 2>&1 | tail -10`
Expected: 离线构建成功

- [ ] **Step 5: 更新项目总结报告**

在 `docs/superpowers/specs/2026-10-02-embedded-database-project-summary.md` 的「供应链审计」小节更新 cargo-vet 状态：

```markdown
| 项 | 状态 |
|----|------|
| 运行时依赖 ≤ 2 个 | ✅ zerocopy + bitflags |
| `cargo vendor` 离线构建 | ✅ 已验证 |
| SBOM 生成 | ✅ CycloneDX |
| `cargo-vet` 审计 | ✅ 已执行（`supply-chain/` 已生成） |
| 全仓 `forbid(unsafe_code)` | ✅ |
```

- [ ] **Step 6: 检查点**

Run: `ls supply-chain/`
Expected: 目录存在且有内容

---

### Task M14-1: M14 收尾验收

**Files:**
- Modify: `docs/superpowers/specs/2026-10-02-embedded-database-design.md`

**Interfaces:**
- Consumes: 全部 M14 Task
- Produces: 设计文档「实施状态」小节更新

- [ ] **Step 1: 全量测试**

Run: `cargo test --workspace --features embedded-database-core/std 2>&1 | tail -15`
Expected: 全部通过

- [ ] **Step 2: 格式化与 lint**

Run: `cargo fmt --all && cargo clippy --workspace --all-targets --features embedded-database-core/std -- -D warnings 2>&1 | tail -20`
Expected: 无警告

- [ ] **Step 3: 工具二进制可用**

Run: `cargo run -p embedded-database-bench --bin inject_crash >/dev/null 2>&1; echo "inject=$?"; cargo run -p embedded-database-bench --bin wcet >/dev/null 2>&1; echo "wcet=$?"`
Expected: `inject=0` 且 `wcet=0`

- [ ] **Step 4: 更新设计文档**

在 `2026-10-02-embedded-database-design.md` 的「实施状态」小节追加 M14 交付内容：

```markdown
## 实施状态（M14 完成）

- M14 交付：`cargo-vet` 审计（`supply-chain/` 已生成）、离线构建可重复验证、
  项目总结报告供应链审计状态更新。
- **项目状态**：M1–M14 全部完成，223 测试全绿，0 提交。
```

- [ ] **Step 5: 检查点**

Run: `rg -n 'M14 完成' docs/superpowers/specs/2026-10-02-embedded-database-design.md`
Expected: 有输出

---

## M14 验收标准

| # | 标准 | 验证方式 |
|---|------|----------|
| 1 | `supply-chain/` 目录已生成 | M14-0 Step 6 |
| 2 | 离线构建可重复执行 | M14-0 Step 4 |
| 3 | 项目总结报告已更新 | M14-0 Step 5 |
| 4 | 设计文档含「实施状态（M14 完成）」小节 | M14-1 Step 4 |
| 5 | `inject_crash` exit=0 且 `wcet` exit=0 | M14-1 Step 3 |

## 后续里程碑

M14 完成后，M1–M14 全部交付。剩余工作为**外部依赖**：

- LoongArch 实机验证（需龙芯硬件）
- 认证申报（需 CSTC 提交）
- 试点部署（需现场环境）
