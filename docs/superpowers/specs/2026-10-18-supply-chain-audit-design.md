# Spec 2：真实供应链审计（cargo-vet 本地审计 + cargo-deny 门禁）

日期：2026-10-18 · 状态：待实施 · 前置：Spec 1（账本核销 · Git 基线 · README 修正）已完成（commit `b01fed8`）

## 1. 背景

Spec 1 核销判定 M14（「cargo-vet 审计」）为**存疑**：`supply-chain/audits.toml` 为空（仅 `[audits]` 节），`supply-chain/imports.lock` 只有头部注释，`supply-chain/config.toml` 的 38 条 `[[exemptions.*]]` 仅含 `version` + `criteria`（无 `notes`、无理由、无期限）——即"审计"实际是**豁免放行**。

同时，文档在多处把该状态表述为已完成审计：

- `docs/superpowers/specs/2026-10-02-embedded-database-certification-template.md:40-47`：`cargo-vet 审计 | ✅ 已执行（supply-chain/ 已生成）`
- `docs/superpowers/specs/2026-10-02-embedded-database-project-summary.md:115-122`：同表
- `docs/superpowers/specs/2026-10-02-embedded-database-security-design.md:100`：`供应链审计 | cargo vendor + cargo-vet + SBOM | M5-3`
- `docs/superpowers/specs/2026-10-02-embedded-database-delivery-checklist.md:95`：`离线构建 | cargo vendor + SBOM + cargo-vet`

本 spec 的目标不是"再写一遍文档"，而是让「审计」二字在仓库中变成**可机器验证的事实**，再让文档与之逐字对齐。

### 1.1 实测事实（2026-10-18，本机）

| 事实 | 证据 |
|---|---|
| 依赖总数 45 = 7 workspace 成员 + 38 第三方 | `Cargo.lock` 45 个 `[[package]]`；`vendor/` 38 个顶层 crate 目录 |
| **deploy 图（normal+build，不含 dev）第三方仅 7 个** | `cargo tree --workspace -e normal,build`：`bitflags 2.13.2`、`proc-macro2 1.0.107`、`quote 1.0.47`、`syn 2.0.119`、`unicode-ident 1.0.26`、`zerocopy 0.8.59`、`zerocopy-derive 0.8.59` |
| dev 树 = 31 个（proptest 及其传递依赖），不进交付物 | 38 − 7 = 31；`Cargo.lock` 中 proptest/rand/tempfile/bit-set/rusty-fork 等 |
| `cargo vet check` 在 vendored 模式**离线**可跑且通过 | `cargo vet 0.10.2`；实测输出 `Vetting Succeeded (38 exempted)`，`supply-chain/*` 无改动 |
| cargo-vet 0.10.2 的豁免**不支持 `expires` 字段** | 写入 `expires = "2027-04-30"` 后 `cargo vet check` 仍 `Vetting Succeeded`，但配置被重写、该字段被静默剥离 |
| `cargo deny check licenses` 可离线跑，但**默认配置必然失败** | `cargo deny 0.20.2`；实测 `licenses FAILED`，且日志 `unable to find a config path, falling back to default config` |
| `cargo deny check advisories` **必须网络** | 实测报错：advisory-db 目录不存在（`$CARGO_HOME/advisory-dbs/advisory-db-*`），需克隆 RustSec advisory-db |
| 38 个第三方的许可证全为宽松型组合 | 22× `MIT OR Apache-2.0`、4× `Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT`、4× `Apache-2.0 OR MIT`、3× `MIT/Apache-2.0`、2× `BSD-2-Clause OR Apache-2.0 OR MIT`、1× `MIT OR Apache-2.0 OR LGPL-2.1-or-later`、1× `Apache-2.0 / MIT`、1× `(MIT OR Apache-2.0) AND Unicode-3.0` |
| `cargo deny --offline check bans` 与 `check sources` 离线通过 | 实测 `bans ok`、`sources ok`（默认配置）；`Cargo.lock` 无重名多版本（`rg '^name = ' Cargo.lock \| sort \| uniq -d` 为空） |
| CI 无任何审计/SBOM 校验 job；trigger 仅 `main` + PR | `.github/workflows/ci.yml:2-5`（`push: branches: [main]`），jobs: `host`/`nostd`/`big-endian-guard-expected-failure` |
| 仓库当前**无 remote**，CI 无法在本地真实触发 | `git remote -v` 为空；单一分支 `feat/m1-core-skeleton` |
| `.cargo/config.toml` 强制 vendored sources | 4 行，`replace-with = "vendored-sources"` → `directory = "vendor"` |

