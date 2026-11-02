# Spec 2：真实供应链审计 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让 `cargo vet check` 与 `cargo deny check` 成为可复现的硬事实——deploy 图 7 个第三方依赖全部有据可查，文档表述与 `supply-chain/` 实际内容一致。

**Architecture:** 双工具分工——cargo-vet 负责"信任来源"（本地实质性审计 + 上游 import + 带据豁免），cargo-deny 负责"风险门禁"（advisories / licenses / bans / sources）。审计配置入库（`supply-chain/`、`deny.toml`），入口统一为 `scripts/supply-chain-audit.sh` 与 CI 的 `supply-chain` job。

**Tech Stack:** cargo-vet 0.10.2、cargo-deny 0.20.2（均已安装于 `~/.cargo/bin`）、bash、GitHub Actions、vendored sources（`.cargo/config.toml` → `vendor/`）

## Global Constraints

- **零源码改动**：本 spec 不改任何 `crates/**/*.rs`；`cargo fmt --all --check` 干净、`cargo clippy --workspace --all-targets --features embedded-database-core/std -- -D warnings` 干净、`cargo test --workspace --features embedded-database-core/std` 保持 **301 passed**。
- **环境**：每个 shell 先 `export PATH="/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH"`（否则 `cargo`/`cargo-vet`/`cargo-deny` 找不到）。
- **可复现命令**：`cargo vet check` 在 vendored 模式离线即可跑通（实测）；**不要**使用 `cargo vet --offline`（非法参数）、`cargo vet check --locked/--frozen/--no-minimize-exemptions`（`check` 不接受）。`cargo deny --offline check licenses bans sources` 可离线；`cargo deny check advisories` **必须网络**。
- **deploy 图第三方 = 7 个**：`zerocopy 0.8.59`、`zerocopy-derive 0.8.59`、`bitflags 2.13.2`、`proc-macro2 1.0.107`、`quote 1.0.47`、`syn 2.0.119`、`unicode-ident 1.0.26`。**这 7 个不允许无声豁免**（必须有 audit、import 或带 `notes`（含复审日期）的豁免解释）。
- **criteria**：deploy crate = `safe-to-deploy`；dev-only crate = `safe-to-run`。
- **豁免格式**：每条 `[[exemptions.*]]` 必须含 `notes`（非空、说明原因，且文本内含复审日期 `2027-04-30`）。**不得**使用 `expires` 字段——cargo-vet 0.10.2 不支持，写入后会被下一次 `cargo vet check` 重写配置时静默剥离（实测计数 1 → 0）。
- **审计声明**：`who = "embedded-database maintainers"`；每条本地审计的 `notes` 必含取证结论 + 深度声明（结构性取证 + 关键路径抽样，非逐行）。
- **许可证 allowlist**：`MIT`、`Apache-2.0`、`BSD-2-Clause`、`Unicode-3.0`；`[advisories] ignore` 为空。
- **CI**：不用第三方 action（审计工具用 `cargo install --locked`）；trigger 改为所有分支 push + pull_request；仓库无 remote，CI 无法本地真跑，验收以 YAML 静态核对为准。
- **提交**：每逻辑单元一提交，消息用下文各 Task 指定值；提交前 `git status --short` 除预期文件外无输出。
- **不做**：cargo-vet `trust` 机制；dev 树人工审计；SBOM 覆盖面修正（另立任务）。

---

### Task 1: `deny.toml` + cargo-deny 四类检查

**Files:**
- Create: `deny.toml`
- Test: 无（验证即 cargo-deny 自身输出）

**Interfaces:**
- Consumes: `Cargo.lock`（45 包）、`Cargo.toml`（members + `license = "Apache-2.0"`）
- Produces: `deny.toml`（Task 4 的脚本与 Task 5 的 CI job 都直接调用它）

- [ ] **Step 1: 写入 `deny.toml`（完整内容）**

```toml
# cargo-deny 配置 — Spec 2「真实供应链审计」
#
# 校验（vendored 模式）：
#   cargo deny --offline check licenses bans sources   # 可离线
#   cargo deny check                                   # 四类，advisories 需网络（首次克隆 RustSec advisory-db）
#
# 决策依据：docs/superpowers/specs/2026-10-18-supply-chain-audit-design.md

[advisories]
# 不静默放行任何公告；如遇误报必须连同处置理由一起写入本文件并在提交信息中说明
ignore = []

[licenses]
allow = [
    "MIT",
    "Apache-2.0",
    "BSD-2-Clause",
    "Unicode-3.0",
]
confidence-threshold = 0.8

[licenses.private]
ignore = false

[bans]
multiple-versions = "deny"
wildcards = "deny"

[sources]
unknown-registry = "deny"
unknown-git = "deny"
allow-registry = ["https://github.com/rust-lang/crates.io-index"]
allow-git = []
```

