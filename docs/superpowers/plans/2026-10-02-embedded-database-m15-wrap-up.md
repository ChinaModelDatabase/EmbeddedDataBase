# embedded-database M15 实施计划：项目收尾

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 修复文档一致性（certification-template.md §2.2 cargo-vet 状态）、更新项目总结报告为最终版本、生成最终交付清单。

**Architecture:** 纯文档任务，不改代码。修复已知的文档不一致，汇总 M1–M14 全部交付物。

**Tech Stack:** Markdown 文档。

## Global Constraints

- **C11** 不执行任何 git 提交（本项目零提交策略）
- **工具链**：每个 bash 调用前 `export PATH="/opt/homebrew/opt/rustup/bin:$PATH"`
- **验收基线**：M14 交付 223 passed（core 215 + kv 3 + os 5）、5 目标 no_std 编译通过、`inject_crash`/`wcet` exit=0

## 用户裁决记录

- **M15 范围**：项目收尾 + 文档一致性修复。

---

### Task M15-0: 文档一致性修复

**Files:**
- Modify: `docs/superpowers/specs/2026-10-02-embedded-database-certification-template.md`
- Modify: `docs/superpowers/specs/2026-10-02-embedded-database-project-summary.md`

**Interfaces:**
- Consumes: `supply-chain/`（M14-0）、项目总结报告（M13-3）
- Produces: 文档一致性修复

- [ ] **Step 1: 修复 certification-template.md §2.2**

将 `cargo-vet` 审计状态从「⏳ 待执行」改为「✅ 已执行（`supply-chain/` 已生成）」。

- [ ] **Step 2: 核实项目总结报告**

确认项目总结报告的供应链审计状态与实际一致（M14-0 已更新）。

- [ ] **Step 3: 检查点**

Run: `rg -n 'cargo-vet' docs/superpowers/specs/2026-10-02-embedded-database-certification-template.md`
Expected: 状态为 ✅ 已执行

---

### Task M15-1: 最终交付清单

**Files:**
- Create: `docs/superpowers/specs/2026-10-02-embedded-database-delivery-checklist.md`

**Interfaces:**
- Consumes: 全部 M1–M14 交付物
- Produces: 最终交付清单

- [ ] **Step 1: 写交付清单**

