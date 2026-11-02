# Spec 1 实施计划：账本核销 · Git 基线 · README 修正

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把账本声称与工作树事实对齐（16 条幻影条目核销注记 + 全文快检），并让全部工作进入版本管理（三段式基线提交），使 README 状态如实。

**Architecture:** 先改跟踪规则与清理残留 → 提交①（源码/配置/文档，含未核销状态的账本）→ 提交②（vendor + Cargo.lock）→ 尾部深核写入注记 → 全文快检 → 核销汇总 section → README 修正 → 提交③。提交①之前不改动任何工作树文件的内容（保证基线是"发现时状态"）。

**Tech Stack:** git 2.x、Rust 1.81.0（`rust-toolchain.toml` 锁定）、cargo 工具链位于 `/opt/homebrew/opt/rustup/bin`。

## Global Constraints

- 每个新 shell 先执行：`export PATH="/opt/homebrew/opt/rustup/bin:$PATH"`（否则 `cargo: command not found`）。
- 验证门禁（不得回退）：`cargo fmt --all --check` 干净；`cargo clippy --workspace --all-targets --features embedded-database-core/std -- -D warnings` 干净；`cargo test --workspace --features embedded-database-core/std` = **301 passed**（core 216 + kv 8 + os 5 + sql 72）。
- 提交恰好 3 个，消息原样使用：① `chore: 基线快照（源码、配置与文档）` ② `chore: 基线快照（vendor 依赖与 Cargo.lock）` ③ `docs: 账本核销与 README 状态修正`。
- 不修改 `vendor/` 内容；不使用 `git add -f` 强行添加被忽略文件（除 Task 1 规则调整后新增可见的账本文件）。
- 账本历史条目**只加注记、不删不改**；幻影功能**不现场重做**。
- 分支 `feat/m1-core-skeleton` 当前零提交；Task 2 的提交是该仓库的第一个 commit。
- 核销判定标准：账本声称的代码/文档改动在工作树中不存在 = **幻影**；无法复核的历史数字 = **存疑**（标注，不判幻影）。

**引用事实（已由控制者核验）**：尾部（M29–M38）36 条 `[x]`，其中 16 条幻影（第一套 M36×5 + M37×9 + M38×2）；M1–M28 共 154 条 `[x]`。当前 `crates/embedded-database-sql/src/ast.rs` 的 `Statement` 仅 `Select/Insert/CreateTable/DropTable`；sql 实测测试 72，workspace 实测 301。

---

### Task 1: 前置准备（跟踪规则 + 残留清理）

**Files:**
- Modify: `.superpowers/sdd/.gitignore`
- Delete: `.cargo/config.toml.moved-by-offline-build`

**Interfaces:**
- Consumes: 无
- Produces: 可被 `git add` 的 `.superpowers/sdd/.gitignore`、`progress.md`、`progress-m28.md`（Task 2 依赖此规则）

- [ ] **Step 1: 确认残留文件与 config.toml 完全相同**

Run:
```bash
diff .cargo/config.toml .cargo/config.toml.moved-by-offline-build && echo IDENTICAL
```
Expected: 无 diff 输出，打印 `IDENTICAL`（exit 0）。若打印差异内容——**停止**，不要删除，向用户报告。

- [ ] **Step 2: 删除残留备份**

```bash
rm .cargo/config.toml.moved-by-offline-build
ls .cargo/
```
Expected: `ls` 只列出 `config.toml`。

- [ ] **Step 3: 改写 sdd 忽略规则（只放行账本）**

将 `.superpowers/sdd/.gitignore` 全文替换为：
```
*
!.gitignore
!progress.md
!progress-m28.md
```

- [ ] **Step 4: 验证忽略/放行生效**

Run:
```bash
git check-ignore -v .superpowers/sdd/m37-0-report.md; echo "brief_rc=$?"
git check-ignore -q .superpowers/sdd/progress.md; echo "progress_rc=$?"
git status --short --untracked-files=all | rg superpowers
```
Expected:
- `brief_rc=0` 且前面打印 `.superpowers/sdd/.gitignore:1:*` 命中行（brief/report 仍被忽略）；
- `progress_rc=1`（progress.md 未被忽略）；
- `git status` 中 superpowers 相关只出现 `?? .superpowers/sdd/.gitignore`、`?? .superpowers/sdd/progress.md`、`?? .superpowers/sdd/progress-m28.md`（brief/report 不出现）。

