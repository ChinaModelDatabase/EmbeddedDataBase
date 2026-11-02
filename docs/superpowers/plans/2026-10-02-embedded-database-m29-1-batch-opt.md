# embedded-database M29-1 实施计划：批量插入优化

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 优化批量插入吞吐：`KvDb::put_batch` 方法 + `flush` 中 `take()` 替代 `clone()`。

**Architecture:** `KvDb::put_batch` 累积多条变更到 `pending_writes`，一次 `flush` 落盘；`flush` 中用 `std::mem::take()` 替代 `clone()` 避免拷贝 pending 数据。

**Tech Stack:** Rust 1.81、`embedded-database-kv`（std 门控）、`embedded-database-core`。

## Global Constraints

- **C1** `embedded-database-core` 保持 `#![no_std]`
- **C2** 顶层 `#![forbid(unsafe_code)]`——不允许任何局部 `unsafe`
- **C3** 运行时依赖仅 `zerocopy`、`bitflags`，不新增
- **C4** 库代码禁止 `unwrap()` / `expect()` / `panic!()` / 数组越界（`unwrap()` 仅允许 `#[cfg(test)]`；bench bin 可用 `unwrap`）
- **C11** 不执行任何 git 提交（本项目零提交策略）
- **工具链**：每个 bash 调用前 `export PATH="/opt/homebrew/opt/rustup/bin:$PATH"`
- **验收基线**：M29-0 交付 288 passed、`scale_test` 50K 条 exit=0（222 条/秒）

## 用户裁决记录

- **M29-1 范围**：批量插入优化（`put_batch` + `flush` take）。

---

### Task M29-1-0: flush 中 take() 替代 clone()

**Files:**
- Modify: `crates/embedded-database-core/src/db.rs`

**Interfaces:**
- Consumes: `Db::flush`（M7-0/M12）
- Produces: `flush` 中 `std::mem::take()` 替代 `pending_writes.borrow().clone()`

- [ ] **Step 1: 写失败测试**

```rust
#[test]
fn flush_不修改pending_writes语义() {
    let mut dev = InMemoryDevice::new(1 << 20);
    let mut db = Db::open(&mut dev, Config::default()).unwrap();
    {
        let mut txn = db.begin(TxnOptions::default()).unwrap();
        txn.put(b"k", b"v").unwrap();
        txn.commit().unwrap();
    }
    assert!(db.pending_writes_len() > 0);
    db.flush().unwrap();
    assert_eq!(db.pending_writes_len(), 0);
}
```

- [ ] **Step 2: 运行测试验证失败**

Run: `cargo test -p embedded-database-core --features std db 2>&1 | tail -10`
Expected: 测试失败（当前 flush 用 clone，take 后语义应一致）

- [ ] **Step 3: 实现**

在 `flush` 中将 `self.pending_writes.borrow().clone()` 改为 `std::mem::take(&mut *self.pending_writes.borrow_mut())`。

- [ ] **Step 4: 运行测试**

Run: `cargo test -p embedded-database-core --features std db 2>&1 | tail -10`
Expected: 50 passed

---

### Task M29-1-1: KvDb::put_batch

**Files:**
- Modify: `crates/embedded-database-kv/src/lib.rs`

**Interfaces:**
- Consumes: `Db`（M4-3）、`Txn`（M2-6）
- Produces:
  - `KvDb::put_batch(&mut self, items: &[( &[u8], &[u8] )]) -> Result<()>`

- [ ] **Step 1: 写失败测试**

```rust
#[test]
fn kvdb_put_batch_批量写入() {
    let mut dev = InMemoryDevice::new(1 << 20);
    let mut kv = KvDb::open(&mut dev, Config::default()).unwrap();
    let items = [
        (b"k1".as_ref(), b"v1".as_ref()),
        (b"k2".as_ref(), b"v2".as_ref()),
        (b"k3".as_ref(), b"v3".as_ref()),
    ];
    kv.put_batch(&items).unwrap();
    kv.flush().unwrap();
    assert_eq!(kv.get(b"k1").unwrap(), Some(b"v1".to_vec()));
    assert_eq!(kv.get(b"k2").unwrap(), Some(b"v2".to_vec()));
    assert_eq!(kv.get(b"k3").unwrap(), Some(b"v3".to_vec()));
}

#[test]
fn kvdb_put_batch_含0xff被拒() {
    let mut dev = InMemoryDevice::new(1 << 20);
    let mut kv = KvDb::open(&mut dev, Config::default()).unwrap();
    let items = [(b"ok".as_ref(), b"v".as_ref()), ([0x01, 0xFF, 0x02].as_ref(), b"v".as_ref())];
    assert!(kv.put_batch(&items).is_err());
}
```