```markdown
# embedded-database 最终交付清单

> 生成日期：2026-10-02
> 状态：M1–M14 全部完成，223 测试全绿，0 提交

## 1. 代码交付

| Crate | 说明 | 行数 | 状态 |
|-------|------|------|------|
| embedded-database-core | 存储内核（no_std） | ~7600 | ✅ |
| embedded-database-kv | 类型化 KV 接口 | ~50 | ✅ |
| embedded-database-embed | 公开嵌入 API | ~10 | ✅ |
| embedded-database-os | Linux file 后端（std） | ~200 | ✅ |
| embedded-database-bench | 基准工具 | ~500 | ✅ |

## 2. 文档交付

| 文档 | 说明 | 状态 |
|------|------|------|
| 设计文档 | 总体架构、M1–M14 实施状态 | ✅ |
| GB/T 22239 安全设计 | 威胁建模、技术要求对照 | ✅ |
| LoongArch 构建手册 | 工具链要求、验证记录 | ✅ |
| WCET 规格书 | 实测数据、确定性保证 | ✅ |
| LoongArch 实机验证 checklist | 拿到硬件后的验证清单 | ✅ |
| 认证申报材料模板 | CSTC + 自主可控 + 安全可靠测评 | ✅ |
| 试点部署方案 | 部署步骤、监控指标 | ✅ |
| 项目总结报告 | M1–M14 交付物、已知限制 | ✅ |
| 最终交付清单 | 本文件 | ✅ |

## 3. 工具交付

| 工具 | 说明 | 状态 |
|------|------|------|
| inject_crash | 崩溃注入（9 崩溃点 + 基线 + 多段 + compaction） | ✅ |
| wcet | WCET 实测 | ✅ |
| scripts/offline-build.sh | 离线构建 + SBOM | ✅ |
| supply-chain/ | cargo-vet 审计配置 | ✅ |
| sbom.json | CycloneDX SBOM | ✅ |

## 4. 测试覆盖

| 类别 | 数量 |
|------|------|
| embedded-database-core | 215 |
| embedded-database-kv | 3 |
| embedded-database-os | 5 |
| **合计** | **223** |

## 5. 核心技术验证

| 技术 | 验证方式 |
|------|----------|
| SM4-CBC | 与 OpenSSL SM4-CBC 逐字节一致 |
| SM3 哈希 | GB/T 32905-2016 标准测试向量 |
| HMAC-SM3 | RFC 2104 + GB/T 32905-2016 |
| CRC-32C | 标准检验值 + 逐位翻转测试 |
| 崩溃恢复 | inject_crash 9 崩溃点全通过 |
| 5 目标 no_std | aarch64 / loongarch64-gnu / loongarch64-musl / x86_64 / loongarch64-none |
| 离线构建 | cargo vendor + SBOM + cargo-vet |

## 6. 已知限制

| 限制 | 影响 | 后续 |
|------|------|------|
| key 含 `0xFF` 的旧数据会错读 | M9-1 已在 put 拒绝新数据 | M10 已记录 |
| 旧段不物理擦除 | 设备空间未回收 | M13 遗留 |
| 写入状态不跨进程持久化 | 重启后 active_writer 丢失 | M12 简化 |
| HMAC 非常数时间比较 | 受 C3 无 `subtle` 依赖约束 | M8 记录 |
| key→page 映射用 `RefCell<Vec>` | 非 FixedTable，O(n) 查找 | M9 简化 |
| LoongArch 实机验证未执行 | 无硬件 | M13 checklist |

## 7. 后续路线图

- **LoongArch 实机验证**：拿到龙芯硬件后执行 checklist
- **认证申报**：CSTC 适配认证、源代码自主可控评估、安全可靠测评
- **试点部署**：现场稳定运行 ≥ 3 个月
- **生产加固**：密钥轮换、审计日志结构化、TPM/TEE 集成
- **M13 遗留**：旧段物理擦除、段回收
```

- [ ] **Step 2: 检查点**

Run: `ls docs/superpowers/specs/2026-10-02-embedded-database-delivery-checklist.md`
Expected: 文件存在

---

### Task M15-2: M15 收尾验收

**Files:**
- Modify: `docs/superpowers/specs/2026-10-02-embedded-database-design.md`

**Interfaces:**
- Consumes: 全部 M15 Task
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

在 `2026-10-02-embedded-database-design.md` 的「实施状态」小节追加 M15 交付内容：

```markdown
## 实施状态（M15 完成）

- M15 交付：文档一致性修复（certification-template.md cargo-vet 状态）、
  最终交付清单（`2026-10-02-embedded-database-delivery-checklist.md`）。
- **项目状态**：M1–M15 全部完成，223 测试全绿，0 提交。项目代码与文档交付已全部完成。
```

- [ ] **Step 5: 检查点**

Run: `rg -n 'M15 完成' docs/superpowers/specs/2026-10-02-embedded-database-design.md`
Expected: 有输出

---

## M15 验收标准

| # | 标准 | 验证方式 |
|---|------|----------|
| 1 | certification-template.md cargo-vet 状态已修复 | M15-0 |
| 2 | 最终交付清单已生成 | M15-1 |
| 3 | 设计文档含「实施状态（M15 完成）」小节 | M15-2 |
| 4 | `inject_crash` exit=0 且 `wcet` exit=0 | M15-2 Step 3 |

## 后续里程碑

M15 完成后，M1–M15 全部交付。剩余工作为**外部依赖**：

- LoongArch 实机验证（需龙芯硬件）
- 认证申报（需 CSTC 提交）
- 试点部署（需现场环境）