- [ ] **Step 5: 验证离线构建未受影响**

Run:
```bash
cargo build --offline 2>&1 | tail -2
```
Expected: `Finished \`dev\` profile ...`（exit 0）。本步无提交（提交在 Task 2）。

---

### Task 2: 基线提交①（源码、配置与文档）

**Files:**
- Create: 提交（staged 内容即 crates/、docs/、.github/、scripts/、supply-chain/、sbom.json、crates/\*/\*.cdx.json、根配置、README 原样、未核销状态的 progress\*.md、本 spec 文档、本 plan 文档）
- 不含：`vendor/`、`Cargo.lock`（留给 Task 3）

**Interfaces:**
- Consumes: Task 1 的忽略规则
- Produces: commit ①（Task 3 在其上继续）

- [ ] **Step 1: 精确暂存（显式 pathspec，禁用 `git add -A`）**

```bash
git add .cargo .github .gitignore Cargo.toml README.md crates docs rust-toolchain.toml sbom.json scripts supply-chain \
  .superpowers/sdd/.gitignore .superpowers/sdd/progress.md .superpowers/sdd/progress-m28.md
```

- [ ] **Step 2: 校验暂存内容**

Run:
```bash
git diff --cached --name-only | wc -l
git diff --cached --name-only | rg '^vendor/|^Cargo.lock$' ; echo "excluded_rc=$?"
git diff --cached --name-only | rg 'moved-by-offline-build'; echo "moved_rc=$?"
git ls-files --cached | rg '\.superpowers/sdd/'
git ls-files --cached | rg -c '^crates/'
```
Expected:
- 文件数在 60–120 之间；
- `excluded_rc=1`（无 vendor/、无 Cargo.lock 被暂存）；
- `moved_rc=1`（残留文件不存在于暂存区）；
- sdd 恰好 3 行：`.superpowers/sdd/.gitignore`、`.superpowers/sdd/progress-m28.md`、`.superpowers/sdd/progress.md`；
- `crates/` 下文件数 ≥ 40（含 7 个 Cargo.toml、src、cdx.json）。

- [ ] **Step 3: 确认账本暂存的是未核销状态**

Run:
```bash
git show :.superpowers/sdd/progress.md | rg -c '核销 2026-10-18'; echo "annot_rc=$?"
```
Expected: `annot_rc=1`（0 命中——基线里还没有核销注记）。

- [ ] **Step 4: 提交**

```bash
git commit -m "chore: 基线快照（源码、配置与文档）"
```

- [ ] **Step 5: 验证**

Run:
```bash
git log --oneline; git status --short --untracked-files=all | rg -v 'schemas\|vendor/'
```
Expected: `git log` 有 1 条提交；未跟踪残留仅 `vendor/`（及 Cargo.lock，见 Task 3）。若出现其他未忽略文件——补进本次提交或说明原因。

---

### Task 3: 基线提交②（vendor 依赖与 Cargo.lock）

**Files:**
- 提交 `vendor/`（约 2,900 文件）+ `Cargo.lock`

**Interfaces:**
- Consumes: commit ①
- Produces: commit ②（Task 8 的提交③在其上）

- [ ] **Step 1: 暂存**

```bash
git add vendor Cargo.lock
```

- [ ] **Step 2: 校验暂存范围**

Run:
```bash
git diff --cached --name-only | wc -l
git diff --cached --name-only | rg -v '^(vendor/|Cargo.lock$)' | wc -l
```
Expected: 文件数 ≥ 2800；第二行为 `0`（除 vendor/ 与 Cargo.lock 外无其他文件）。

- [ ] **Step 3: 提交**

```bash
git commit -m "chore: 基线快照（vendor 依赖与 Cargo.lock）"
git status --short --untracked-files=all
```
Expected: 提交成功，`git status` 无任何输出（工作树干净，仅剩 ignored）。

---

### Task 4: 尾部深核 M29–M38，写入判定与幻影注记

**Files:**
- Modify: `.superpowers/sdd/progress.md`（仅在其下追加注记行，插入点见 Step 4）

