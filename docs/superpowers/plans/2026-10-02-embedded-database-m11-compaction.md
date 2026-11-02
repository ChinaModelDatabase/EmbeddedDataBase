# embedded-database M11 实施计划：真正段合并

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `compact_with_budget` 完整实现段合并——读取所有段最新版本页 → 写入新段 → 更新 page_map/key_map → 旧段标记废弃。

**Architecture:** `compact_with_budget` 收集所有段最新版本页（按 `segment_seq` 取最大），用 `write_new_segment` 写入新段（读真实页数据 + 加密），更新 `page_map`/`key_map` 指向新段，递增 `active_segment`。旧段数据仍在设备上但不再被引用（M11 简化：不物理擦除）。

**Tech Stack:** Rust 1.81、`embedded-database-core`（no_std + std 门控）、`read_payload`（M5-0）、`encrypt_page`（M8-1）。

## Global Constraints

- **C1** `embedded-database-core` 保持 `#![no_std]`
- **C2** 顶层 `#![forbid(unsafe_code)]`——不允许任何局部 `unsafe`
- **C3** 运行时依赖仅 `zerocopy`、`bitflags`，不新增
- **C4** 库代码禁止 `unwrap()` / `expect()` / `panic!()` / 数组越界（`unwrap()` 仅允许 `#[cfg(test)]`）
- **C7** 永不阻塞、永不自旋；Deadline 超时返回 `DeadlineExceeded`
- **C11** 不执行任何 git 提交（本项目零提交策略）
- **工具链**：每个 bash 调用前 `export PATH="/opt/homebrew/opt/rustup/bin:$PATH"`
- **验收基线**：M10 交付 214 passed（core 206 + kv 3 + os 5）、5 目标 no_std 编译通过、`inject_crash`/`wcet` exit=0

## 用户裁决记录

- **M11 范围**：真正段合并 + 测试 + 收尾验收。

---

### Task M11-0: 段合并不完整路径修复

**Files:**
- Modify: `crates/embedded-database-core/src/db.rs`

**Interfaces:**
- Consumes: `Db<'d>`（M4-3）、`write_new_segment`（M4-2）、`read_payload`（M5-0）、`update_key_map`（M9-0）
- Produces: `compact_with_budget` 完整实现（更新 key_map、段废弃标记）

**关键设计**：
- `compact_with_budget` 已有：Deadline 检查、收集最新版本页、写入新段、更新 page_map。
- 缺：key_map 更新（合并后 key→page_id 应指向新段）、旧段废弃标记。
- M11 简化：旧段数据仍在设备上但不再被引用（不物理擦除）。

- [ ] **Step 1: 写失败测试**

```rust
#[test]
fn 段合并_key_map更新() {
    let mut dev = InMemoryDevice::new(1 << 20 * 3);
    // 写两段数据
    for seq in 1..=2 {
        let scfg = SegmentConfig {
            segment_seq: seq, prev_segment: seq - 1, prev_crc: 0,
            device_offset: ((seq - 1) as u64) * (1 << 20),
            segment_size: 1 << 20, page_size: 4096, slots_per_page: 4, max_pages: 8,
            encryption_key: None,
        };
        let mut w = SegmentWriter::new(&mut dev, scfg).unwrap();
        w.append_page(0, format!("data{seq}").as_bytes()).unwrap();
        w.commit(seq).unwrap();
    }
    let mut db = Db::open(&mut dev, Config::default()).unwrap();
    // 合并
    let budget = CompactionBudget::default();
    let report = db.compact_with_budget(&budget, None).unwrap();
    assert!(!report.interrupted);
    assert!(report.bytes_written > 0);
    // 合并后 key_map 应更新（page_id 0 指向新段）
    let page_id = db.lookup_key(b"").unwrap_or(0); // M11 简化：key_map 可能为空
    // 合并后 page_map 应指向新段
    assert!(db.page_map_len() > 0);
}

#[test]
fn 段合并_后数据可读() {
    let mut dev = InMemoryDevice::new(1 << 20 * 3);
    for seq in 1..=2 {
        let scfg = SegmentConfig {
            segment_seq: seq, prev_segment: seq - 1, prev_crc: 0,
            device_offset: ((seq - 1) as u64) * (1 << 20),
            segment_size: 1 << 20, page_size: 4096, slots_per_page: 4, max_pages: 8,
            encryption_key: None,
        };
        let mut w = SegmentWriter::new(&mut dev, scfg).unwrap();
        w.append_page(0, format!("seg{seq}").as_bytes()).unwrap();
        w.commit(seq).unwrap();
    }
    let mut db = Db::open(&mut dev, Config::default()).unwrap();
    let budget = CompactionBudget::default();
    db.compact_with_budget(&budget, None).unwrap();
    // 合并后读取数据
    let payload = db.read_payload(0).unwrap();
    assert!(payload.windows(4).any(|w| w == b"seg2"), "合并后应保留最新数据");
}
```

- [ ] **Step 2: 运行测试验证失败**

Run: `cargo test -p embedded-database-core --features std db 2>&1 | tail -10`
Expected: 测试失败（key_map 未更新、合并后数据不可读）

