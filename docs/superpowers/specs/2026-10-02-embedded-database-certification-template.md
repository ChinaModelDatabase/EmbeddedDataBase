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
| embedded-database-embed | ~362 | ~4% |
| embedded-database-os | ~200 | ~3% |
| embedded-database-bench | ~500 | ~5% |
| 外部依赖（zerocopy + bitflags） | ~100 | <1% |
| **合计** | **~8900** | **100%** |

（Spec 3a 后 embedded-database-embed 为实测行数；合计与占比为历史近似值，未重算。）

### 2.2 供应链审计

| 项 | 状态 |
|----|------|
| 运行时依赖 ≤ 2 个 | ✅ zerocopy + bitflags |
| `cargo vendor` 离线构建 | ✅ 已验证 |
| SBOM 生成 | ✅ CycloneDX |
| `cargo-vet` 审计 | ✅ deploy 图 7 个依赖来源明确：本地审计 `zerocopy`/`zerocopy-derive`/`bitflags`/`proc-macro2`/`syn`/`unicode-ident`；上游 import 覆盖 `quote` |
| `cargo-deny` 门禁 | ✅ advisories / licenses / bans / sources 四类（`deny.toml`，CI `supply-chain` job） |
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
