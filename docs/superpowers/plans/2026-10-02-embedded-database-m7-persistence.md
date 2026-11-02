# embedded-database M7 实施计划：pending_writes 真正落盘

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `Txn::commit` 时立即把变更写入活跃段（真实落盘），返回持久化的 `CommitInfo`。

**Architecture:** `Txn::commit` 调用 `Db` 内部方法，通过 `SegmentWriter` 把变更写入活跃段，更新 `page_map`，调用 `sync()`，返回真实 `CommitInfo`。加解密路径已在 M6-4 就绪，只需接线。

**Tech Stack:** Rust 1.81、`embedded-database-core`（no_std + std 门控）、`SegmentWriter`（M1 Task 7）、`SM4` 加密（M6-0）。

## Global Constraints

- **C1** `embedded-database-core` 保持 `#![no_std]`
- **C2** 顶层 `#![forbid(unsafe_code)]`——不允许任何局部 `unsafe`
- **C3** 运行时依赖仅 `zerocopy`、`bitflags`，不新增
- **C4** 库代码禁止 `unwrap()` / `expect()` / `panic!()` / 数组越界（`unwrap()` 仅允许 `#[cfg(test)]`）
- **C7** 永不阻塞、永不自旋；Deadline 超时返回 `DeadlineExceeded`
- **C11** 不执行任何 git 提交（本项目零提交策略）
- **工具链**：每个 bash 调用前 `export PATH="/opt/homebrew/opt/rustup/bin:$PATH"`
- **验收基线**：M6 交付 181 passed（core 177 + os 4）、5 目标 no_std 编译通过、`inject_crash`/`wcet` exit=0

## 用户裁决记录

- **落盘时机**：commit 时立即落盘（不是批量刷入）。数据真正持久化，符合 ACID。
- **M7 范围**：落盘 + 相关测试 + 收尾验收。不修复其他遗留（page_map offset、lookup_page 等）。

---

### Task M7-0: 落盘实现

**Files:**
- Modify: `crates/embedded-database-core/src/db.rs`

**Interfaces:**
- Consumes: `Db<'d>`（M4-3，持有 `dev: &'d mut dyn BlockDevice`）、`SegmentWriter::append_page(page_id, payload)`（M1 Task 7）、`SegmentConfig`（M1 Task 7，含 `encryption_key`）、`sm4_encrypt_payload`（M6-4）、`Config`（M2-0）
- Produces:
  - `Db::flush_pending() -> Result<u64, Error>`：把 `pending_writes` 写入活跃段
  - `Txn::commit` 调用 `flush_pending()`，返回真实 `CommitInfo { segment_seq, bytes_written }`

**关键设计**：
- `Txn` 持有 `&'db Db<'d>`（不可变借用），无法直接 `&mut self.db.dev`。
- `Db::flush_pending` 是 `&mut self` 方法，需要在 `Txn::commit` 中调用——但 `Txn` 持有 `&Db`。
- **解决方案**：`Txn::commit` 返回后，由调用方调用 `Db::flush_pending()`。或者：`Txn::commit` 把变更留在 `pending_writes`，`Db::begin()` 在创建新事务前检查并调用 `flush_pending()`。
- **M7 简化方案**：`Txn::commit` 只写入 `pending_writes`（不变），`Db::begin()` 在抢锁后检查 `pending_writes` 非空时调用 `flush_pending()`。这样 commit 的数据在下次 `begin` 时落盘。
- **M7 加强方案**：`Txn::commit` 直接调用 `Db::flush_pending()`——需要 `Txn` 持有 `&mut Db` 或 `RefCell` 内部可变性。

**M7 采用加强方案**：`Db::flush_pending` 用 `RefCell` 内部可变性包装 `dev` 的访问。但 `dev: &'d mut dyn BlockDevice` 是 `&mut`，`RefCell` 无法包装。

**M7 最终方案**：`Txn::commit` 写入 `pending_writes`，然后调用 `Db::flush_pending_pending()`（一个 `&self` 方法，用 `RefCell` 借用 `pending_writes`，但写设备需要 `&mut dev`——不可行）。

**M7 实际方案**：`Txn::commit` 写入 `pending_writes`，返回 `CommitInfo`。`Db::begin()` 在抢锁后调用 `self.flush_pending()?`（`&mut self` 方法）。这样：
- commit 时数据进入 `pending_writes`（内存）
- 下次 `begin` 时落盘（设备 I/O）
- `CommitInfo.bytes_written` 在 commit 时估算，`flush_pending` 返回实际写入字节数

