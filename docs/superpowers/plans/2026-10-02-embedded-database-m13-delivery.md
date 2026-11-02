# embedded-database M13 实施计划：非代码交付与项目总结

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 交付 LoongArch 实机验证 checklist、认证申报材料模板、试点部署方案、项目总结报告（M1–M12 全部交付物、测试覆盖、已知限制、后续路线图）。

**Architecture:** 纯文档任务，不改代码。每个 Task 生成一份独立交付文档，基于现有设计文档、安全设计文档、LoongArch 构建手册、WCET 规格书、进度账本汇总。

**Tech Stack:** Markdown 文档。

## Global Constraints

- **C11** 不执行任何 git 提交（本项目零提交策略）
- **工具链**：每个 bash 调用前 `export PATH="/opt/homebrew/opt/rustup/bin:$PATH"`
- **验收基线**：M12 交付 223 passed（core 215 + kv 3 + os 5）、5 目标 no_std 编译通过、`inject_crash`/`wcet` exit=0

## 用户裁决记录

- **M13 范围**：非代码交付 + 项目总结。

---

### Task M13-0: LoongArch 实机验证 Checklist

**Files:**
- Create: `docs/superpowers/specs/2026-10-02-embedded-database-loongarch-checklist.md`

**Interfaces:**
- Consumes: `docs/superpowers/specs/2026-10-02-embedded-database-loongarch-build-guide.md`（M5-4）、设计文档 §6.1（龙芯构建要求）
- Produces: LoongArch 实机验证 checklist

- [ ] **Step 1: 写 checklist**

基于 LoongArch 构建手册和设计文档，生成拿到龙芯硬件后的验证 checklist：

```markdown
# embedded-database LoongArch 实机验证 Checklist

> 生成日期：2026-10-02
> 适用：龙芯 3B6000/3C6000（Ⅱ级）、银河麒麟 V10 SP1、统信 UOS、openEuler
> 前置：已阅读 `2026-10-02-embedded-database-loongarch-build-guide.md`

## 1. 环境准备

- [ ] 确认 CPU 型号（`lscpu`）与内核版本（`uname -r`）
- [ ] 确认内核 ≥ 5.19（或使用 musl 静态链接）
- [ ] 确认 Rust 工具链已安装（`rustc --version` ≥ 1.81）
- [ ] 确认 3 个 loongarch target 的 std 已安装

## 2. 构建验证

- [ ] `cargo check -p embedded-database-core --target loongarch64-unknown-linux-gnu`
- [ ] `cargo check -p embedded-database-core --target loongarch64-unknown-linux-musl`
- [ ] `cargo check -p embedded-database-core --target loongarch64-unknown-none`
- [ ] `cargo build --workspace --features embedded-database-core/std`（本机构建）

## 3. 测试验证

- [ ] `cargo test --workspace --features embedded-database-core/std`（预期 223 passed）
- [ ] `cargo run -p embedded-database-bench --bin inject_crash`（预期 exit=0）
- [ ] `cargo run -p embedded-database-bench --bin wcet`（记录实测值）

## 4. WCET 对比

- [ ] 记录 LoongArch 实机 WCET（append_page / btree_insert / commit / db_put / db_get / db_commit）
- [ ] 与 macOS 基线对比（`2026-10-02-embedded-database-wcet-spec.md`）
- [ ] 若差异 > 2 倍，分析原因（缓存、分支预测、内存带宽）

## 5. 加密验证

- [ ] SM4/SM3/HMAC-SM3 标准测试向量在 LoongArch 上通过
- [ ] CBC+HMAC 加密写入→解密读回端到端通过

## 6. 记录与报告

- [ ] 填写验证结果表格（目标、状态、备注）
- [ ] 记录发现的问题与解决方案
- [ ] 更新 WCET 规格书（`2026-10-02-embedded-database-wcet-spec.md`）
- [ ] 更新设计文档「实施状态」小节
```

- [ ] **Step 2: 检查点**

Run: `ls docs/superpowers/specs/2026-10-02-embedded-database-loongarch-checklist.md`
Expected: 文件存在

---

### Task M13-1: 认证申报材料模板

**Files:**
- Create: `docs/superpowers/specs/2026-10-02-embedded-database-certification-template.md`