**修订（2026-10-18）**：上表原称豁免支持 `expires` 字段——**实测有误**。复现：写入 `expires` 后运行一次 `cargo vet check`，字段即被静默剥离（`rg -c '^expires' supply-chain/config.toml` 由 1 → 0）；cargo-vet 0.10.2 无此字段（二进制中唯一的 "expires" 是 h2 HTTP 头）。复审日期改由每条豁免的 `notes` 承载（`notes` 经实测可经受重写而不丢失）。

**修订（2026-10-18）**：licenses allowlist 移除未使用的 `Apache-2.0 WITH LLVM-exception` 独立条目（无依赖单独使用该表达式，保留会反复触发 `warning[license-not-encountered]`）；实测遇到的 LLVM-exception 变体均在 `OR` 组合内，其另一项（Apache-2.0 / MIT）已在 allowlist，无需独立条目。

## 2. 目标与范围

**目标**：deploy 图的第三方依赖不再靠无据豁免，而是**有取证、有记录、可复查**；公告/许可/来源三类风险有硬门禁；文档表述与 `supply-chain/` 实际内容一致。

**范围内**：

1. cargo-vet：`[policy]` 表 + 7 个 deploy 第三方依赖的审计处理（本地取证 audit 条目 / 上游 import）+ 豁免收窄（补 `notes`，复审日期写入 `notes`）。
2. cargo-deny：新建 `deny.toml`，licenses allowlist + bans + sources + advisories。
3. 本地与 CI 的审计入口：`scripts/supply-chain-audit.sh` + `.github/workflows/ci.yml` 新增 `supply-chain` 硬门禁 job + trigger 修正。
4. 文档如实化：certification-template / project-summary / security-design / delivery-checklist / README 的相关表述。
5. 账本补记：`.superpowers/sdd/progress.md` 的 M14 条下追加核销注记（存疑 → 已落实，附证据）。

**范围外**：逐行人工审计 syn 等大型 crate；`trust`（cargo-vet 可信发布者）机制；SBOM 覆盖面修正（`sbom.json` 只含 9 个组件、缺 sql/cli，属独立任务）；源码改动（本 spec 预期**零源码改动**）；M37/第一套 M36 是否重做。

## 3. 审计策略

### 3.1 criteria 与 policy

`supply-chain/config.toml` 增加（7 个成员 crate 逐一列出，无省略）：

```toml
[policy.embedded-database-core]
criteria = "safe-to-deploy"

[policy.embedded-database-kv]
criteria = "safe-to-deploy"

[policy.embedded-database-os]
criteria = "safe-to-deploy"

[policy.embedded-database-embed]
criteria = "safe-to-deploy"

[policy.embedded-database-sql]
criteria = "safe-to-deploy"

[policy.embedded-database-cli]
criteria = "safe-to-deploy"

[policy.embedded-database-bench]
criteria = "safe-to-deploy"
```

（成员 crate 之间的依赖由 cargo-vet 视为第一方，无需审计。）

### 3.2 deploy 第三方（7 个）的审计处理

**本地实质性审计**（3 个）：`zerocopy` 0.8.59、`zerocopy-derive` 0.8.59、`bitflags` 2.13.2 —— 写真实 `[[audits.*]]` 条目：

```toml
[[audits.bitflags]]
who = "embedded-database maintainers"
criteria = "safe-to-deploy"
version = "2.13.2"
notes = "本地实质性审计 2026-10-18，按 Spec 2 §3.3 八项清单取证：build.rs 无；unsafe 3 处（external.rs:2、internal.rs:1、lib.rs:1），均为位运算/类型转换用途；无网络/进程/fs/env 访问；no_std 兼容；MIT OR Apache-2.0。深度：结构性取证 + 关键路径抽样，未逐行。"
```

`notes` 必须**自包含**（结论 + 深度声明），不依赖外部路径；本 spec 的 §3.3 只是方法说明。

**上游 import 优先**（4 个）：`proc-macro2`、`quote`、`syn`、`unicode-ident` —— 先执行 `cargo vet import <peer>`（需网络，best-effort）。**降级规则（确定性，不依赖 import 成败）**：

1. import 覆盖 → 该 crate 由 `imports.lock` 提供审计路径，不写本地条目；
2. import 不可得或未覆盖 → 按同一取证清单做本地审计并写 `[[audits.*]]`；
3. 仍无法取证（例如工具/网络受限）→ 保留 `[[exemptions.*]]`，但必须带 `notes`（写明原因与复审日期）。

无论走哪条分支，**本文档 §6 的验收标准不变**：`cargo vet check` 必须通过，且 deploy 图的每一个第三方 crate 都能解释其"被信任"的来源（audit / import / 带据豁免）。

### 3.3 固定取证清单（本地审计每条必填）

对每个本地审计的 crate 逐项取证并记录结论：