- [ ] **Step 2: 离线三类检查**

Run:
```bash
export PATH="/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH"
cargo deny --offline check licenses
cargo deny --offline check bans
cargo deny --offline check sources
```
Expected: 三条命令均以 `licenses ok` / `bans ok` / `sources ok` 结尾，退出码 0。（若无配置时 licenses 会 FAILED；本步验证 allowlist 覆盖了 `vendor/*/Cargo.toml` 中实际出现的许可证表达式——`OR` 组合中任一允许项即视为合规；提及 `LLVM-exception` 的变体仅出现在 `OR` 组合内，其另一项（Apache-2.0 / MIT）已在 allowlist，故无需独立条目（单独列出会反复触发 `warning[license-not-encountered]`，实测）。若某 crate 报无法解析 SPDX，例如 `Apache-2.0 / MIT` 斜杠写法，则按其 `LICENSE*` 文件内容在 `[licenses.clarify]` 里更正为等价标准表达式，并在报告中记录 crate 与表达式。）

- [ ] **Step 3: 联网第四类检查**

Run:
```bash
cargo deny check advisories
```
Expected: 以 `advisories ok` 结尾，退出码 0。
**若报告真实公告**：逐条评估——能升级依赖则升级（但本 spec 要求零源码改动，因此优先判断是否影响 deploy 图）；不能升级/不适用时加 `ignore = [{ id = "RUSTSEC-XXXX-XXXX", reason = "<具体理由>" }]`，并在 Task 报告与提交信息中列出该公告与理由。**不得**默认放行。

- [ ] **Step 4: 四类一次性检查**

Run:
```bash
cargo deny check
```
Expected: 四类全 `ok`，退出码 0。

- [ ] **Step 5: 提交**

```bash
git add deny.toml
git commit -m "chore: 引入 cargo-deny 门禁配置（advisories/licenses/bans/sources）"
```

---

### Task 2: cargo-vet `[policy]` + 三条本地实质性审计

**Files:**
- Modify: `supply-chain/config.toml`（新增 `[policy.*]` 七项）
- Modify: `supply-chain/audits.toml`（新增 3 条 `[[audits.*]]`）

**Interfaces:**
- Consumes: Task 1 的 `deny.toml`（本任务不直接用，但共享"审计即事实"约定）
- Produces: `[policy.embedded-database-*]`（Task 3 的 `cargo vet check` 依赖它判定 criteria）；`audits.toml` 的 3 条本地审计（Task 3 的 `cargo vet check` 从此不再需要这 3 条的豁免）

- [ ] **Step 1: 取证 `zerocopy`（按 Spec §3.3 八项清单）**

Run:
```bash
cd /Users/caosunliu/opencode/EmbeddedDataBase
test -f vendor/zerocopy/build.rs && echo BUILD_RS_YES
rg -o 'unsafe' vendor/zerocopy/src | wc -l
rg -l 'unsafe' vendor/zerocopy/src | wc -l
rg -n 'std::net|std::process|std::env|Command::new|env!|include_str!|include_bytes!' vendor/zerocopy/build.rs vendor/zerocopy/src | head -20
rg -n '^#!\[no_std\]|no_std' vendor/zerocopy/src/lib.rs | head -3
rg -N '^license|^rust-version' vendor/zerocopy/Cargo.toml
rg -n '^name = "zerocopy"' -A1 Cargo.lock
```
Expected（实测基线）：`build.rs` 存在；`unsafe` 出现 **792** 次、分布在 **19** 个文件；`build.rs` 只做 lint/cfg 与特性探测（无网络/进程/文件系统访问，逐条记录实际命中）；`src/lib.rs` 含 `#![no_std]`；`license = "BSD-2-Clause OR Apache-2.0 OR MIT"`；`Cargo.lock` 版本 `0.8.59` 与 `vendor/zerocopy/Cargo.toml` 版本一致。

- [ ] **Step 2: 取证 `zerocopy-derive`**

Run:
```bash
test -f vendor/zerocopy-derive/build.rs && echo BUILD_RS_YES || echo BUILD_RS_NO
rg -o 'unsafe' vendor/zerocopy-derive/src | wc -l
rg -l 'unsafe' vendor/zerocopy-derive/src | wc -l
rg -n 'proc-macro = true' vendor/zerocopy-derive/Cargo.toml
rg -N '^(syn|quote|proc-macro2|unicode-ident) *=' vendor/zerocopy-derive/Cargo.toml
rg -N '^license' vendor/zerocopy-derive/Cargo.toml
```
Expected（实测基线）：无 `build.rs`；`unsafe` **809** 次 / **44** 个文件（绝大多数出现在宏展开产生的代码与测试中——`notes` 里必须写明这一判断依据，例如 `rg -n 'unsafe' vendor/zerocopy-derive/src | rg -v 'tests?/' | wc -l` 的实际值）；`proc-macro = true`；依赖 `syn`/`quote`/`proc-macro2`/`unicode-ident`；许可证同上。