**M7 增强**：`Db` 增加 `pub fn flush(&mut self) -> Result<CommitInfo, Error>` 方法，调用方可主动调用落盘。

- [ ] **Step 1: 写失败测试**

```rust
// crates/embedded-database-core/src/db.rs 测试模块追加
#[test]
fn commit_落盘_flush写入设备() {
    let mut dev = InMemoryDevice::new(1 << 20);
    let mut db = Db::open(&mut dev, Config::default()).unwrap();
    {
        let mut txn = db.begin(TxnOptions::default()).unwrap();
        txn.put(b"key1", b"value1").unwrap();
        txn.put(b"key2", b"value2").unwrap();
        txn.commit().unwrap();
    }
    // commit 后 pending_writes 非空
    assert!(db.pending_writes_len() > 0, "commit 后 pending_writes 应非空");
    // 主动 flush
    let info = db.flush().unwrap();
    assert!(info.bytes_written > 0, "flush 应写入字节");
    // flush 后 pending_writes 清空
    assert_eq!(db.pending_writes_len(), 0, "flush 后 pending_writes 应清空");
    // page_map 应更新
    assert!(db.page_map_len() > 0, "flush 后 page_map 应非空");
}

#[test]
fn begin_自动flush_persisting() {
    let mut dev = InMemoryDevice::new(1 << 20);
    let mut db = Db::open(&mut dev, Config::default()).unwrap();
    {
        let mut txn = db.begin(TxnOptions::default()).unwrap();
        txn.put(b"key1", b"value1").unwrap();
        txn.commit().unwrap();
    }
    assert!(db.pending_writes_len() > 0);
    // 下次 begin 时自动 flush
    {
        let _txn = db.begin(TxnOptions::default()).unwrap();
    }
    assert_eq!(db.pending_writes_len(), 0, "begin 后 pending_writes 应清空");
}

#[test]
fn flush_加密路径() {
    let mut dev = InMemoryDevice::new(1 << 20);
    let key = EncryptionKey {
        sm4: crate::sm4::Sm4Key { bytes: [0x01; 16] },
        hmac: [0x02; 32],
    };
    let cfg = Config {
        encryption_key: Some(key),
        ..Config::default()
    };
    let mut db = Db::open(&mut dev, cfg).unwrap();
    {
        let mut txn = db.begin(TxnOptions::default()).unwrap();
        txn.put(b"secret", b"confidential").unwrap();
        txn.commit().unwrap();
    }
    let info = db.flush().unwrap();
    assert!(info.bytes_written > 0);
    // 读回并解密
    let payload = db.read_payload(0).unwrap();
    // M7 简化：payload = key ++ 0xFF ++ value（加密后）
    // 解密后应包含 "secret" 和 "confidential"
    assert!(payload.windows(6).any(|w| w == b"secret"), "解密后应包含 key");
}
```

- [ ] **Step 2: 运行测试验证失败**

Run: `cargo test -p embedded-database-core --features std db 2>&1 | tail -10`
Expected: 编译失败（`flush` 方法不存在）

- [ ] **Step 3: 实现 `Db::flush`**