**Interfaces:**
- Consumes: 事实：尾部 36 条 `[x]`、16 条幻影、sql 测试 72、workspace 301
- Produces: 16 条「⚠️ 核销 2026-10-18」注记行 + 核实清单（Task 6 汇总依赖这些数字）

- [ ] **Step 1: 建立基线证据（三条命令，输出存档到本任务报告）**

Run:
```bash
export PATH="/opt/homebrew/opt/rustup/bin:$PATH"
cargo test -p embedded-database-sql 2>&1 | rg 'test result: ok.*passed' | head -1
rg -c '#\[test\]' crates/embedded-database-core/src/*.rs crates/embedded-database-kv/src/*.rs crates/embedded-database-os/src/*.rs crates/embedded-database-sql/src/*.rs
stat -f "%Sm %N" crates/embedded-database-sql/src/ast.rs crates/embedded-database-sql/src/parser.rs crates/embedded-database-sql/src/executor.rs \
  .superpowers/sdd/m37-7-report.md .superpowers/sdd/m36-3-report.md .superpowers/sdd/m36-0-report.md .superpowers/sdd/m36-1-report.md
```
Expected: sql `72 passed`；各文件 `#[test]` 数之和 = 301；sql 源文件时间戳为 `Oct 5 15:25–15:45`，M37 报告为 `11:02–11:45`，`m36-3-report` 为 `10:18`（第一套），`m36-0/1-report` 为 `15:37/15:45`（第二套，已覆盖）。

- [ ] **Step 2: 核验第一套 M36 与 M37 声称的代码不存在**

Run:
```bash
rg -n 'Statement::Update|Statement::Delete|parse_where_expr|Expr::Between|UnionStmt|CaseWhen|Expr::Cast|Expr::Exists|AlterTableStmt|CreateIndexStmt|CteDef|JoinType' crates/embedded-database-sql/src/; echo "phantom_rc=$?"
rg -n 'where_between|where_like|where_in|where_is_null|union_|case_when' crates/embedded-database-sql/src/executor.rs; echo "tests_rc=$?"
rg -n '"BETWEEN"|"IN"|"IS"' crates/embedded-database-sql/src/lexer.rs; echo "kw_rc=$?"
```
Expected: 三条命令全部 `rc=1`（0 命中）。`Keyword::Offset` 存在于 `token.rs` 但 parser 不消费，`Expr::IsNull(Box<Expr>, bool)` 仍为元组变体（非 M36-2 报告声称的结构体变体）——这两点写入注记。

- [ ] **Step 3: 核验第二套 M36 与更早尾部条目为真**

Run:
```bash
rg -c 'Boolean|Blob' crates/embedded-database-sql/src/result.rs
rg -c 'coalesce' crates/embedded-database-sql/src/function.rs
rg -n 'resume' crates/embedded-database-core/src/writer.rs | head -3
rg -n 'compact_with_budget' crates/embedded-database-core/src/db.rs | head -3
rg -n 'put_batch' crates/embedded-database-kv/src/lib.rs | head -3
rg -n '跨平台' docs/superpowers/specs/2026-10-02-embedded-database-operations-manual.md
```
Expected: 全部有命中（>0）：Boolean/Blob 存在、coalesce 存在、writer `resume` 存在、`compact_with_budget` 存在、`put_batch` 存在、操作手册含「跨平台」。据此判定 M29-0/M29-1-*、M30-0、M30-1、M32-*、M34、M35、第二套 M36-* 为**已核实**（无需注记）。

- [ ] **Step 4: 追加 16 条幻影注记（严格按左列条目行位置插入其下）**

在 `.superpowers/sdd/progress.md` 中，对下列每个条目的行（行号为基线提交①时的位置，条目文本未变，可用 `rg -n` 重新定位），在其**紧下方**追加对应注记行（两个空格 + `- ⚠️ …`）：

