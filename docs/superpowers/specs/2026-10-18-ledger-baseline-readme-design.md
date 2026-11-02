# Spec 1：账本核销 · Git 基线 · README 修正

日期：2026-10-18 · 状态：待实施 · 前置分析：项目全仓分析（同会话）

## 1. 背景

`.superpowers/sdd/progress.md`（866 行进度账本）尾部存在幻影条目：声称已完成的代码在当前工作树中不存在。证据链：

- **两套 M36 编号**：第一套 M36-0..3 = UPDATE / DELETE / WHERE 增强 / LEFT JOIN（简报报告写于 10-05 10:15–10:18，其中 m36-0/1 的 brief/report 已被**同名文件覆盖**）；第二套 M36-0..3 = Boolean/Blob、6 个标量函数、跨平台文档、收尾验收（10-05 15:37–15:45 写就，与当前代码吻合）。
- **M37-0..7**（UNION / CASE WHEN / CAST / OFFSET / EXISTS / ALTER TABLE / INDEX / WITH）报告写于 10-05 11:02–11:45，声称 sql 测试 75→89、workspace 310 全绿；当前实测 sql 72、workspace 301，`ast.rs` 的 `Statement` 枚举无任何对应变体。
- **M38**（操作手册更新到 M37）：操作手册中无 UPDATE/DELETE/UNION/CASE WHEN/ALTER TABLE 字样。
- 时间戳证据：`crates/embedded-database-sql/src/*.rs` 于 10-05 15:25–15:45 被重写，晚于全部 M37 报告（11:xx），推断为第二套 M36 实现时从更早的基线重写、覆盖丢失了第一套 M36 + M37 + M38 的成果。
- 另：全部工作处于**零提交**状态（分支 `feat/m1-core-skeleton` 无任何 commit），2,927 个文件 untracked——这是 M37 式丢失得以发生的环境条件。
- README 状态行过时：声称 "M1–M28、288 测试、0 提交"，实际为 M36、301 测试。
- `supply-chain/audits.toml` 为空（仅 `[audits]` 节），38 条依赖全靠 config.toml exemptions 放行，README 却暗示 "CI 可配置 cargo-vet 门禁"。

## 2. 目标与范围

**目标**：使"文档声称"与"工作树事实"一致，并让全部工作进入版本管理。

**范围内**：

1. 账本核销：尾部深核（M29–M38）+ 全文快检（M1–M28），幻影条目加核销注记，文末新增核销汇总 section。
2. Git 基线：三段式提交（源码 → vendor → 修正），启用"每逻辑单元完成即提交"策略。
3. README 修正：状态行 + cargo-vet 小节，与实测一致。
4. 清理残留：删除 `.cargo/config.toml.moved-by-offline-build`（与 `config.toml` 内容相同的离线构建脚本遗留备份）。

**范围外**：M37/第一套 M36 功能重做（留待后续任务决策）；真实供应链审计（Spec 2）；embed/flash 实装（Spec 3）；WCET 数据重测；project-summary / 设计文档改动（经核实已如实）。

## 3. 核销方法

- **尾部深核（M29–M38，约 55 条 [x]）**：逐条对照工作树验证——测试数 vs `rg -c '#\[test\]'` 实测、声称的代码/文档改动是否存在、关键特性 grep；每条给出「已核实 / 幻影 / 存疑」判定。
- **全文快检（M1–M28，约 65 条）**：廉价不变量——里程碑测试数与 project-summary 里程碑表一致性、关键模块/特性存在性抽查（大端 `compile_error`、CRC 黄金值测试、inject_crash 崩溃点枚举、forbid(unsafe_code) 逐文件等）；任何疑点就地升级为深核。
- **已知幻影清单（16 条，核销时确认并可扩充）**：
  - 第一套 M36-0、M36-1、M36-2、M36-3、「M36 全部完成」（5 条）
  - M37-0 … M37-7、「M37 全部完成」（9 条）
  - M38 两条（2 条）
- **两套 M36 编号**：不重命名、不删除任何历史条目；在核销汇总中说明编号冲突事实。
- **新幻影处置**：核销中若发现新的幻影条目，同样加注记并计入汇总；不扩大范围、不现场重做。

## 4. 账本修改格式

幻影条目保留原 `- [x]` 行不动，在其下追加一行注记：

```markdown
- [x] **M37-0: UNION / UNION ALL — 完成**
  - ⚠️ 核销 2026-10-18：代码未在工作树中核实（`ast.rs` 无 UnionStmt，sql 实测 72 ≠ 账本 77），疑似被第二套 M36 重写覆盖；重做与否待后续决策。
```