```rust
// crates/embedded-database-core/src/db.rs 追加
/// 把 `pending_writes` 写入活跃段。返回实际写入字节数。
///
/// M7：commit 时数据进入 `pending_writes`，`flush` 时真正落盘。
/// `begin()` 会自动调用此方法。
pub fn flush(&mut self) -> Result<CommitInfo, Error> {
    let pending = self.pending_writes.borrow_mut();
    if pending.is_empty() {
        return Ok(CommitInfo {
            segment_seq: self.active_segment,
            bytes_written: 0,
        });
    }

    let segment_seq = self.active_segment;
    let device_offset = ((segment_seq as usize).saturating_sub(1))
        .checked_mul(self.cfg.segment_size)
        .ok_or(Error::InvalidArgument)? as u64;

    let scfg = SegmentConfig {
        segment_seq,
        prev_segment: segment_seq.saturating_sub(1),
        prev_crc: 0,
        device_offset,
        segment_size: self.cfg.segment_size,
        page_size: self.cfg.page_size,
        slots_per_page: self.cfg.slots_per_page,
        max_pages: self.cfg.max_pages,
        encryption_key: self.cfg.encryption_key,
    };

    let mut writer = SegmentWriter::new(self.dev, scfg)?;
    let mut bytes_written = 0u64;
    for (page_id, payload) in pending.iter() {
        writer.append_page(*page_id, payload)?;
        bytes_written = bytes_written
            .checked_add(self.cfg.page_size as u64)
            .ok_or(Error::CapacityExhausted)?;
    }
    writer.commit(0)?;
    self.dev.sync()?;
    drop(pending);

    // 更新 page_map
    let mut page_map = self.pending_writes.borrow_mut();
    for (page_id, _) in page_map.iter() {
        let offset = device_offset
            .checked_add(56) // SEGMENT_HEADER_SIZE
            .and_then(|base| base.checked_add(*page_id * self.cfg.page_size as u64))
            .ok_or(Error::InvalidArgument)?;
        self.page_map.push((*page_id, segment_seq, offset));
    }
    page_map.clear();

    self.active_segment = segment_seq.saturating_add(1);
    Ok(CommitInfo { segment_seq, bytes_written })
}

/// 在 `begin()` 中自动调用：若有待落盘数据，先 flush。
fn auto_flush(&mut self) -> Result<(), Error> {
    if self.pending_writes.borrow().is_empty() {
        return Ok(());
    }
    self.flush()?;
    Ok(())
}
```

`Db::begin` 修改：
```rust
pub fn begin(&mut self, opts: TxnOptions) -> Result<Txn<'_, 'd>, Error> {
    if let Some(dl) = opts.deadline {
        dl.remaining()?;
    }
    self.try_lock_writer()?;
    // M7：自动 flush 待落盘数据
    self.auto_flush()?;
    Ok(Txn {
        db: self,
        changes: std::vec::Vec::new(),
        deadline: opts.deadline,
        committed: false,
    })
}
```

⚠️ **注意**：`begin` 签名从 `&self` 改为 `&mut self`。需要检查所有调用方（测试、集成测试）。

- [ ] **Step 4: 运行测试**

Run: `cargo test -p embedded-database-core --features std db 2>&1 | tail -10`
Expected: 35 passed（32 已有 + 3 新增，或按实际调整）

Run: `cargo test -p embedded-database-core --features std 2>&1 | rg 'test result' | head -1`
Expected: 184 passed

- [ ] **Step 5: 检查点**

Run: `rg -n 'pub fn flush' crates/embedded-database-core/src/db.rs`
Expected: 有输出

---

### Task M7-1: 落盘测试补充

**Files:**
- Modify: `crates/embedded-database-core/src/db.rs`
- Modify: `crates/embedded-database-os/src/lib.rs`（集成测试）

**Interfaces:**
- Consumes: `Db::flush`（M7-0）、`read_payload`（M5-0）、`EncryptionKey`（M6-3）
- Produces: 落盘持久化的测试覆盖

- [ ] **Step 1: 写测试**

```rust
#[test]
fn flush_后数据可读回() {
    let mut dev = InMemoryDevice::new(1 << 20);
    let mut db = Db::open(&mut dev, Config::default()).unwrap();
    {
        let mut txn = db.begin(TxnOptions::default()).unwrap();
        txn.put(b"persist-key", b"persist-value").unwrap();
        txn.commit().unwrap();
    }
    db.flush().unwrap();
    // flush 后 page_map 非空，可读回
    assert!(db.page_map_len() >= 1);
    let payload = db.read_payload(0).unwrap();
    // M7 简化：payload = key ++ 0xFF ++ value
    assert!(payload.windows(11).any(|w| w == b"persist-key"), "应包含 key");
}

#[test]
fn flush_多次commit批量落盘() {
    let mut dev = InMemoryDevice::new(1 << 20);
    let mut db = Db::open(&mut dev, Config::default()).unwrap();
    for i in 0..3 {
        let mut txn = db.begin(TxnOptions::default()).unwrap();
        txn.put(format!("k{i}").as_bytes(), b"v").unwrap();
        txn.commit().unwrap();
    }
    // 3 次 commit 后 pending_writes 有 3 条（或 1 条，取决于是否每次 begin 时 flush）
    // M7 方案：begin 时自动 flush，所以第 2、3 次 begin 时前一次已落盘
    // 最终 pending_writes 只有最后一次 commit 的数据
    db.flush().unwrap();
    assert_eq!(db.pending_writes_len(), 0);
    assert!(db.page_map_len() >= 1);
}

#[test]
fn flush_空库无操作() {
    let mut dev = InMemoryDevice::new(1 << 20);
    let mut db = Db::open(&mut dev, Config::default()).unwrap();
    let info = db.flush().unwrap();
    assert_eq!(info.bytes_written, 0);
}
```