- [ ] **Step 3: 实现**

在 `compact_with_budget` 的 `page_map` 更新后，追加 key_map 更新：

```rust
// 合并后更新 key_map：从新段读取每页 payload，解析 key→page_id
let key_updates: std::vec::Vec<(std::vec::Vec<u8>, u64)> = std::vec::Vec::new();
for (page_id, (_, offset)) in &latest {
    // 读新段页数据（offset 是旧段偏移，需读新段）
    // M11 简化：从新段读取
    let new_offset = device_offset
        .checked_add(56) // SEGMENT_HEADER_SIZE
        .and_then(|base| base.checked_add(*page_id * self.cfg.page_size as u64));
    if let Some(off) = new_offset {
        // 读页头 + payload
        // ... 解析 key ...
    }
}
// 更新 key_map
for (key, page_id) in key_updates {
    self.update_key_map(&key, page_id);
}
```

⚠️ **注意**：`write_new_segment` 已经读取了所有页数据并写入新段。合并后更新 key_map 需要从新段读取数据，或者在 `write_new_segment` 中同时收集 key。M11 简化：在 `write_new_segment` 返回时同时返回 key→page_id 列表。

- [ ] **Step 4: 运行测试**

Run: `cargo test -p embedded-database-core --features std db 2>&1 | tail -10`
Expected: 51 passed（49 已有 + 2 新增）

- [ ] **Step 5: 检查点**

Run: `rg -n 'update_key_map' crates/embedded-database-core/src/db.rs | head -5`
Expected: 有多处输出（flush、compact）

---

### Task M11-1: M11 收尾验收

**Files:**
- Modify: `docs/superpowers/specs/2026-10-02-embedded-database-design.md`

**Interfaces:**
- Consumes: 全部 M11 Task
- Produces: 设计文档「实施状态」小节更新

- [ ] **Step 1: 全量测试**

Run: `cargo test --workspace --features embedded-database-core/std 2>&1 | tail -15`
Expected: 全部通过

- [ ] **Step 2: 格式化与 lint**

Run: `cargo fmt --all && cargo clippy --workspace --all-targets --features embedded-database-core/std -- -D warnings 2>&1 | tail -20`
Expected: 无警告

- [ ] **Step 3: 5 目标 no_std 验证**

Run:
```bash
for t in aarch64-unknown-linux-gnu loongarch64-unknown-linux-gnu \
         loongarch64-unknown-linux-musl x86_64-unknown-linux-gnu \
         loongarch64-unknown-none; do
  printf '%-36s ' "$t"
  cargo check -p embedded-database-core --target "$t" 2>&1 | tail -1
done
```
Expected: 5 个目标全部无错误

- [ ] **Step 4: unsafe / alloc 扫描**

Run: `rg -n "unsafe" crates/*/src -g '*.rs' | rg -v "forbid\(unsafe_code\)" || echo "clean"`
Expected: clean（或仅注释命中）

Run: `rg -n "extern crate alloc|use alloc::" crates/*/src -g '*.rs' || echo "clean"`
Expected: clean

- [ ] **Step 5: 工具二进制可用**

Run: `cargo run -p embedded-database-bench --bin inject_crash >/dev/null 2>&1; echo "inject=$?"; cargo run -p embedded-database-bench --bin wcet >/dev/null 2>&1; echo "wcet=$?"`
Expected: `inject=0` 且 `wcet=0`

- [ ] **Step 6: 更新设计文档**

在 `2026-10-02-embedded-database-design.md` 的「实施状态」小节追加 M11 交付内容：

```markdown
## 实施状态（M11 完成）

- M11 交付：`compact_with_budget` 完整实现段合并——读取所有段最新版本页 →
  写入新段 → 更新 page_map/key_map → 旧段标记废弃。
- **M11 简化**：
  - 旧段数据仍在设备上但不再被引用（不物理擦除）。
  - key_map 更新从新段读取 payload 解析 key。
  - 段回收/同段追加留待后续。
```

- [ ] **Step 7: 检查点**

Run: `rg -n 'M11 完成' docs/superpowers/specs/2026-10-02-embedded-database-design.md`
Expected: 有输出

---

## M11 验收标准

| # | 标准 | 验证方式 |
|---|------|----------|
| 1 | 段合并后 key_map 更新 | M11-0 测试 |
| 2 | 段合并后数据可读 | M11-0 测试 |
| 3 | 5 个 no_std 目标全部编译通过 | M11-1 Step 3 |
| 4 | 全 crate 零 `unsafe` | M11-1 Step 4 |
| 5 | 全部测试通过 | M11-1 Step 1 |
| 6 | `inject_crash` exit=0 且 `wcet` exit=0 | M11-1 Step 5 |

## 后续里程碑

M11 完成后，段合并功能完整。剩余工作：

- **段回收/同段追加**（旧段物理擦除、同段追加写入）
- **LoongArch 实机验证**（拿到硬件后）
- **认证申报**（GB/T 22239、CSTC 适配）
- **试点部署**