|- 条目行 | 追加注记 |
|---|---|
| `- [x] **M36-0: UPDATE 语句 — 完成**` | `  - ⚠️ 核销 2026-10-18：代码未在工作树中核实——ast.rs 无 Statement::Update，parser 不消费 Keyword::Update，无 update 测试；m36-0-brief/report 已被第二套 M36 同名覆盖（15:37 写就）。疑似被第二套 M36 重写覆盖；重做与否待后续决策。` |
| `- [x] **M36-1: DELETE 语句 — 完成**` | `  - ⚠️ 核销 2026-10-18：代码未在工作树中核实——无 Statement::Delete、无墓碑软删除；m36-1-report 已被第二套 M36 同名覆盖（15:45 写就）。` |
| `- [x] **M36-2: WHERE 运算符增强 — 完成**` | `  - ⚠️ 核销 2026-10-18：代码未在工作树中核实——parser 无 parse_where_expr、无 Expr::Between、lexer 无 BETWEEN/IN/IS 关键字、executor 无 where_between/where_like/where_in/where_is_null 测试；Expr::IsNull 仍为元组变体（非报告声称的结构体变体）。` |
| `- [x] **M36-3: LEFT JOIN — 完成**` | `  - ⚠️ 核销 2026-10-18：代码未在工作树中核实——ast/parser 无 JoinType/LEFT JOIN，JoinClause 无连接类型字段，仅 INNER JOIN。` |
| `- [x] **M36 全部完成**：SQL 功能补齐 P1–P3（UPDATE/DELETE/WHERE 增强/LEFT JOIN）。` | `  - ⚠️ 核销 2026-10-18：本条汇总的 5 条子项（含本条共 5 条）全部未在工作树中核实，均为幻影。` |
| `- [x] **M37-0: UNION / UNION ALL — 完成**` | `  - ⚠️ 核销 2026-10-18：代码未在工作树中核实——ast.rs 无 Statement::Union/UnionStmt，parser 无 UNION 分支，无 union 测试（账本声称 sql 77，实测 72）。` |
| `- [x] **M37-1: CASE WHEN — 完成**` | `  - ⚠️ 核销 2026-10-18：代码未在工作树中核实——无 Expr::CaseWhen。` |
| `- [x] **M37-2: CAST — 完成**` | `  - ⚠️ 核销 2026-10-18：代码未在工作树中核实——无 Expr::Cast。` |
| `- [x] **M37-3: OFFSET — 完成**` | `  - ⚠️ 核销 2026-10-18：代码未在工作树中核实——token.rs 有 Keyword::Offset 但 parser 不消费，SelectStmt 无 offset 字段。` |
| `- [x] **M37-4: EXISTS 子查询 — 完成**` | `  - ⚠️ 核销 2026-10-18：代码未在工作树中核实——无 Expr::Exists。` |
| `- [x] **M37-5: ALTER TABLE — 完成**` | `  - ⚠️ 核销 2026-10-18：代码未在工作树中核实——无 AlterTableStmt。` |
| `- [x] **M37-6: CREATE INDEX / DROP INDEX — 完成**` | `  - ⚠️ 核销 2026-10-18：代码未在工作树中核实——无 CreateIndexStmt/DropIndexStmt。` |
| `- [x] **M37-7: WITH (CTE) — 完成**` | `  - ⚠️ 核销 2026-10-18：代码未在工作树中核实——无 CteDef，SelectStmt 无 with 字段。` |
| `- [x] **M37 全部完成**：SQL 功能补齐 P4–P8（UNION/CASE WHEN/CAST/OFFSET/EXISTS/ALTER TABLE/CREATE INDEX/DROP INDEX/WITH）。` | `  - ⚠️ 核销 2026-10-18：本条汇总的 9 条子项全部未在工作树中核实；sql 实测 72 ≠ 账本声称 89，workspace 实测 301 ≠ 310；证据：M37 报告写于 10-05 11:02–11:45，而 embedded-database-sql/src/*.rs 于 15:25–15:45 被重写。` |
| `- [x] **M38: 操作手册更新 — 完成**` | `  - ⚠️ 核销 2026-10-18：未核实——操作手册中无 UPDATE/DELETE/UNION/CASE WHEN/ALTER TABLE 字样，§3.2 未更新到 M37。` |
| `- [x] **M38 全部完成**：操作手册同步 M36/M37 SQL 功能。` | `  - ⚠️ 核销 2026-10-18：未核实——操作手册无 M36/M37 SQL 功能同步痕迹。` |

- [ ] **Step 5: 验证注记数量与位置**