- [ ] **Step 3: 取证 `bitflags`**

Run:
```bash
test -f vendor/bitflags/build.rs && echo BUILD_RS_YES || echo BUILD_RS_NO
rg -o 'unsafe' vendor/bitflags/src | wc -l
rg -n 'unsafe' vendor/bitflags/src
rg -n 'std::net|std::process|std::env|Command::new|env!' vendor/bitflags/src | head
rg -n '^#!\[no_std\]|no_std' vendor/bitflags/src/lib.rs | head -3
rg -N '^license' vendor/bitflags/Cargo.toml
```
Expected（实测基线）：无 `build.rs`；`unsafe` **4** 次（`external.rs:2`、`internal.rs:1`、`lib.rs:1` 共 3 个文件）；无网络/进程/环境访问；`no_std` 兼容；`license = "MIT OR Apache-2.0"`。

- [ ] **Step 4: 写入 `[policy]`（完整内容，追加到 `supply-chain/config.toml` 末尾）**

```toml
[policy.embedded-database-bench]
criteria = "safe-to-deploy"

[policy.embedded-database-cli]
criteria = "safe-to-deploy"

[policy.embedded-database-core]
criteria = "safe-to-deploy"

[policy.embedded-database-embed]
criteria = "safe-to-deploy"

[policy.embedded-database-kv]
criteria = "safe-to-deploy"

[policy.embedded-database-os]
criteria = "safe-to-deploy"

[policy.embedded-database-sql]
criteria = "safe-to-deploy"
```

- [ ] **Step 5: 写入三条审计条目（`supply-chain/audits.toml`，替换其全部内容）**

将 Step 1–3 的真实取证结果填进 `notes`（下方为模板；`unsafe` 计数等数字**必须**用实际值替换，不得照抄除非一致）：

```toml
# cargo-vet audits file

[audits]

[[audits.bitflags]]
who = "embedded-database maintainers"
criteria = "safe-to-deploy"
version = "2.13.2"
notes = "本地实质性审计 2026-10-18（Spec 2 §3.3 八项清单）：无 build.rs；unsafe 4 处（external.rs 2、internal.rs 1、lib.rs 1），均为位运算与类型转换用途；无网络/进程/文件系统/环境变量访问；no_std 兼容；MIT OR Apache-2.0，LICENSE-APACHE 与 LICENSE-MIT 一致；Cargo.lock 与 vendor 版本均为 2.13.2。深度：结构性取证 + 关键路径抽样，未逐行。"
```

```toml
[[audits.zerocopy]]
who = "embedded-database maintainers"
criteria = "safe-to-deploy"
version = "0.8.59"
notes = "本地实质性审计 2026-10-18（Spec 2 §3.3 八项清单）：build.rs 仅做 lint/cfg 与特性探测，无网络/进程/文件系统访问；src 含 792 处 unsafe，分布 19 个文件，集中在 impls.rs/byte_slice.rs 等零拷贝类型转换路径；no_std 兼容；BSD-2-Clause OR Apache-2.0 OR MIT；Cargo.lock 与 vendor 版本均 0.8.59。深度声明：792 处 unsafe 未逐处审阅，为结构性取证 + 关键路径抽样。"
```

```toml
[[audits.zerocopy-derive]]
who = "embedded-database maintainers"
criteria = "safe-to-deploy"
version = "0.8.59"
notes = "本地实质性审计 2026-10-18（Spec 2 §3.3 八项清单）：proc-macro=true，无 build.rs；809 处 unsafe / 44 文件，绝大多数为宏展开模板与测试代码（取证命令与计数见 Task 2 报告）；仅依赖 syn/quote/proc-macro2/unicode-ident；BSD-2-Clause OR Apache-2.0 OR MIT；Cargo.lock 与 vendor 版本均 0.8.59。深度声明：宏展开代码未逐行审阅，为结构性取证 + 关键路径抽样。"
```

- [ ] **Step 6: 运行 cargo-vet（会自动清理这 3 条已不再需要的豁免）**

Run:
```bash
cargo vet check
```
Expected: 输出含 `Vetting Succeeded`，退出码 0；`supply-chain/config.toml` 中 `[[exemptions.zerocopy]]`、`[[exemptions.zerocopy-derive]]`、`[[exemptions.bitflags]]` 被自动移除（豁免数 38 → 35）。

- [ ] **Step 7: 验证信任来源已改为本地审计**