**Interfaces:**
- Consumes: 安全设计文档（GB/T 22239）、设计文档 §6.3（认证路线）
- Produces: 认证申报材料模板

- [ ] **Step 1: 写模板**

基于安全设计文档和设计文档 §6.3，生成认证申报材料模板：

```markdown
# embedded-database 认证申报材料模板

> 生成日期：2026-10-02
> 依据：设计文档 §6.3 认证路线、GB/T 22239 安全设计

## 1. 信创适配认证（CSTC）

### 1.1 申请材料

| 材料 | 状态 | 说明 |
|------|------|------|
| 产品说明书 |待编写 | 功能、架构、目标平台 |
| 技术白皮书 | 待编写 | 确定性时延、Flash 友好、no_std |
| 测试报告 | 待生成 | 5 目标编译 + 223 测试通过 |
| 源代码 | 可提供 | Apache-2.0，`forbid(unsafe_code)` |
| SBOM | 已生成 | `sbom.json`（CycloneDX） |

### 1.2 适配平台矩阵

| 平台 | CPU | OS | 状态 |
|------|-----|-----|------|
| LoongArch64 | 龙芯 3B6000 | 麒麟 V10 SP1 | 待实机验证 |
| ARM64 | 飞腾 S5000C-E | 统信 UOS | 待实机验证 |
| x86_64 | 海光 | openEuler | macOS 开发机已验证 |

## 2. 源代码自主可控评估（CSTC）

### 2.1 自主代码比例

| 类别 | 行数 | 占比 |
|------|------|------|
| embedded-database-core | ~8000 | ~95% |
| embedded-database-kv | ~50 | <1% |
| embedded-database-embed | ~10 | <1% |
| embedded-database-os | ~200 | ~3% |
| embedded-database-bench | ~500 | ~5% |
| 外部依赖（zerocopy + bitflags） | ~100 | <1% |
| **合计** | **~8900** | **100%** |

### 2.2 供应链审计

| 项 | 状态 |
|----|------|
| 运行时依赖 ≤ 2 个 | ✅ zerocopy + bitflags |
| `cargo vendor` 离线构建 | ✅ 已验证 |
| SBOM 生成 | ✅ CycloneDX |
| `cargo-vet` 审计 | ⏳ 待执行 |
| 全仓 `forbid(unsafe_code)` | ✅ |

## 3. 安全可靠测评 Ⅰ级

### 3.1 申报条件

- 依据《安全可靠测评工作指南 V3.0》
- 有效期三年
- 需通过 CSTC 测试

### 3.2 准备材料

- [ ] 产品定级报告
- [ ] 安全设计文档（GB/T 22239 对照）
- [ ] 测试报告（功能 + 安全）
- [ ] 源代码审计报告

## 4. 网络安全等级保护 GB/T 22239

### 4.1 第二级技术要求对照

见 `2026-10-02-embedded-database-security-design.md` §2。

### 4.2 待补项

| 项 | 优先级 | 状态 |
|----|--------|------|
| 数据加密（SM4） | 中 | ✅ 已实现（§6.1） |
| 密钥管理 | 中 | ✅ 已实现（§6.3） |
| 安全审计日志 | 低 | ✅ 已实现（§6.4，计数式） |
| 剩余信息物理擦除 | 低 | ⏳ 未实现 |
| 可信验证（TPM/TEE） | 低 | ⏳ 留待后续 |
```

- [ ] **Step 2: 检查点**

Run: `ls docs/superpowers/specs/2026-10-02-embedded-database-certification-template.md`
Expected: 文件存在

---

### Task M13-2: 试点部署方案

**Files:**
- Create: `docs/superpowers/specs/2026-10-02-embedded-database-pilot-deployment.md`

**Interfaces:**
- Consumes: 设计文档 §9（M6 试点）、LoongArch 构建手册
- Produces: 试点部署方案

- [ ] **Step 1: 写方案**