`progress.md` 文末新增 section：

```markdown
## 2026-10-18 核销
- 判定汇总：已核实 X 条 / 幻影 16 条 / 存疑 Y 条
- 幻影条目清单（逐条 + 证据）
- 两套 M36 编号说明
- 全文快检结论
- 遗留待办：第一套 M36 + M37 + M38 是否重做（另行走 Spec 3 或新任务）
```

账本被 `.superpowers/sdd/.gitignore` 整体忽略，本次改为仅跟踪 progress 账本（见 §5），使核销结果进入版本管理。

## 5. Git 基线（三段式）

**前置改动**：

- `.superpowers/sdd/.gitignore` 内容改为：
  ```
  *
  !progress.md
  !progress-m28.md
  ```
  （370 个 brief/report 过程文件保持忽略；仅跟踪核销主账本与 m28 终审。第一套 M36 的报告已被同名覆盖、无法复原，过程文件不作为机构记忆依赖。）
- 删除 `.cargo/config.toml.moved-by-offline-build`。

**提交序列**（消息为初步拟定，执行时可微调）：

| # | 消息 | 内容 |
|---|------|------|
| ① | `chore: 基线快照（源码、配置与文档）` | crates/、docs/、.github/、scripts/、supply-chain/、sbom.json、crates/\*/\*.cdx.json、根配置（Cargo.toml、Cargo.lock 除外）、README（原样）、**未核销状态的 progress\*.md**——保发现时状态 |
| ② | `chore: 基线快照（vendor 依赖与 Cargo.lock）` | vendor/（约 60M、2,900 文件）+ Cargo.lock——第三方代码单独成提交，呼应信创交付的可审计性 |
| ③ | `docs: 账本核销与 README 状态修正` | 账本核销注记 + README 修正，核销内容以 diff 形式可审阅 |

提交①之前不改动任何被跟踪文件的内容（§3、§6 的修改全部发生在基线之后）；本 spec 文档与后续实施计划（plan）文档均为新增文件，随提交①一并入库（或先行单独提交，均不改变"3 个基线提交"的构成）。

**提交策略**：基线完成后启用"每逻辑单元完成即提交"——spec、plan、每组核销/改动各自成提交，杜绝零提交式丢失重演。

## 6. README 修正

- 状态行改为：
  ```markdown
  - 状态：M1–M36 已核实完成，301 测试全绿（core 216 + kv 8 + os 5 + sql 72）；M37/M38 账本条目已核销为未实现（详见 .superpowers/sdd/progress.md）
  ```
  （核销若发现 M1–M28 侧新幻影，状态行措辞随之收窄。）
- cargo-vet 小节改为如实表述：当前 `supply-chain/audits.toml` 无真实审计条目，38 条依赖靠 exemptions 放行，真实审计为独立后续任务；删除 "CI 中可配置 cargo vet 作为门禁" 的暗示（CI 实际未配置）。
- WCET 表、构建说明、离线构建说明不动。

## 7. 错误处理与风险

- **核销发现更多幻影**：按 §3 新幻影处置——注记 + 汇总，不扩大范围。
- **快检无法判定**（如历史测试数无从复核）：标注「存疑：无法复核」而非强行判定。
- **提交内容遗漏**：执行 `git status --short --untracked-files=all` 与 `.gitignore` 规则交叉核对，确保除 sdd 过程文件与 target/ 外无遗漏；提交后以 `git status` 干净为门禁。
- **账本跟踪引入意外噪音**：`!progress*.md` 否定规则只匹配根级 progress 文件；若 sdd 下出现其他 progress 前缀文件，人工确认后再决定。

## 8. 验收标准

1. `cargo fmt --check`、`cargo clippy -- -D warnings`、`cargo test --features embedded-database-core/std` 全过，**301 passed**（与核销声称一致）。
2. README 状态行数字与 `rg -c '#\[test\]'` 实测一致；cargo-vet 小节不再含失实暗示。
3. `git log --oneline` 恰为 3 个基线提交（在无其他提交的前提下），`git status` 干净，`git ls-files .superpowers/sdd/` 列出 `progress.md` 与 `progress-m28.md`。
4. 16 条已知幻影全部带核销注记；`progress.md` 文末存在「2026-10-18 核销」section 且含判定汇总、幻影清单、编号冲突说明、快检结论、遗留待办。
5. `.cargo/config.toml.moved-by-offline-build` 已删除，离线构建不受影响（`cargo build --offline` 可过）。
6. 后续提交策略已在账本核销 section 中记录启用。