Run:
```bash
cargo vet explain-audit bitflags 2.13.2 safe-to-deploy
cargo vet explain-audit zerocopy 0.8.59 safe-to-deploy
cargo vet explain-audit zerocopy-derive 0.8.59 safe-to-deploy
rg -c '^\[\[exemptions\.' supply-chain/config.toml
git diff --stat supply-chain/
```
Expected: 前三条输出均**不再**出现 `exemption`（应为本地 audit 路径）；豁免计数为 `35`；`git diff --stat` 只涉及 `config.toml` 与 `audits.toml`。

- [ ] **Step 8: 提交**

```bash
git add supply-chain/config.toml supply-chain/audits.toml
git commit -m "chore: cargo-vet 本地审计 zerocopy/zerocopy-derive/bitflags + policy 表"
```

---

### Task 3: 上游 import（4 个 build 依赖）+ 降级收尾 + 豁免收窄

**Files:**
- Modify: `supply-chain/imports.lock`（import 结果）
- Modify: `supply-chain/config.toml`（豁免 `notes` 含复审日期；criteria 精确化）
- Modify: `supply-chain/audits.toml`（仅当降级分支需要新增条目）

**Interfaces:**
- Consumes: Task 2 的 `[policy]` 与 3 条本地审计
- Produces: 完整可复现的 `cargo vet check`（Task 4 脚本与 Task 5 CI 的依赖）

- [ ] **Step 1: 记录 import 前的建议清单**

Run:
```bash
cargo vet suggest 2>&1 | tee /tmp/vet-suggest-before.txt | head -12
```
Expected: 输出 `recommended audits for safe-to-deploy` 表，含 `unicode-ident`、`quote`、`proc-macro2`、`syn` 及 NOTE 行（`cargo vet import google would reduce this to …`）。**退出码为 101（表示仍需审计）属正常。**

- [ ] **Step 2: 执行四个上游 import**

Run:
```bash
cargo vet import google 2>&1 | tail -5
cargo vet import mozilla 2>&1 | tail -5
cargo vet import bytecode-alliance 2>&1 | tail -5
cargo vet import zcash 2>&1 | tail -5
```
Expected: 每条以写入 `supply-chain/imports.lock` 结束（输出行数会明显增加）。若某 peer 不可达（网络失败），记录该 peer 与错误原文，继续下一个——**降级规则见 Step 3**。

- [ ] **Step 3: 检查 4 个 build 依赖的覆盖情况（决策表）**

Run:
```bash
cargo vet check
cargo vet suggest 2>&1 | head -12
```
Expected（分支处理）：
- **check 通过（退出码 0）且 suggest 不再列出 `proc-macro2`/`quote`/`syn`/`unicode-ident`** → 4 个 crate 由 import 覆盖，**不写本地审计条目**，跳到 Step 4。
- **仍列出某个 crate** → 对该 crate 按 Task 2 Step 1 的取证方式做本地审计（`who = "embedded-database maintainers"`、`criteria = "safe-to-deploy"`、`notes` 含八项结论与深度声明），写入 `audits.toml`，再跑 `cargo vet check` 确认通过。
- **import 完全失败且取证也受限** → 保留其 `[[exemptions.<crate>]]`，但必须在 Step 4 的收窄中带上 `notes`（写明"import 不可得 + 取证受限"与复审日期）。

- [ ] **Step 4: 豁免收窄（`notes` 含复审日期 + criteria 精确化），用脚本一次性完成**

Run:
```bash
python3 - <<'PY'
import re, pathlib
p = pathlib.Path("supply-chain/config.toml")
s = p.read_text()
parts = re.split(r'(?=\[\[exemptions\.)', s)
NOTE = ("notes = \"dev-only（proptest 及其传递依赖，不进入交付物）；未做人工审计，"
        "保留豁免以放行构建与测试。复审日期 2027-04-30（cargo-vet 0.10.2 不支持豁免 "
        "expires 字段——该字段会被下一次重写静默剥离，故以本 notes 约定）。\"")
out = []
for part in parts:
    if part.startswith("[[exemptions."):
        name = re.match(r'\[\[exemptions\.([^\]]+)\]\]', part).group(1)
        if "safe-to-deploy" in part:
            part = part.replace('criteria = "safe-to-deploy"', 'criteria = "safe-to-run"')
        if "notes =" not in part:
            part = part.rstrip("\n") + "\n" + NOTE + "\n"
        out.append(part)
    else:
        out.append(part)
p.write_text("".join(out))
print("exemptions:", s.count("[[exemptions."))
PY
```
Expected: 打印豁免总数。**注意**：若某 deploy crate 落在 Step 3 的第三分支（带据豁免），把它的 `notes` 手工改为 import/取证受限的说明（脚本文本只适用于 dev-only）。**不得写入 `expires` 字段**（cargo-vet 0.10.2 会静默剥离）。