1. `build.rs` 是否存在及其行为（是否执行命令、访问网络/文件系统、读环境变量）；
2. `unsafe` 面：出现次数与关键位置、用途分类（FFI / 未初始化内存 / 类型转换等）；
3. 网络、进程、文件系统、环境变量访问（grep 取证）；
4. `no_std` / `alloc` 使用情况与 feature 门控；
5. 许可证 SPDX 表达式与 `LICENSE*` 文件一致性；
6. 版本固定：`Cargo.lock` 与 `vendor/` 版本一致（本轮为精确版本固定，非 `^` 范围）；
7. 变更记录/发布历史抽查（crates.io 记录，需网络时说明取证时间）；
8. 测试与 CI 情况（是否存在测试、CI 配置）。

**深度声明（必须写入每条 `notes`）**：这是**结构性取证 + 关键路径抽样**，不是逐行审计；大 crate（如 `zerocopy`，`src/impls.rs` 一处 200+ `unsafe`）明确写明未逐行覆盖。

### 3.4 豁免收窄（31 个 dev 树 + 任何降级项）

- criteria 精确化：dev-only crate 用 `safe-to-run`（不放行 `safe-to-deploy`）。
- 每条豁免必须带 `notes`（说明"dev-only，不进入交付物"或降级原因）。
- 每条豁免必须带 `notes`，其中写明豁免原因与复审日期（本轮统一 `2027-04-30`；复审即重跑审计流程）。**不得**使用 `expires` 字段——cargo-vet 0.10.2 不支持，会在下一次配置重写时被静默剥离（见 §1.1 修订注记）。

### 3.5 明确不做

- 不引入 cargo-vet `trust`（可信发布者）机制——它会以发布者信任替代逐 crate 审计，与本 spec「有取证」的目标相悖；若将来引入需另立 spec。
- 不对 dev 树做人工审计（收益低、不进交付物），只做带据豁免。

## 4. 交付物

1. `supply-chain/config.toml`：`[policy]` + 豁免收窄（`notes` 含复审日期 + criteria 精确化）+ 移除已被 audit/import 覆盖的豁免。
2. `supply-chain/audits.toml`：本地实质性审计条目（至少 `zerocopy`、`zerocopy-derive`、`bitflags`，以及 §3.2 降级分支覆盖到的 crate）。
3. `supply-chain/imports.lock`：`cargo vet import` 结果（若 import 成功；失败则保持头部注释并在报告中说明）。
4. `deny.toml`（新建，基于 `cargo deny init` 模板裁剪）：
   - `[licenses]` allowlist = `MIT`、`Apache-2.0`、`BSD-2-Clause`、`Unicode-3.0`（覆盖 `vendor/*/Cargo.toml` 中实际出现的许可证表达式；`OR` 表达式中任一允许项即视为合规；提及 `LLVM-exception` 的变体仅出现在 `OR` 组合内，其另一项（Apache-2.0 / MIT）已在 allowlist，故无需独立条目——单独列出会触发 `warning[license-not-encountered]`，实测）；
   - `[bans]`：`multiple-versions` 与 `wildcards` 设为 `deny`；
   - `[sources]`：仅允许 `crates.io`（与 vendored 交付一致）；
   - `[advisories]`：`ignore` 为空（不静默放行）；遇真实公告时逐条记录处置。
5. `scripts/supply-chain-audit.sh`（新建，遵循 `scripts/offline-build.sh` 的风格：显式 PATH、失败即停、输出分区）：
   - 离线可跑：`cargo vet check`、`cargo deny check licenses bans sources`；
   - 需网络：`cargo deny check advisories`（先探测 advisory-db 是否存在，不存在则明确提示"需网络"而非静默跳过）；
   - 不修改任何被跟踪文件。
6. `.github/workflows/ci.yml`：
   - 新增 `supply-chain` job：安装 `cargo-vet@0.10.2`、`cargo-deny@0.20.2`（`cargo install --locked`，不引入第三方 action），跑 `cargo vet check` + `cargo deny check`（全部四类，网络可用）；
   - trigger 改为**所有分支 push + pull_request**。
7. 文档如实化（逐处修改，措辞以 `supply-chain/` 实际内容为准）：
   - `certification-template.md:40-47`、`project-summary.md:115-122`：`cargo-vet 审计 ✅` → 准确表述（本地审计覆盖 deploy 图 7 个第三方 + cargo-deny 门禁 + 豁免清单及复审日期）；
   - `security-design.md:100`、`delivery-checklist.md:95`：同步；
   - `README.md` cargo-vet 小节：补一句当前审计覆盖范围与 `scripts/supply-chain-audit.sh` 入口。