```markdown
# embedded-database 试点部署方案

> 生成日期：2026-10-02
> 依据：设计文档 §9 M6（试点现场稳定运行 ≥ 3 个月）

## 1. 试点目标

- 验证 embedded-database 在真实工业场景下的稳定性
- 收集 WCET 实测数据（LoongArch 实机）
- 发现并修复生产环境问题
- 为认证申报提供现场测试报告

## 2. 试点环境

| 项 | 配置 |
|----|------|
| CPU | 龙芯 3B6000 / 3C6000 |
| OS | 银河麒麟 V10 SP1 / 统信 UOS |
| 内核 | ≥ 5.19 |
| 存储 | eMMC / SSD（Flash 友好） |
| 数据库容量 | 按实际场景配置（建议 ≤ 64 MiB） |

## 3. 部署步骤

1. 交叉编译：`cargo build --release --target loongarch64-unknown-linux-musl`
2. 静态链接二进制拷贝到目标机
3. 运行 `inject_crash` 验证崩溃恢复
4. 运行 `wcet` 记录实测时延
5. 集成到宿主应用

## 4. 监控指标

| 指标 | 预算 | 采集方式 |
|------|------|----------|
| append_page p99 | 150μs | wcet 工具 |
| btree_insert p99 | 200μs | wcet 工具 |
| commit p99 | 30μs | wcet 工具 |
| 崩溃恢复成功率 | 100% | inject_crash |
| 磁盘写放大 | < 2.0 | CompactReport |

## 5. 验收标准

- [ ] 连续运行 ≥ 3 个月无数据丢失
- [ ] 崩溃恢复 100% 成功
- [ ] WCET 实测值在预算内
- [ ] 无内存泄漏（长期运行稳定）

## 6. 风险与应对

| 风险 | 应对 |
|------|------|
| LoongArch 内核版本不足 | 使用 musl 静态链接 |
| Flash 磨损超预期 | 监控写放大，调整 compaction 频率 |
| 容量耗尽 | 监控 `CapacityExhausted` 错误，提前扩容 |
```

- [ ] **Step 2: 检查点**

Run: `ls docs/superpowers/specs/2026-10-02-embedded-database-pilot-deployment.md`
Expected: 文件存在

---

### Task M13-3: 项目总结报告

**Files:**
- Create: `docs/superpowers/specs/2026-10-02-embedded-database-project-summary.md`

**Interfaces:**
- Consumes: 全部 M1–M12 交付物、进度账本、设计文档「实施状态」
- Produces: 项目总结报告

- [ ] **Step 1: 写总结报告**

基于进度账本和设计文档，生成项目总结报告：