- [ ] **Step 5: 验证豁免完备性与 vet 通过**

Run:
```bash
rg -c '^\[\[exemptions\.' supply-chain/config.toml
rg -c '^notes = ' supply-chain/config.toml
rg -c '复审日期 2027-04-30' supply-chain/config.toml
cargo vet check
```
Expected: 三个计数**相等**；`cargo vet check` 输出含 `Vetting Succeeded`，退出码 0。

- [ ] **Step 6: 逐个确认 deploy 图 7 个 crate 的信任来源**

Run:
```bash
for spec in "zerocopy 0.8.59" "zerocopy-derive 0.8.59" "bitflags 2.13.2" "proc-macro2 1.0.107" "quote 1.0.47" "syn 2.0.119" "unicode-ident 1.0.26"; do
  set -- $spec
  echo "--- $1 $2"
  cargo vet explain-audit "$1" "$2" safe-to-deploy 2>&1 | tail -3
done
```
Expected: 7 条输出均给出明确的信任路径（`[local] audit` 或 imported audit）；**不允许 7 条中有任何一条只显示 `exemption`**——若有，说明 Step 3 的分支处理不完整，必须回到 Step 3。

- [ ] **Step 7: 提交**

```bash
git add supply-chain/
git commit -m "chore: cargo-vet 上游 import + 豁免收窄（notes/expires/criteria）"
```

---

### Task 4: `scripts/supply-chain-audit.sh`

**Files:**
- Create: `scripts/supply-chain-audit.sh`（`chmod +x`）

**Interfaces:**
- Consumes: `deny.toml`（Task 1）、`supply-chain/`（Task 2/3）
- Produces: 唯一审计入口（Task 5 的 CI job 与 README 都指向它）

- [ ] **Step 1: 写入脚本（完整内容）**

```bash
#!/usr/bin/env bash
# 供应链审计入口 — Spec 2
#   离线可跑：cargo vet check、cargo deny --offline check licenses/bans/sources
#   需网络  ：cargo deny check advisories（首次会克隆 RustSec advisory-db）
# 退出码：0=全部通过；2=advisories 未运行（无本地 RustSec 克隆，需网络）；其它非 0=cargo vet/deny 检查失败
set -euo pipefail

export PATH="/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH"

cd "$(dirname "$0")/.."

echo "== cargo-vet check（离线；信任来源：本地审计 + import + 带据豁免）=="
cargo vet check

echo
echo "== cargo-deny：licenses / bans / sources（离线）=="
cargo deny --offline check licenses bans sources

echo
echo "== cargo-deny：advisories（需网络）=="
# 注意：$CARGO_HOME/advisory-dbs 目录可能仅含 db.lock 而无实际克隆（实测），
# 因此必须检测真实克隆路径，否则离线时会以 cargo-deny 的不透明错误失败。
if [ -d "${CARGO_HOME:-$HOME/.cargo}/advisory-dbs/rustsec/advisory-db/.git" ]; then
  cargo deny check advisories
else
  echo "跳过：未发现本地 RustSec advisory 克隆（${CARGO_HOME:-$HOME/.cargo}/advisory-dbs/rustsec/advisory-db）。"
  echo "advisories 检查需网络，请联网后执行：cargo deny check advisories"
  exit 2
fi
```

- [ ] **Step 2: 赋权并运行**

Run:
```bash
chmod +x scripts/supply-chain-audit.sh
./scripts/supply-chain-audit.sh
```
Expected: cargo-vet 段输出 `Vetting Succeeded`；cargo-deny 段输出 `licenses ok`/`bans ok`/`sources ok`；最后一段：若本地已有 advisory-db 则输出 `advisories ok`，否则打印"跳过…（需网络）"并以退出码 2 结束（这是**预期**的两条路径之一，请在报告中写明实际走了哪条）。

- [ ] **Step 3: 验证脚本不改动任何被跟踪文件**

Run:
```bash
git status --short
```
Expected: 仅 `?? scripts/supply-chain-audit.sh`（尚未 add）；`supply-chain/` 无改动。

- [ ] **Step 4: 提交**

```bash
git add scripts/supply-chain-audit.sh
git commit -m "chore: 新增 supply-chain-audit.sh（vet + deny 一键审计）"
```

---

### Task 5: CI — trigger 修正 + `supply-chain` 硬门禁 job

**Files:**
- Modify: `.github/workflows/ci.yml`（trigger 段 `:2-5`；新增 job）

**Interfaces:**
- Consumes: `deny.toml`、`supply-chain/`、`scripts/supply-chain-audit.sh`（Task 1–4）
- Produces: 推送/PR 即跑的四类门禁（交付验收对象）

- [ ] **Step 1: 修正 trigger（`ci.yml:2-5`）**