Run:
```bash
rg -c '⚠️ 核销 2026-10-18' .superpowers/sdd/progress.md
rg -n '⚠️ 核销 2026-10-18' .superpowers/sdd/progress.md | head -3
```
Expected: 计数 **16**。同时用 `rg -n` 逐条目视确认：16 条注记各自紧跟在对应幻影条目行的正下方（无错位、无遗漏）。Task 6 的汇总 section 措辞已刻意避开该字符串，故此后该计数恒为 16。

---

### Task 5: 全文快检 M1–M28

**Files:**
- Modify: `.superpowers/sdd/progress.md`（仅当发现新幻影/存疑时，按 Task 4 格式追加注记）

**Interfaces:**
- Consumes: 154 条 M1–M28 条目
- Produces: 快检结论（Task 6 汇总引用）

- [ ] **Step 1: 里程碑数字一致性核对**

Run:
```bash
rg -n 'M[0-9]+ \|' docs/superpowers/specs/2026-10-02-embedded-database-project-summary.md
rg -n 'workspace [0-9]+ passed|workspace [0-9]+ 测试' .superpowers/sdd/progress.md | head -30
```
Expected: 里程碑表 M27/M28=288、M30=291、M32=294、M36=301 与账本尾部一致。已知差异：账本 M29-1-2 写「workspace 291」而里程碑表 M29 行写 288——M29-1 在 M29 行之后执行、其后 M30 行即 291，属**有合理解释**，记入快检结论的"说明"而非存疑。

- [ ] **Step 2: 关键特性存在性抽查（逐条执行）**

Run:
```bash
rg -c '黄金|golden' crates/embedded-database-core/src/crc.rs
rg -n 'compile_error!' crates/embedded-database-core/src/segment.rs
rg -c 'CrashPoint' crates/embedded-database-bench/src/bin/inject_crash.rs
rg -n 'sm3|sm4|hmac' crates/embedded-database-core/src/lib.rs
rg -n 'pub fn flush' crates/embedded-database-core/src/db.rs
rg -n 'fn resume' crates/embedded-database-core/src/writer.rs
rg -c 'GROUP BY|GROUP|having' crates/embedded-database-sql/src/parser.rs
rg -n 'JOIN' crates/embedded-database-sql/src/parser.rs | head -3
rg -n 'ORDER BY|ORDER' crates/embedded-database-sql/src/parser.rs | head -3
rg -n '\.exit' crates/embedded-database-cli/src/main.rs
test -f .superpowers/sdd/progress-m28.md && echo M28_OK
test -f docs/superpowers/specs/2026-10-02-embedded-database-delivery-checklist.md && echo CHECKLIST_OK
test -f supply-chain/config.toml && test -f supply-chain/audits.toml && echo VET_FILES_OK
```
Expected: 全部有命中/打印 OK（对应 M1 CRC、M1 大端防护、M1 崩溃注入、M6/M8 国密、M7 flush、M12 resume、M24–M26 聚合/GROUP/HAVING、M23 JOIN、M21 ORDER BY、M20 CLI、M28 终审、M13/M15 交付文档、M14 供应链文件）。

- [ ] **Step 3: 处理已知存疑项——M14「cargo-vet 审计」**

Run:
```bash
wc -l supply-chain/audits.toml; rg -c 'exemptions' supply-chain/config.toml
```
Expected: `audits.toml` 为 4 行（无真实审计条目）；`[[exemptions.` 条目数 = 38。判定：M14 声称的「cargo-vet 审计」实际是**豁免配置**，无真实审计条目——记为**存疑**，在 Task 6 汇总中说明"真实审计归 Spec 2，本条不作幻影处理"。

- [ ] **Step 4: 若有新幻影则追加注记**

对快检中确认"声称的代码/文档不存在"的条目，按 Task 4 Step 4 的格式追加注记，并在任务报告中列出。若无非幻影发现，明确记录"快检未发现 M1–M28 侧新幻影"。

---

### Task 6: 核销汇总 section + 账本头部约定更新

**Files:**
- Modify: `.superpowers/sdd/progress.md`（文末追加 section；头部 `约定` 行更新）

**Interfaces:**
- Consumes: Task 4/5 的判定结果（16 幻影、20 已核实、154 条快检、M14 存疑、M30-0 重复条目）
- Produces: 核销 section（Task 8 验收对象）

- [ ] **Step 1: 更新头部约定行**

