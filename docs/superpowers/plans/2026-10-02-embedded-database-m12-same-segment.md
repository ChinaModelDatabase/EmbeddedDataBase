# embedded-database M12 实施计划：同段追加

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `flush` 时检查当前活跃段剩余空间，有则追加写入（复用 `SegmentWriter`），无则开新段。减少段数、提高空间利用率。

**Architecture:** `Db` 持有 `active_writer: Option<SegmentWriter>`（std 门控），`flush` 时若当前 writer 有剩余空间则追加写入，否则 commit 当前段并开新段。`Db::compact` 时 commit 当前 writer 再合并。

**Tech Stack:** Rust 1.81、`embedded-database-core`（no_std + std 门控）、`SegmentWriter`（M1 Task 7）。

## Global Constraints

- **C1** `embedded-database-core` 保持 `#![no_std]`
- **C2** 顶层 `#![forbid(unsafe_code)]`——不允许任何局部 `unsafe`
- **C3** 运行时依赖仅 `zerocopy`、`bitflags`，不新增
- **C4** 库代码禁止 `unwrap()` / `expect()` / `panic!()` / 数组越界（`unwrap()` 仅允许 `#[cfg(test)]`）
- **C7** 永不阻塞、永不自旋；Deadline 超时返回 `DeadlineExceeded`
- **C11** 不执行任何 git 提交（本项目零提交策略）
- **工具链**：每个 bash 调用前 `export PATH="/opt/homebrew/opt/rustup/bin:$PATH"`
- **验收基线**：M11 交付 216 passed（core 208 + kv 3 + os 5）、5 目标 no_std 编译通过、`inject_crash`/`wcet` exit=0

## 用户裁决记录

- **M12 范围**：同段追加 + 测试 + 收尾验收。

---

### Task M12-0: 同段追加

**Files:**
- Modify: `crates/embedded-database-core/src/db.rs`

**Interfaces:**
- Consumes: `SegmentWriter`（M1 Task 7，含 `append_page`/`commit`/`committed`）、`Db<'d>`（M4-3）
- Produces:
  - `Db` 持有 `active_writer: Option<SegmentWriter<'d>>`（std 门控）
  - `flush` 时若 `active_writer` 有剩余空间则追加，否则 commit 并开新段

**关键设计**：
- `SegmentWriter` 每次 `append_page` 消耗 `page_size` 字节（顺序追加区）。
- 段剩余空间 = `segment_size - SEGMENT_HEADER_SIZE - COMMIT_RECORD_SIZE - cursor`。
- `flush` 检查 `pending_writes` 总字节是否 ≤ 剩余空间。
- 若是：复用 `active_writer` 追加写入。
- 若否：commit 当前 writer，开新 writer。
- `Db::compact` 时先 commit 当前 writer（若有）。

- [ ] **Step 1: 写失败测试**

```rust
#[test]
fn 同段追加_多次flush复用同一段() {
    let mut dev = InMemoryDevice::new(1 << 20);
    let mut db = Db::open(&mut dev, Config::default()).unwrap();
    // 第一次 flush
    {
        let mut txn = db.begin(TxnOptions::default()).unwrap();
        txn.put(b"key1", b"value1").unwrap();
        txn.commit().unwrap();
    }
    db.flush().unwrap();
    let seq_after_first = db.active_segment();
    // 第二次 flush（同段应有空间）
    {
        let mut txn = db.begin(TxnOptions::default()).unwrap();
        txn.put(b"key2", b"value2").unwrap();
        txn.commit().unwrap();
    }
    db.flush().unwrap();
    let seq_after_second = db.active_segment();
    // M12：同段追加，active_segment 不应递增
    assert_eq!(seq_after_first, seq_after_second, "同段追加时 active_segment 不应递增");
}

#[test]
fn 同段追加_空间耗尽后开新段() {
    let mut dev = InMemoryDevice::new(1 << 20);
    let mut db = Db::open(&mut dev, Config::default()).unwrap();
    // 多次 flush 直到空间耗尽
    for i in 0..10 {
        let mut txn = db.begin(TxnOptions::default()).unwrap();
        txn.put(format!("key{i}").as_bytes(), format!("value{i}").as_bytes()).unwrap();
        txn.commit().unwrap();
        db.flush().unwrap();
    }
    // 空间耗尽后应开新段
    assert!(db.active_segment() > 1, "空间耗尽后应开新段");
}
```

- [ ] **Step 2: 运行测试验证失败**

Run: `cargo test -p embedded-database-core --features std db 2>&1 | tail -10`
Expected: `同段追加_多次flush复用同一段` 失败（当前每次 flush 占新段）

- [ ] **Step 3: 实现**