将：
```yaml
on:
  push:
    branches: [main]
  pull_request:
```
替换为：
```yaml
on:
  push:
  pull_request:
```

- [ ] **Step 2: 在 `jobs:` 下新增 `supply-chain` job（完整片段；放在文件 `jobs:` 后的首位或 `host` 之前均可，本计划插在 `jobs:` 之后、`host:` 之前）**

```yaml
  supply-chain:
    # Spec 2 硬门禁：cargo-vet 信任来源 + cargo-deny 四类检查
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - run: rustup toolchain install
      - name: 安装审计工具（固定版本；必须在仓库目录外安装）
        # 仓库跟踪了 .cargo/config.toml（source.crates-io replace-with = vendored-sources → vendor/），
        # 在仓库内 cargo install 会因 source replacement 找不到 crates.io 包而失败。
        run: |
          (cd "$RUNNER_TEMP" && cargo install --locked cargo-vet@0.10.2)
          (cd "$RUNNER_TEMP" && cargo install --locked cargo-deny@0.20.2)
      - name: cargo-vet check（信任来源必须齐备）
        run: cargo vet check
      - name: cargo-deny check（advisories/licenses/bans/sources）
        run: cargo deny check
```

- [ ] **Step 3: 静态核对 YAML（本仓库无 remote，无法本地真跑）**

Run:
```bash
python3 -c "import yaml,sys; d=yaml.safe_load(open('.github/workflows/ci.yml')); print(sorted(d['jobs'])); print(d[True] if True in d else d['on'])"
rg -n 'cargo-vet@0.10.2|cargo-deny@0.20.2|cargo vet check|cargo deny check' .github/workflows/ci.yml
rg -n -A3 '^on:' .github/workflows/ci.yml
```
Expected: job 列表含 `supply-chain`（与 `host`/`nostd`/`big-endian-guard-expected-failure` 并列）；四条工具/命令命中；`on:` 段为 `push:`（无 branches 过滤）与 `pull_request:`。（若 python 无 `yaml` 模块，改用 `ruby -ryaml -e 'p YAML.load_file(".github/workflows/ci.yml")["jobs"].keys'`，或仅以 `rg` 逐行核对缩进。）

- [ ] **Step 4: 确认既有 job 未被破坏**

Run:
```bash
rg -n 'fmt --all --check|clippy|test --workspace|nostd|big-endian' .github/workflows/ci.yml | head
git diff --stat .github/workflows/ci.yml
```
Expected: 原有 4 个 job 的命令仍各自存在；diff 仅新增 job 与 trigger 两处改动。

- [ ] **Step 5: 提交**

```bash
git add .github/workflows/ci.yml
git commit -m "ci: 新增 supply-chain 硬门禁 job 并将 trigger 扩展到所有分支与 PR"
```

---

### Task 6: 文档如实化 + 账本 M14 注记

**Files:**
- Modify: `docs/superpowers/specs/2026-10-02-embedded-database-certification-template.md:45`（§2.2 表内；§2.2 标题在 :40）
- Modify: `docs/superpowers/specs/2026-10-02-embedded-database-project-summary.md:120`（§3.6 表内；标题在 :115）
- Modify: `docs/superpowers/specs/2026-10-02-embedded-database-security-design.md:99`
- Modify: `docs/superpowers/specs/2026-10-02-embedded-database-delivery-checklist.md:95`
- Modify: `README.md:85-86`（`### cargo-vet 审计` 小节在 :82）
- Modify: `.superpowers/sdd/progress.md`（在 `M14-0` 条目后插入注记）

**Interfaces:**
- Consumes: Task 1–5 的最终事实（审计覆盖范围、import 结果、CI job 名、脚本路径）
- Produces: 与 `supply-chain/` 实际内容逐字一致的文档（Spec 2 验收标准 7）

- [ ] **Step 1: `certification-template.md` 的 2.2 表**

将：
```
| `cargo-vet` 审计 | ✅ 已执行（`supply-chain/` 已生成） |
```
替换为（**按实际 import 结果填写**：7 个 deploy crate 各自的信任来源必须与 `cargo vet explain-audit` 输出一致）：
```
| `cargo-vet` 审计 | ✅ deploy 图 7 个依赖来源明确：本地审计 `zerocopy`/`zerocopy-derive`/`bitflags`；上游 import 覆盖 `proc-macro2`/`quote`/`syn`/`unicode-ident` |
| `cargo-deny` 门禁 | ✅ advisories / licenses / bans / sources 四类（`deny.toml`，CI `supply-chain` job） |
```
（若 Task 3 的降级分支把某个 build 依赖改为本地审计或带据豁免，此处的 crate 归属必须相应改写。）

- [ ] **Step 2: `project-summary.md` §3.6 表**——将同一行文本替换为与 Step 1 **完全相同**的两行。