```markdown
# embedded-database 项目总结报告

> 生成日期：2026-10-02
> 状态：M1–M12 全部完成，223 测试全绿

## 1. 项目概述

embedded-database 是一个纯 Rust、`no_std`、确定性时延、Flash 友好的嵌入式数据库内核，
面向信创与 PLC/嵌入式硬实时场景。

## 2. 里程碑交付

| 里程碑 | 内容 | 测试数 | 状态 |
|--------|------|--------|------|
| M1 | 段格式、CRC-32C、Arena、哈希表、B+tree、页槽分配、写入器、恢复、崩溃注入、WCET | 111 | ✅ |
| M2 | 事务管理、Commit Record、崩溃恢复、ArenaPool、PageMap、`Db::open`/`begin` | 137 | ✅ |
| M3 | Deadline 传播、范围扫描、CompactionBudget、WCET 规格书 | 153 | ✅ |
| M4 | `embedded-database-os` file 后端、真正段合并、写放大统计 | 162 | ✅ |
| M5 | 离线构建、SBOM、LoongArch 手册、GB/T 22239 安全设计 | 167 | ✅ |
| M6 | SM4/SM3/HMAC-SM3、页级加密、密钥管理、审计日志 | 181 | ✅ |
| M7 | `pending_writes` 真正落盘（`Db::flush`） | 189 | ✅ |
| M8 | SM4-CBC、每页 HMAC、审计接线、遗留修复 | 202 | ✅ |
| M9 | key→page 映射、`Txn::get` 读已提交数据、`KvDb` 接口 | 210 | ✅ |
| M10 | key_map 冷启动重建 | 214 | ✅ |
| M11 | 真正段合并（compact_with_budget 完整） | 216 | ✅ |
| M12 | 同段追加 | 223 | ✅ |

## 3. 核心交付物

### 3.1 代码

- 5 个 crate：`embedded-database-core`（~8000 行）、`embedded-database-kv`、`embedded-database-embed`、`embedded-database-os`、`embedded-database-bench`
- 223 测试全绿（core 215 + kv 3 + os 5）
- 全仓 `forbid(unsafe_code)`，`no_std` 路径零 `alloc`
- 5 目标 no_std 编译通过（aarch64 / loongarch64-gnu / loongarch64-musl / x86_64 / loongarch64-none）

### 3.2 文档

- 设计文档（总体架构、M1–M12 实施状态）
- GB/T 22239 安全设计文档
- LoongArch 构建手册
- WCET 规格书
- LoongArch 实机验证 checklist
- 认证申报材料模板
- 试点部署方案

### 3.3 工具

- `inject_crash`：崩溃注入（9 崩溃点 + 基线 + 多段 + compaction）
- `wcet`：WCET 实测
- `scripts/offline-build.sh`：离线构建 + SBOM

## 4. 核心技术

| 技术 | 说明 |
|------|------|
| Append-Only Segment | 100% 顺序写，Flash 友好 |
| CRC-32C + CRC 链 | 数据完整性，跨段校验 |
| Commit Record | 原子提交，单次定长写 |
| SM4-CBC + HMAC-SM3 | 页级加密 + 消息认证（国密标准） |
| 定高两层 B+tree | 深度恒为 2，WCET 可上界 |
| 开放寻址哈希表 | O(1) 查找，固定容量 |
| 同段追加 | 减少段数，提高空间利用率 |
| 崩溃恢复 | 冷启动扫描 + key_map 重建 |

## 5. 已知限制

| 限制 | 影响 | 后续 |
|------|------|------|
| key 含 `0xFF` 的旧数据会错读 | M9-1 已在 put 拒绝新数据 | M10 已记录 |
| 旧段不物理擦除 | 设备空间未回收 | M13 遗留 |
| 写入状态不跨进程持久化 | 重启后 active_writer 丢失 | M12 简化 |
| HMAC 非常数时间比较 | 受 C3 无 `subtle` 依赖约束 | M8 记录 |
| key→page 映射用 `RefCell<Vec>` | 非 FixedTable，O(n) 查找 | M9 简化 |
| LoongArch 实机验证未执行 | 无硬件 | M13 checklist |

## 6. 后续路线图

- **LoongArch 实机验证**：拿到龙芯硬件后执行 checklist
- **认证申报**：CSTC 适配认证、源代码自主可控评估、安全可靠测评
- **试点部署**：现场稳定运行 ≥ 3 个月
- **生产加固**：密钥轮换、审计日志结构化、TPM/TEE 集成
- **M13 遗留**：旧段物理擦除、段回收
```

- [ ] **Step 2: 检查点**

Run: `ls docs/superpowers/specs/2026-10-02-embedded-database-project-summary.md`
Expected: 文件存在

---

### Task M13-4: M13 收尾验收

**Files:**
- Modify: `docs/superpowers/specs/2026-10-02-embedded-database-design.md`

**Interfaces:**
- Consumes: 全部 M13 Task
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

在 `2026-10-02-embedded-database-design.md` 的「实施状态」小节追加 M13 交付内容：

```markdown
## 实施状态（M13 完成）

- M13 交付：LoongArch 实机验证 checklist、认证申报材料模板、试点部署方案、
  项目总结报告。纯文档交付，不改代码。
- **项目状态**：M1–M13 全部完成，223 测试全绿，0 提交。
```

- [ ] **Step 5: 检查点**

Run: `rg -n 'M13 完成' docs/superpowers/specs/2026-10-02-embedded-database-design.md`
Expected: 有输出

---

## M13 验收标准

| # | 标准 | 验证方式 |
|---|------|----------|
| 1 | LoongArch 实机验证 checklist 已生成 | M13-0 |
| 2 | 认证申报材料模板已生成 | M13-1 |
| 3 | 试点部署方案已生成 | M13-2 |
| 4 | 项目总结报告已生成 | M13-3 |
| 5 | 设计文档含「实施状态（M13 完成）」小节 | M13-4 |
| 6 | `inject_crash` exit=0 且 `wcet` exit=0 | M13-4 Step 3 |

## 后续里程碑

M13 完成后，M1–M13 全部交付。剩余工作为**外部依赖**：

- LoongArch 实机验证（需龙芯硬件）
- 认证申报（需 CSTC 提交）
- 试点部署（需现场环境）