```rust
// Db 结构体追加字段（std 门控）
#[cfg(feature = "std")]
active_writer: Option<SegmentWriter<'d>>,

// Db::open 初始化
#[cfg(feature = "std")]
active_writer: None,

// flush 修改
pub fn flush(&mut self) -> Result<CommitInfo, Error> {
    if self.pending_writes.borrow().is_empty() {
        return Ok(CommitInfo {
            segment_seq: self.active_segment,
            bytes_written: 0,
        });
    }

    let segment_seq = self.active_segment;
    let device_offset = ((segment_seq as usize).saturating_sub(1))
        .checked_mul(self.cfg.segment_size)
        .ok_or(Error::InvalidArgument)? as u64;

    // 计算 pending_writes 总字节
    let pending_bytes: u64 = self.pending_writes.borrow().iter()
        .map(|(_, payload)| payload.len() as u64)
        .sum();

    // 检查当前 writer 是否有足够空间
    let scfg = SegmentConfig {
        segment_seq,
        prev_segment: segment_seq.saturating_sub(1),
        prev_crc: self.last_commit_crc,
        device_offset,
        segment_size: self.cfg.segment_size,
        page_size: self.cfg.page_size,
        slots_per_page: self.cfg.slots_per_page,
        max_pages: self.cfg.max_pages,
        encryption_key: self.cfg.encryption_key,
    };

    // M12：检查 active_writer 是否有剩余空间
    let can_append = match &self.active_writer {
        Some(w) => {
            let remaining = self.cfg.segment_size
                .saturating_sub(56) // SEGMENT_HEADER_SIZE
                .saturating_sub(40) // COMMIT_RECORD_SIZE
                .saturating_sub(w.cursor() as usize);
            pending_bytes <= remaining as u64
        }
        None => false,
    };

    let mut bytes_written = 0u64;
    let mut placed = std::vec::Vec::new();
    let mut key_updates = std::vec::Vec::new();

    if can_append {
        // 追加到当前段
        let writer = self.active_writer.as_mut().unwrap();
        let pending = self.pending_writes.borrow();
        for (page_id, payload) in pending.iter() {
            let app = writer.append_page(*page_id, payload)?;
            placed.push((*page_id, app.offset));
            bytes_written = bytes_written
                .checked_add(self.cfg.page_size as u64)
                .ok_or(Error::CapacityExhausted)?;
            if let Some(sep) = payload.iter().position(|b| *b == 0xFF) {
                key_updates.push((payload[..sep].to_vec(), *page_id));
            }
        }
        drop(pending);
        // 不 commit，保留 writer 供下次追加
    } else {
        // 开新段
        if let Some(mut old_writer) = self.active_writer.take() {
            old_writer.commit(0)?;
            self.dev.sync()?;
        }
        let mut writer = SegmentWriter::new(self.dev, scfg)?;
        let pending = self.pending_writes.borrow();
        for (page_id, payload) in pending.iter() {
            let app = writer.append_page(*page_id, payload)?;
            placed.push((*page_id, app.offset));
            bytes_written = bytes_written
                .checked_add(self.cfg.page_size as u64)
                .ok_or(Error::CapacityExhausted)?;
            if let Some(sep) = payload.iter().position(|b| *b == 0xFF) {
                key_updates.push((payload[..sep].to_vec(), *page_id));
            }
        }
        drop(pending);
        // 不 commit，保留 writer
        self.active_writer = Some(writer);
        self.active_segment = segment_seq.saturating_add(1);
    }

    // 更新 page_map / key_map / pending_writes 清空
    // ...（与现有逻辑相同）

    Ok(CommitInfo { segment_seq, bytes_written })
}
```

⚠️ **注意**：`SegmentWriter` 需要 `cursor()` 方法（当前无）。在 `writer.rs` 追加：

```rust
pub fn cursor(&self) -> u64 {
    self.cursor
}
```

- [ ] **Step 4: 运行测试**

Run: `cargo test -p embedded-database-core --features std db 2>&1 | tail -10`
Expected: 54 passed（52 已有 + 2 新增）

- [ ] **Step 5: 检查点**

Run: `rg -n 'active_writer' crates/embedded-database-core/src/db.rs | head -5`
Expected: 有多处输出

---

### Task M12-1: M12 收尾验收

**Files:**
- Modify: `docs/superpowers/specs/2026-10-02-embedded-database-design.md`

**Interfaces:**
- Consumes: 全部 M12 Task
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

在 `2026-10-02-embedded-database-design.md` 的「实施状态」小节追加 M12 交付内容：

```markdown
## 实施状态（M12 完成）

- M12 交付：同段追加——`flush` 时检查当前活跃段剩余空间，有则追加写入
  （复用 `SegmentWriter`），无则 commit 当前段并开新段。减少段数、提高空间利用率。
- **M12 简化**：
  - `active_writer` 用 `Option<SegmentWriter>`（std 门控）。
  - 空间检查基于 `segment_size - SEGMENT_HEADER_SIZE - COMMIT_RECORD_SIZE - cursor`。
  - 旧段物理擦除留待后续。
```

- [ ] **Step 7: 检查点**

Run: `rg -n 'M12 完成' docs/superpowers/specs/2026-10-02-embedded-database-design.md`
Expected: 有输出

---

## M12 验收标准

| # | 标准 | 验证方式 |
|---|------|----------|
| 1 | 同段追加（多次 flush 复用同一段） | M12-0 测试 |
| 2 | 空间耗尽后开新段 | M12-0 测试 |
| 3 | 5 个 no_std 目标全部编译通过 | M12-1 Step 3 |
| 4 | 全 crate 零 `unsafe` | M12-1 Step 4 |
| 5 | 全部测试通过 | M12-1 Step 1 |
| 6 | `inject_crash` exit=0 且 `wcet` exit=0 | M12-1 Step 5 |

## 后续里程碑

M12 完成后，同段追加功能完整。剩余工作：

- **旧段物理擦除**（compaction 后清零设备空间）
- **LoongArch 实机验证**（拿到硬件后）
- **认证申报**（GB/T 22239、CSTC 适配）
- **试点部署**