- [ ] **Step 3: `security-design.md`:100**

将：
```
| 供应链审计 | `cargo vendor` + `cargo-vet` + SBOM | M5-3（`vendor/`、`sbom.json`） |
```
替换为：
```
| 供应链审计 | `cargo vendor` + `cargo-vet`（本地审计 + 上游 import）+ `cargo-deny` 四类门禁 + SBOM | `supply-chain/`、`deny.toml`、`vendor/`、`sbom.json`；入口 `scripts/supply-chain-audit.sh`，CI `supply-chain` job |
```

- [ ] **Step 4: `delivery-checklist.md`:95**

将：
```
| 离线构建 | cargo vendor + SBOM + cargo-vet |
```
替换为：
```
| 离线构建 | cargo vendor + SBOM |
| 供应链审计 | cargo-vet（本地审计 + 上游 import + 带据豁免）+ cargo-deny 四类；脚本 `scripts/supply-chain-audit.sh`，CI `supply-chain` job |
```

- [ ] **Step 5: `README.md` 的 cargo-vet 小节（`:85-86`）**

将：
```
当前 `supply-chain/audits.toml` 无真实审计条目，全部依赖靠 `supply-chain/config.toml`
的 38 条 exemptions 放行；真实供应链审计为独立的后续任务。CI 中未配置 `cargo vet` 门禁。
```
替换为（数字与归属按实际结果填写）：
```
`supply-chain/` 现记录真实审计结论：deploy 图 7 个第三方依赖中，`zerocopy`/`zerocopy-derive`/`bitflags`
为本地实质性审计（取证清单见 `docs/superpowers/specs/2026-10-18-supply-chain-audit-design.md` §3.3），
`proc-macro2`/`quote`/`syn`/`unicode-ident` 由上游 import 覆盖；dev 树（proptest 及其传递依赖）为带据豁免，
均含 `notes`（文本内含复审日期 2027-04-30）。公告/许可/来源门禁由 `deny.toml` + cargo-deny 提供；
本地入口 `scripts/supply-chain-audit.sh`，CI 由 `supply-chain` job 强制执行（`cargo vet check` 可离线，
`cargo deny check advisories` 需网络）。
```

- [ ] **Step 6: 账本 M14 注记**

Run:
```bash
rg -n 'M14-0' .superpowers/sdd/progress.md
```
在第 560 行 `- [x] **M14-0: cargo-vet 审计 — 完成**` 之后插入一行（保持两空格缩进）：
```
  - ⚠️ 核销 2026-10-18（Spec 2 已落实）：原判定"存疑——豁免配置非真实审计"现已闭环。现状：deploy 图 7 个第三方依赖来源明确（本地审计 zerocopy/zerocopy-derive/bitflags + 上游 import proc-macro2/quote/syn/unicode-ident）；所有豁免带 notes（文本内含复审日期 2027-04-30，cargo-vet 0.10.2 不支持 expires 字段故由 notes 承载）；cargo-deny 四类门禁进 CI（supply-chain job）。设计见 docs/superpowers/specs/2026-10-18-supply-chain-audit-design.md。
```

- [ ] **Step 7: 校验文档与事实一致**

Run:
```bash
rg -c '已执行（`supply-chain/` 已生成）' docs/superpowers/specs/2026-10-02-embedded-database-certification-template.md docs/superpowers/specs/2026-10-02-embedded-database-project-summary.md
rg -c '本地审计|上游 import' README.md docs/superpowers/specs/2026-10-02-embedded-database-certification-template.md docs/superpowers/specs/2026-10-02-embedded-database-project-summary.md docs/superpowers/specs/2026-10-02-embedded-database-security-design.md
rg -c 'Spec 2 已落实' .superpowers/sdd/progress.md
```
Expected: 第一条 `rc=1`（旧的夸大表述已全部消失）；第二条四个文件各 ≥1；第三条 = 1。

- [ ] **Step 8: 提交**

```bash
git add README.md docs/superpowers/specs/2026-10-02-embedded-database-certification-template.md docs/superpowers/specs/2026-10-02-embedded-database-project-summary.md docs/superpowers/specs/2026-10-02-embedded-database-security-design.md docs/superpowers/specs/2026-10-02-embedded-database-delivery-checklist.md .superpowers/sdd/progress.md
git commit -m "docs: 供应链审计表述如实化 + 账本 M14 核销闭环"
```

---

### Task 7: 终验（零源码改动 + 门禁 + 汇总）

**Files:**
- 无新增；只跑验证并记录（报告写入临时目录，不入库）
- Modify: 无

**Interfaces:**
- Consumes: Task 1–6 全部产物
- Produces: Spec 2 验收证据（§6 八条逐条对应）

- [ ] **Step 1: 确认改动范围只有预期文件**