将第 8 行的：
```
- 约定：**不执行任何 git 提交**（用户决定）。各 Task 的「检查点」步骤只确认文件存在。
```
替换为：
```
- 约定：**不执行任何 git 提交**（用户决定）——**2026-10-18 已废止**，见文末「2026-10-18 核销」：
  现改为"每逻辑单元完成即提交"。各 Task 的「检查点」步骤只确认文件存在。
```

- [ ] **Step 2: 文末追加核销 section**

在文件末尾追加：

```markdown

## 2026-10-18 核销

- 方法：尾部（M29–M38，36 条 `[x]`）逐条深核 + 全文（M1–M28，154 条 `[x]`）廉价不变量快检。
- 判定汇总：**已核实 20 条 / 幻影 16 条 / 存疑 1 条**（M14 的「cargo-vet 审计」）。
- 幻影条目（16）：第一套 M36-0/1/2/3 + 「M36 全部完成」（5）；M37-0…M37-7 + 「M37 全部完成」（9）；
  M38 + 「M38 全部完成」（2）。逐条证据见各条目下方的核销注记。
- 证据链：`crates/embedded-database-sql/src/*.rs` 于 10-05 15:25–15:45 被重写，晚于 M37 报告（11:02–11:45）
  与第一套 M36 报告（10:15–10:18）；第二套 M36 的报告（15:37/15:45）与当前代码吻合。
  第一套 M36 的 `m36-0/1-brief/report` 被第二套同名**覆盖**销毁，不可复原。
- 编号冲突：账本存在**两套 M36 编号**——第一套（UPDATE/DELETE/WHERE 增强/LEFT JOIN，幻影）与
  第二套（Boolean/Blob、6 个标量函数、跨平台文档、收尾，已核实）。历史条目不重命名、不删除。
- 账本数据质量：`M30-0: BTreeMap key_map 重构` 条目重复出现两次（内容相同）；不删改。
- 全文快检结论：M1–M28 未发现新幻影；里程碑测试数表与账本尾部一致；
  M29 行 288 vs 账本 M29-1-2 的 291 属时序差异（M29-1 在 M29 行之后执行），非矛盾。
- 存疑：M14 声称「cargo-vet 审计」，实际 `supply-chain/audits.toml` 无真实审计条目、38 条依赖
  全靠 exemptions 放行；真实供应链审计为独立后续任务（Spec 2）。
- 遗留待办：第一套 M36 + M37 + M38 是否重做——待后续决策（不在本 spec 范围）。
- 提交策略：自 2026-10-18 起改为"每逻辑单元完成即提交"，取代此前的零提交约定。
```

- [ ] **Step 3: 验证 section 与计数**

Run:
```bash
rg -n '^## 2026-10-18 核销' .superpowers/sdd/progress.md
rg -c '核销 2026-10-18' .superpowers/sdd/progress.md
tail -3 .superpowers/sdd/progress.md
```
Expected: 标题命中 1 处；`核销 2026-10-18` 计数 ≥ 16；`tail` 显示 section 末行「提交策略：…」。

---

### Task 7: README 修正

**Files:**
- Modify: `README.md`（状态行 + cargo-vet 小节）

**Interfaces:**
- Consumes: 核销结论（301 测试、M36 已核实、M37/M38 幻影）
- Produces: README（Task 8 验收对象）

- [ ] **Step 1: 修正状态行**

将 README 第 7 行：
```
- 状态：M1–M28 全部完成，288 测试全绿（core 215 + kv 6 + os 5 + sql 62），0 提交
```
替换为：
```
- 状态：M1–M36 已核实完成，301 测试全绿（core 216 + kv 8 + os 5 + sql 72）；
  M37/M38 账本条目已核销为未实现（详见 `.superpowers/sdd/progress.md`）
```

- [ ] **Step 2: 修正 cargo-vet 小节**

将 README 第 82–91 行整段：
```
### cargo-vet 审计

```bash
# 需网络；若 .cargo/config.toml 已指向 vendored sources，先移开再安装
cargo install cargo-vet
cargo vet
```

首次运行会生成 `supply-chain/` 目录，包含每个依赖的来源与内容摘要。
CI 中可配置 `cargo vet` 作为门禁。
```
替换为：
```
### cargo-vet 审计

当前 `supply-chain/audits.toml` 无真实审计条目，全部依赖靠 `supply-chain/config.toml`
的 38 条 exemptions 放行；真实供应链审计为独立的后续任务。CI 中未配置 `cargo vet` 门禁。

```bash
# 需网络；若 .cargo/config.toml 已指向 vendored sources，先移开再安装
cargo install cargo-vet
cargo vet
```
```

- [ ] **Step 3: 验证 README 数字与实测一致**

Run:
```bash
rg -n 'M1–M36|301 测试' README.md
rg -c 'CI 中可配置 `cargo vet` 作为门禁' README.md; echo "old_rc=$?"
rg -c '#\[test\]' crates/*/src/*.rs | awk -F: '{s+=$2} END {print "tests="s}'
```
Expected: 第一行命中新状态行；`old_rc=1`（旧失实句已删）；`tests=301`。

---

### Task 8: 提交③ + 终验

**Files:**
- 提交 `.superpowers/sdd/progress.md`、`README.md`、`docs/superpowers/specs/2026-10-18-ledger-baseline-readme-design.md`、`docs/superpowers/plans/2026-10-18-ledger-baseline-readme.md` 的修改

**Interfaces:**
- Consumes: Task 2/3 的 commit ①②、Task 4–7 的文件修改
- Produces: commit ③ + 终验证据（本 spec 验收标准）

- [ ] **Step 1: 核对改动范围**

Run:
```bash
git status --short
git diff --stat
```
Expected: 仅 `.superpowers/sdd/progress.md`、`README.md` 与 spec/plan 两份设计文档被修改（spec/plan 的 exemptions 计数修正为 38，见用户 2026-10-18 决定；无其他改动、无新增未跟踪文件）。

- [ ] **Step 2: 运行完整验证门禁**

Run:
```bash
export PATH="/opt/homebrew/opt/rustup/bin:$PATH"
cargo fmt --all --check && echo FMT_OK
cargo clippy --workspace --all-targets --features embedded-database-core/std -- -D warnings && echo CLIPPY_OK
cargo test --workspace --features embedded-database-core/std 2>&1 | rg 'test result: ok'
cargo build --offline 2>&1 | tail -1
```
Expected: `FMT_OK`、`CLIPPY_OK`；测试行含 `216 passed`、`8 passed`、`5 passed`、`72 passed`（合计 301，全部 `0 failed`）；离线构建 `Finished`。

- [ ] **Step 3: 提交**

```bash
git add .superpowers/sdd/progress.md README.md docs/superpowers/specs/2026-10-18-ledger-baseline-readme-design.md docs/superpowers/plans/2026-10-18-ledger-baseline-readme.md
git commit -m "docs: 账本核销与 README 状态修正"
```

- [ ] **Step 4: 终验**

Run:
```bash
git log --oneline
git status --short --untracked-files=all
git ls-files .superpowers/sdd/
rg -n '^## 2026-10-18 核销' .superpowers/sdd/progress.md
rg -c '⚠️ 核销 2026-10-18' .superpowers/sdd/progress.md
```
Expected:
- `git log --oneline` 恰 3 条，顺序为 ①→②→③；
- `git status` 无输出；
- `git ls-files` 列出 `.superpowers/sdd/.gitignore`、`progress-m28.md`、`progress.md`；
- 核销 section 存在；
- 注记计数 = 16（此时汇总 section 用「- 幻影条目（16）」不复用「⚠️ 核销 2026-10-18」串，故为 16）。

---

## Self-Review

- **Spec 覆盖**：§2 范围内 4 项 → Task 1（残留清理）、Task 4/5/6（核销）、Task 2/3/8（git 基线）、Task 7（README）；§4 注记格式与 §5 提交序列、§8 验收 6 条逐条落到 Task 4–8 的验证步骤。
- **占位符扫描**：无 TBD/TODO；所有 code/命令块为可直接执行内容；注记文本 16 条全量给出。
- **类型/名称一致性**：注记串统一 `⚠️ 核销 2026-10-18`；提交消息三处一致；测试数 216+8+5+72=301 全篇一致；文件路径 `.superpowers/sdd/progress.md`、`docs/superpowers/specs/2026-10-02-embedded-database-operations-manual.md` 与仓库实际一致。
- **已知风险**：Task 4 Step 5 的计数断言在 Task 6 之后失效，已就地说明改用法（Task 8 Step 4 固定为 16）。
