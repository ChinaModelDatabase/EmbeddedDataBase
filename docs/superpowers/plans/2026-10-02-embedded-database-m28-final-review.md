# embedded-database M28 实施计划：全分支终审

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 对 M1–M27 全部交付做最终代码评审，修复所有阻断问题，确保项目可交付。

**Architecture:** 生成全分支评审包（全部源文件 + 文档），派发终审 subagent，修复发现的阻断问题，更新设计文档。

**Tech Stack:** 评审包生成 + Markdown 文档。

## Global Constraints

- **C11** 不执行任何 git 提交（本项目零提交策略）
- **工具链**：每个 bash 调用前 `export PATH="/opt/homebrew/opt/rustup/bin:$PATH"`
- **验收基线**：M27 交付 288 passed（core 215 + kv 6 + os 5 + sql 62）、5 目标 no_std 编译通过、`inject_crash`/`wcet` exit=0

## 用户裁决记录

- **M28 范围**：全分支终审。

---

### Task M28-0: 全分支终审

**Files:**
- Create: `.superpowers/sdd/m28-final-package.md`
- Create: `.superpowers/sdd/m28-final-review.md`

**Interfaces:**
- Consumes: 全部 M1–M27 源文件与文档
- Produces: 终审评审报告

- [ ] **Step 1: 生成评审包**

```bash
cd /Users/caosunliu/opencode/EmbeddedDataBase
python3 - <<'PY'
import os
files=[]
for root,_,fnames in os.walk("crates"):
    for f in sorted(fnames):
        if f.endswith(".rs"): files.append(os.path.join(root,f))
for extra in ["Cargo.toml","Cargo.lock","rust-toolchain.toml",".github/workflows/ci.yml","README.md","scripts/offline-build.sh",".cargo/config.toml"]:
    files.append(extra)
out=["# M28 全分支终审评审包\n\n本项目零提交，全部文件为新增/修改。\n\n"]
for f in files:
    try: c=open(f).read()
    except: continue
    lang="rust" if f.endswith(".rs") else ("yaml" if f.endswith((".yml",".yaml")) else "toml")
    if f.endswith(".sh"): lang="bash"
    out.append(f"## FILE: {f}\n\n```{lang}\n{c}\n```\n\n")
open(".superpowers/sdd/m28-final-package.md","w").write("".join(out))
print(f"package: {sum(len(o) for o in out)} bytes, {len(files)} files")
PY
```

- [ ] **Step 2: 派发终审 subagent**

生成评审包后，派发终审 subagent 评审全部交付。

- [ ] **Step 3: 修复阻断问题**

若终审发现阻断问题，派发修复 subagent。

- [ ] **Step 4: 检查点**

Run: `cargo test --workspace --features embedded-database-core/std 2>&1 | rg '^test result:' | rg -v '0 passed'`
Expected: 全部通过

---

### Task M28-1: M28 收尾验收

**Files:**
- Modify: `docs/superpowers/specs/2026-10-02-embedded-database-design.md`

**Interfaces:**
- Consumes: 全部 M28 Task
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

在 `2026-10-02-embedded-database-design.md` 的「实施状态」小节追加 M28 交付内容：

```markdown
## 实施状态（M28 完成）

- M28 交付：全分支终审——对 M1–M27 全部交付做最终代码评审，修复所有阻断问题。
- **项目状态**：M1–M28 全部完成，288 测试全绿，0 提交。项目代码与文档交付已全部完成。
```

- [ ] **Step 6: 检查点**

Run: `rg -n 'M28 完成' docs/superpowers/specs/2026-10-02-embedded-database-design.md`
Expected: 有输出

---

## M28 验收标准

| # | 标准 | 验证方式 |
|---|------|----------|
| 1 | 全分支终审完成 | M28-0 |
| 2 | 阻断问题已修复 | M28-0 Step 3 |
| 3 | 设计文档含「实施状态（M28 完成）」小节 | M28-1 Step 5 |
| 4 | 全部测试通过 | M28-1 Step 1 |
| 5 | `inject_crash` exit=0 且 `wcet` exit=0 | M28-1 Step 4 |

## 后续里程碑

M28 完成后，M1–M28 全部交付。剩余工作为**外部依赖**：

- LoongArch 实机验证（需龙芯硬件）
- 认证申报（需 CSTC 提交）
- 试点部署（需现场环境）