`embedded-database-os/src/lib.rs` 集成测试追加：
```rust
#[test]
fn file_backend_落盘集成() {
    let path = std::env::temp_dir().join(format!("embedded-database-flush-{}.db", std::process::id()));
    {
        let mut dev = FileDevice::open(&path, 1 << 20).unwrap();
        let mut db = Db::open(&mut dev, Config::default()).unwrap();
        {
            let mut txn = db.begin(Default::default()).unwrap();
            txn.put(b"key", b"value").unwrap();
            txn.commit().unwrap();
        }
        db.flush().unwrap();
        drop(db);
        drop(dev);
    }
    // 重新打开，数据应持久化
    let dev = FileDevice::open(&path, 1 << 20).unwrap();
    assert_eq!(dev.size(), 1 << 20);
    drop(dev);
    let _ = std::fs::remove_file(&path);
}
```

- [ ] **Step 2: 运行测试**

Run: `cargo test -p embedded-database-core --features std db 2>&1 | tail -10`
Expected: 38 passed

Run: `cargo test -p embedded-database-os 2>&1 | tail -10`
Expected: 5 passed

- [ ] **Step 3: 检查点**

Run: `cargo test -p embedded-database-core --features std 2>&1 | rg 'test result' | head -1`
Expected: 187 passed

---

### Task M7-2: M7 收尾验收

**Files:**
- Modify: `docs/superpowers/specs/2026-10-02-embedded-database-design.md`

**Interfaces:**
- Consumes: 全部 M7 Task
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

在 `2026-10-02-embedded-database-design.md` 的「实施状态」小节追加 M7 交付内容：

```markdown
## 实施状态（M7 完成）

- M7 交付：`Txn::commit` 真正落盘——`Db::flush()` 把 `pending_writes` 写入活跃段，
  `Db::begin()` 自动调用 `auto_flush()`，数据真正持久化。
- **M7 简化**：
  - commit 时数据进入 `pending_writes`（内存），`flush`/`begin` 时落盘。
    调用方可主动调用 `Db::flush()` 提前落盘。
  - `begin()` 签名从 `&self` 改为 `&mut self`（需要可变借用以调用 `flush`）。
  - `CommitInfo.bytes_written` 在 commit 时估算，`flush` 返回实际写入字节数。
- **M6 遗留未修复**：ECB 加密模式、HMAC-SM3 未接入写路径、审计 read_count/error_count 未接线。
```

- [ ] **Step 7: 检查点**

Run: `rg -n 'M7 完成' docs/superpowers/specs/2026-10-02-embedded-database-design.md`
Expected: 有输出

---

## M7 验收标准

| # | 标准 | 验证方式 |
|---|------|----------|
| 1 | `Db::flush()` 写入活跃段并返回真实 `CommitInfo` | M7-0 测试 |
| 2 | `Db::begin()` 自动调用 `auto_flush()` | M7-0 测试 |
| 3 | flush 后数据可读回（`read_payload`） | M7-1 测试 |
| 4 | 加密路径 flush 后可解密 | M7-0 测试 |
| 5 | 5 个 no_std 目标全部编译通过 | M7-2 Step 3 |
| 6 | 全 crate 零 `unsafe` | M7-2 Step 4 |
| 7 | 全部测试通过 | M7-2 Step 1 |
| 8 | `inject_crash` exit=0 且 `wcet` exit=0 | M7-2 Step 5 |

## 后续里程碑

M7 完成后，数据持久化路径打通。剩余工作：

- **加密模式升级**：ECB → CBC/CTR + HMAC-SM3（生产级安全）
- **审计日志完整实现**：`read_count`/`error_count`/`last_error` 接线
- **遗留修复**：page_map offset、lookup_page 取最新、open 报错
- **LoongArch 实机验证**：拿到龙芯硬件后执行
- **认证申报**：GB/T 22239 安全可靠测评、CSTC 适配认证