8. `.superpowers/sdd/progress.md`：M14 条下追加 `⚠️ 核销 2026-10-18（Spec 2）` 注记（存疑已落实，附命令与结论）。

## 5. 执行顺序（供实施计划参考）

1. cargo-deny：`deny.toml` → 三类离线检查通过 → 记录四类检查（含 advisories 联网）结果。
2. cargo-vet：取证 3 个本地审计 crate → `cargo vet import` 尝试 4 个 build 依赖 → 按降级规则收尾 → `cargo vet check` 通过 → 豁免收窄（`notes` 含复审日期）。
3. 脚本 + CI job + trigger。
4. 文档与账本更新。
5. 门禁复核（§6）与提交（每逻辑单元一提交：审计配置 / CI+脚本 / 文档+账本）。

## 6. 验收标准

1. `cargo vet check` 通过（离线即可），且 **deploy 图 7 个第三方 crate 零无声豁免**——每个都有 audit、import 或带 `notes`（含复审日期）的豁免解释。
2. `cargo deny check` 四类全绿（advisories 在联网环境）；`cargo deny --offline check licenses bans sources` 在 vendored 模式全绿。
3. `deny.toml` 的 licenses allowlist 覆盖 `vendor/*/Cargo.toml` 中实际出现的许可证表达式（提及 `LLVM-exception` 的变体经其 `OR` 备选项覆盖，无独立条目）。`[licenses.clarify]` 仅允许用于**把非标准写法更正为等价标准 SPDX**（不改变许可判定，且需在报告中记录）；不得用 clarify 或 `ignore` 放行任何原本不在 allowlist 的许可证。`[advisories] ignore` 为空。
4. CI trigger 改为「所有分支 push + pull_request」，`supply-chain` job 存在且命令与 §4.6 一致。**验证方式**：本仓库无 remote，CI 无法本地真跑——以 workflow YAML 静态核对（事件过滤器 + job/step 内容）为验收依据，并在交付清单中登记"首次推送后需真跑一次确认"。
5. **源码零改动**：`cargo fmt --all --check` 干净、`cargo clippy --workspace --all-targets --features embedded-database-core/std -- -D warnings` 干净、`cargo test --workspace --features embedded-database-core/std` 仍 **301 passed**。
6. `supply-chain/` 的每条豁免都含**非空** `notes`，且 `notes` 中含复审日期 `2027-04-30`（不使用 `expires` 字段）；`supply-chain/audits.toml` 至少含 `zerocopy`、`zerocopy-derive`、`bitflags` 三条本地审计，其 `notes` 含取证结论与深度声明。
7. 文档四处表述与 `supply-chain/` 实际一致（无"已执行审计"式夸大）；账本 M14 注记存在。
8. 提交序列遵循"每逻辑单元完成即提交"，且 `git status` 干净。

## 7. 风险与错误处理

- **上游 import 完全不可得**：按 §3.2 降级到本地审计；若连本地取证也受限（例如 crates.io 元数据不可达），则保留带 `notes` 的豁免，并在文档中如实写明"该 4 个 crate 目前为带据豁免，复审日期 2027-04-30"——不允许留下无据豁免。
- **`cargo vet check` 会自动清理不再使用的豁免**（这是预期且希望的行为：为 `zerocopy` 等写入 audit 后，其旧豁免自动消失）。执行节奏：一次只改一类内容，随后 `git diff supply-chain/` 复核被清理的条目是否符合预期；清理结果本身也要进入提交，不得静默丢弃。
- **advisories 首次运行需克隆 advisory-db**（数十 MB，需网络）：CI 上正常；本地由脚本明确提示，不静默跳过。
- **许可证表达式差异**：`Apache-2.0 / MIT`（斜杠写法）等非标准 SPDX 可能触发解析告警；若 `cargo-deny` 报无法解析，逐条在 `[licenses.clarify]` 中按实际文件修正（需在报告中记录）。
- **命令形式必须可复现**：`cargo vet check` 无需任何离线开关即可在 vendored 模式跑通（实测）；`--offline` 不是 cargo-vet 的合法参数（实测报 `unexpected argument`），`--frozen`/`--locked`/`--no-minimize-exemptions` 不被 `check` 接受。脚本与文档只记录实测可用的命令形式。

## 8. 与其它 spec 的关系

- 本 spec 完成后，M14「存疑」在账本中转为「已落实」；Spec 1 中记录的"真实审计归 Spec 2"交付闭环。
- 后续 Spec 3a（embed 公开 API）/ 3b（Flash 介质后端）与本 spec 无依赖关系。
- SBOM 覆盖面（`sbom.json` 仅 9 个组件、缺 `embedded-database-sql`/`embedded-database-cli`）不在本 spec 范围内，建议作为独立小任务登记。