Run:
```bash
git log --oneline | head -6
git show --stat --oneline HEAD | head -12
git status --short
```
Expected: 新增 5 条提交（deny.toml / vet 审计 / vet import+收窄 / 脚本 / CI / 文档，按实际拆分）；`git status` 无输出；`HEAD` 的 `--stat` 只含 Task 6 的文件。

- [ ] **Step 2: 零源码改动校验**

Run:
```bash
git diff --stat b01fed8..HEAD -- crates/ Cargo.toml Cargo.lock
```
Expected: **无输出**（Spec 2 全程未碰源码与依赖清单）。

- [ ] **Step 3: 三项工程门禁**

Run:
```bash
export PATH="/opt/homebrew/opt/rustup/bin:$PATH"
cargo fmt --all --check && echo FMT_OK
cargo clippy --workspace --all-targets --features embedded-database-core/std -- -D warnings >/dev/null && echo CLIPPY_OK
cargo test --workspace --features embedded-database-core/std 2>&1 | rg 'test result: ok' | awk '{s+=$4} END {print "tests="s}'
```
Expected: `FMT_OK`、`CLIPPY_OK`、`tests=301`。

- [ ] **Step 4: 审计门禁复核**

Run:
```bash
export PATH="$HOME/.cargo/bin:$PATH"
cargo vet check
cargo deny --offline check licenses bans sources
for spec in "zerocopy 0.8.59" "zerocopy-derive 0.8.59" "bitflags 2.13.2" "proc-macro2 1.0.107" "quote 1.0.47" "syn 2.0.119" "unicode-ident 1.0.26"; do
  set -- $spec; echo "--- $1 $2"; cargo vet explain-audit "$1" "$2" safe-to-deploy 2>&1 | tail -2
done
```
Expected: `Vetting Succeeded`；`licenses ok`/`bans ok`/`sources ok`；7 条信任路径无一条是裸 `exemption`。

- [ ] **Step 5: 豁免完备性与计数**

Run:
```bash
rg -c '^\[\[exemptions\.' supply-chain/config.toml
rg -c '^notes = ' supply-chain/config.toml
rg -c '复审日期 2027-04-30' supply-chain/config.toml
rg -c '^\s+notes = ' supply-chain/audits.toml
```
Expected: 前三个数相等（豁免条数 = 带 notes 数 = notes 含复审日期数）；最后一个 ≥ 3（三条本地审计的 notes）。

- [ ] **Step 6: CI 静态核对（无 remote，无法真跑）**

Run:
```bash
rg -n '^on:' -A3 .github/workflows/ci.yml
rg -n 'supply-chain:' -A12 .github/workflows/ci.yml
```
Expected: `on:` 下 `push:`（无 branches）与 `pull_request:`；`supply-chain` job 含 4 步（checkout / toolchain / install 两工具 / vet+deny）。

- [ ] **Step 7: 汇总报告**

在任务报告中给出验收结果表（Spec §6 八条逐条 ✅/⚠️ + 证据命令输出），并明确记录：
- 实际 import 覆盖了哪几个 crate、哪些走了降级分支；
- `advisories` 首次联网检查的结果（有无公告、如何处置）；
- CI 未真跑，需在首次推送后确认（登记为待办）。

---

## Self-Review

**Spec 覆盖**：§3.1 policy → Task 2 Step 4；§3.2 本地审计 3 个 → Task 2 Step 1–5/6；§3.2 import 4 个 + 降级 → Task 3 Step 1–3；§3.3 取证清单 → Task 2 Step 1–3（八项逐条落到具体命令）；§3.4 豁免收窄 → Task 3 Step 4–5；§3.5 不做 trust → Global Constraints；§4.1–4.3 → Task 1/2/3；§4.4 deny.toml → Task 1；§4.5 脚本 → Task 4；§4.6 CI → Task 5；§4.7 文档 → Task 6 Step 1–5；§4.8 账本 → Task 6 Step 6；§6 八条验收 → Task 7 Step 1–6；§7 风险（prune、advisories 网络、clarify、命令形式）→ Task 2 Step 6、Task 1 Step 2/3、Global Constraints。

**占位符扫描**：无 TBD/TODO；每个 code step 含完整文件内容或完整命令；条件分支均为"依实测数据二选一"的确定性规则（import 覆盖与否 / advisories 有网与无网），并有对应验证步骤。

**类型/名称一致性**：`who = "embedded-database maintainers"`、`criteria`、`notes`（统一含复审日期 `2027-04-30`）全文统一；脚本路径 `scripts/supply-chain-audit.sh`、配置 `deny.toml`、job 名 `supply-chain` 全文统一；7 个 deploy crate 及其版本三处（Global Constraints、Task 2/3 的 explain-audit 循环、Task 7 Step 4）逐字一致。