- [ ] **Step 2: 运行测试验证失败**

Run: `cargo test -p embedded-database-kv kvdb_put_batch 2>&1 | tail -10`
Expected: 编译失败（`put_batch` 不存在）

- [ ] **Step 3: 实现**

```rust
/// 批量写入。累积多条变更到 pending_writes，一次 flush 落盘。
pub fn put_batch(&mut self, items: &[( &[u8], &[u8] )]) -> Result<()> {
    for (k, v) in items {
        if k.contains(&0xFF) {
            return Err(Error::InvalidArgument);
        }
    }
    let mut txn = self.db.begin(Default::default())?;
    for (k, v) in items {
        txn.put(k, v)?;
    }
    txn.commit()?;
    self.db.flush()?;
    Ok(())
}
```

- [ ] **Step 4: 运行测试**

Run: `cargo test -p embedded-database-kv kvdb_put_batch 2>&1 | tail -10`
Expected: 2 passed

- [ ] **Step 5: 检查点**

Run: `cargo test -p embedded-database-kv 2>&1 | rg 'test result:' | head -1`
Expected: 8 passed

---

### Task M29-1-2: M29-1 收尾验收

**Files:**
- Modify: `crates/embedded-database-bench/src/bin/scale_test.rs`
- Modify: `docs/superpowers/specs/2026-10-02-embedded-database-design.md`

**Interfaces:**
- Consumes: `KvDb::put_batch`（M29-1-1）
- Produces: `scale_test.rs` 使用 `put_batch`，设计文档更新

- [ ] **Step 1: 修改 scale_test.rs 使用 put_batch**

将写入循环改为批量模式（每批 10000 条）：

```rust
let batch_size = 10000;
for batch_start in (0..RECORD_COUNT).step_by(batch_size) {
    let batch_end = (batch_start + batch_size).min(RECORD_COUNT);
    let items: Vec<(&[u8], &[u8])> = (batch_start..batch_end)
        .map(|i| {
            let key = format!("key{i:010}");
            let value = format!("value{i:010}");
            (key.into_bytes().leak(), value.into_bytes().leak())
        })
        .collect();
    kv.put_batch(&items).expect("批量写入失败");
}
```

- [ ] **Step 2: 运行验证**

Run: `cargo run --release -p embedded-database-bench --bin scale_test 2>&1 | tail -30`
Expected: 全部通过，退出码 0

- [ ] **Step 3: 更新设计文档**

在 `2026-10-02-embedded-database-design.md` 的「实施状态」小节追加 M29-1 交付内容：

```markdown
## 实施状态（M29-1 完成）

- M29-1 交付：批量插入优化——`KvDb::put_batch` 方法、
  `flush` 中 `std::mem::take()` 替代 `clone()`。
- **M29-1 简化**：
  - `put_batch` 内部仍走 `begin→put→commit→flush` 路径（批量语义在 `Db` 层）。
  - `take()` 替代 `clone()` 减少 flush 时的内存拷贝。
```

- [ ] **Step 4: 检查点**

Run: `rg -n 'M29-1 完成' docs/superpowers/specs/2026-10-02-embedded-database-design.md`
Expected: 有输出

---

## M29-1 验收标准

| # | 标准 | 验证方式 |
|---|------|----------|
| 1 | `flush` 用 `take()` 替代 `clone()` | M29-1-0 测试 |
| 2 | `KvDb::put_batch` 批量写入 | M29-1-1 测试 |
| 3 | `scale_test` 使用 `put_batch` | M29-1-2 Step 2 |
| 4 | 全部测试通过 | M29-1-2 Step 2 |
| 5 | 设计文档含「实施状态（M29-1 完成）」小节 | M29-1-2 Step 4 |

## 后续里程碑

M29-1 完成后，批量插入优化交付。后续：

- **M30**：亿级规模验证（10000 万条记录）
- **M31**：十亿级规模验证（100000 万条记录）
- **M32**：LoongArch 实机验证（需硬件）
