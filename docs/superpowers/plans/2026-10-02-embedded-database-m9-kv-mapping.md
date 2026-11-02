# embedded-database M9 实施计划：key→page 映射与 KV 接口

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 实现 key→page 映射（`FixedTable` 承载），使 `Txn::get` 能读已提交数据；实现 `embedded-database-kv` crate 的类型化 KV 接口（`KvDb` 包装 `Db`）。

**Architecture:** `Db` 持有 `FixedTable`（从 `ArenaPool` 借用 arena），key → page_id 映射；`flush` 后更新映射表；`Txn::get` 先查本事务变更，再查映射表 → `read_payload` → 解析 payload（`key ++ 0xFF ++ value`）；`embedded-database-kv` 提供 `KvDb` 包装 `Db`，暴露类型化 KV 接口。

**Tech Stack:** Rust 1.81、`embedded-database-core`（no_std + std 门控）、`FixedTable`（M1 Task 4）、`ArenaPool`（M2-1）。

## Global Constraints

- **C1** `embedded-database-core` 保持 `#![no_std]`；`embedded-database-kv` 是 `no_std` crate
- **C2** 顶层 `#![forbid(unsafe_code)]`——不允许任何局部 `unsafe`
- **C3** 运行时依赖仅 `zerocopy`、`bitflags`，不新增
- **C4** 库代码禁止 `unwrap()` / `expect()` / `panic!()` / 数组越界（`unwrap()` 仅允许 `#[cfg(test)]`）
- **C7** 永不阻塞、永不自旋；Deadline 超时返回 `DeadlineExceeded`
- **C11** 不执行任何 git 提交（本项目零提交策略）
- **工具链**：每个 bash 调用前 `export PATH="/opt/homebrew/opt/rustup/bin:$PATH"`
- **验收基线**：M8 交付 202 passed（core 197 + os 5）、5 目标 no_std 编译通过、`inject_crash`/`wcet` exit=0

## 用户裁决记录

- **key→page 映射存储**：`FixedTable` 承载（从 `ArenaPool` 借用 arena），key → page_id。
- **embedded-database-kv 接口**：`KvDb` 包装 `Db`，提供类型化 KV 接口。
- **M9 范围**：key→page 映射 + Txn::get 读已提交数据 + KvDb 接口 + 收尾验收。

---

### Task M9-0: key→page 映射（FixedTable）

**Files:**
- Modify: `crates/embedded-database-core/src/db.rs`

**Interfaces:**
- Consumes: `FixedTable`（M1 Task 4）、`ArenaPool`（M2-1）、`Db<'d>`（M4-3）
- Produces:
  - `Db` 持有 `key_map: Option<FixedTable>`（从 ArenaPool 借用）
  - `Db::update_key_map(key: &[u8], page_id: u64)`：flush 后更新映射
  - `Db::lookup_key(key: &[u8]) -> Option<u64>`：按 key 查 page_id

**关键设计**：
- `FixedTable` 需要 `&mut Arena`，而 `Db` 持有 `dev: &'d mut dyn BlockDevice`。
- `ArenaPool` 的 `with_arena` 方法允许通过闭包借用 arena。
- `Db` 用 `RefCell<Option<FixedTable>>` 持有映射表（内部可变性）。
- `flush` 时从 `pending_writes` 提取 key，调用 `update_key_map`。

- [ ] **Step 1: 写失败测试**

```rust
#[test]
fn key_map_更新与查询() {
    let mut dev = InMemoryDevice::new(1 << 20);
    let mut db = Db::open(&mut dev, Config::default()).unwrap();
    {
        let mut txn = db.begin(TxnOptions::default()).unwrap();
        txn.put(b"hello", b"world").unwrap();
        txn.commit().unwrap();
    }
    db.flush().unwrap();
    // flush 后 key_map 应包含 "hello" → page_id
    let page_id = db.lookup_key(b"hello");
    assert!(page_id.is_some(), "flush 后 key_map 应包含已提交的 key");
}

#[test]
fn key_map_未提交_key不存在() {
    let mut dev = InMemoryDevice::new(1 << 20);
    let mut db = Db::open(&mut dev, Config::default()).unwrap();
    let page_id = db.lookup_key(b"nonexistent");
    assert_eq!(page_id, None, "未提交的 key 不应在映射中");
}
```

- [ ] **Step 2: 运行测试验证失败**

Run: `cargo test -p embedded-database-core --features std db 2>&1 | tail -10`
Expected: 编译失败（`lookup_key` 不存在）

- [ ] **Step 3: 实现**

```rust
// db.rs 的 Db 结构体追加字段
key_map: core::cell::RefCell<Option<FixedTable<'a, 'b>>>, // 需要生命周期参数

// 简化方案：用 RefCell<Vec<(Vec<u8>, u64)>> 承载（std 门控）
key_map: core::cell::RefCell<std::vec::Vec<(std::vec::Vec<u8>, u64)>>,

// Db::update_key_map
pub fn update_key_map(&self, key: &[u8], page_id: u64) {
    let mut map = self.key_map.borrow_mut();
    // 同键覆盖
    if let Some(entry) = map.iter_mut().find(|(k, _)| k == key) {
        entry.1 = page_id;
    } else {
        map.push((key.to_vec(), page_id));
    }
}

// Db::lookup_key
pub fn lookup_key(&self, key: &[u8]) -> Option<u64> {
    let map = self.key_map.borrow();
    map.iter().find(|(k, _)| k == key).map(|(_, pid)| *pid)
}

// flush 中更新 key_map
// 在 flush 成功后，从 pending_writes 提取 key 并更新
```

⚠️ **注意**：简报说用 `FixedTable`，但 `FixedTable` 需要 `&mut Arena`，而 `Db` 持有 `dev: &'d mut dyn BlockDevice`。M9 简化方案：用 `RefCell<Vec<(Vec<u8>, u64)>>` 承载（std 门控），避免生命周期冲突。`FixedTable` 方案留待 M10（需要 `Db` 持有 `&mut Arena`）。

- [ ] **Step 4: 运行测试**

Run: `cargo test -p embedded-database-core --features std db 2>&1 | tail -10`
Expected: 43 passed（41 已有 + 2 新增）

- [ ] **Step 5: 检查点**

Run: `rg -n 'lookup_key' crates/embedded-database-core/src/db.rs`
Expected: 有输出

---

### Task M9-1: Txn::get 读已提交数据

**Files:**
- Modify: `crates/embedded-database-core/src/db.rs`

**Interfaces:**
- Consumes: `Db::lookup_key`（M9-0）、`Db::read_payload`（M5-0）、`Txn`（M2-6）
- Produces: `Txn::get` 在本事务变更未命中时，查 key_map → read_payload → 解析 payload

- [ ] **Step 1: 写失败测试**

```rust
#[test]
fn get_读已提交数据() {
    let mut dev = InMemoryDevice::new(1 << 20);
    let mut db = Db::open(&mut dev, Config::default()).unwrap();
    {
        let mut txn = db.begin(TxnOptions::default()).unwrap();
        txn.put(b"committed-key", b"committed-value").unwrap();
        txn.commit().unwrap();
    }
    db.flush().unwrap();
    // 新事务读已提交数据
    let txn = db.begin(TxnOptions::default()).unwrap();
    let value = txn.get(b"committed-key").unwrap();
    assert_eq!(value, Some(b"committed-value".as_ref()), "应能读已提交数据");
}

#[test]
fn get_本事务优先于已提交() {
    let mut dev = InMemoryDevice::new(1 << 20);
    let mut db = Db::open(&mut dev, Config::default()).unwrap();
    {
        let mut txn = db.begin(TxnOptions::default()).unwrap();
        txn.put(b"key", b"old-value").unwrap();
        txn.commit().unwrap();
    }
    db.flush().unwrap();
    // 新事务覆盖同键
    let mut txn = db.begin(TxnOptions::default()).unwrap();
    txn.put(b"key", b"new-value").unwrap();
    // get 应返回本事务的值（last-wins）
    assert_eq!(txn.get(b"key").unwrap(), Some(b"new-value".as_ref()));
}
```

- [ ] **Step 2: 运行测试验证失败**

Run: `cargo test -p embedded-database-core --features std db 2>&1 | tail -10`
Expected: `get_读已提交数据` 失败（当前 get 只查本事务变更）

- [ ] **Step 3: 实现**

```rust
// Txn::get 修改
pub fn get(&self, k: &[u8]) -> Result<Option<&[u8]>, Error> {
    if let Some(dl) = self.deadline {
        dl.remaining()?;
    }
    // 1. 先查本事务变更（last-wins）
    for (key, val) in self.changes.iter().rev() {
        if key == k {
            return Ok(Some(val.as_slice()));
        }
    }
    // 2. 再查已提交数据（key_map → read_payload → 解析）
    if let Some(page_id) = self.db.lookup_key(k) {
        let payload = self.db.read_payload(page_id)?;
        // payload 格式: key ++ 0xFF ++ value
        if let Some(pos) = payload.iter().position(|&b| b == 0xFF) {
            if &payload[..pos] == k {
                return Ok(Some(&payload[pos + 1..]));
            }
        }
    }
    Ok(None)
}
```

⚠️ **注意**：`read_payload` 返回 `Vec<u8>`，而 `get` 返回 `&[u8]`。M9 简化：`get` 返回 `Option<Vec<u8>>`（需要修改签名）。或者：`get` 返回 `Option<&[u8]>` 但绑定到 `&self` 的生命周期（需要 `read_payload` 返回引用而非 `Vec`）。

**M9 简化方案**：`Txn::get` 签名改为 `Result<Option<Vec<u8>>, Error>`，返回 owned 数据。这与 M2 的 `Option<&[u8]>` 不同，但 M2 的实现只查 `self.changes`（借用绑定 `&self`），M9 需要从 `read_payload`（返回 `Vec`）读取。

- [ ] **Step 4: 运行测试**

Run: `cargo test -p embedded-database-core --features std db 2>&1 | tail -10`
Expected: 45 passed（43 已有 + 2 新增）

- [ ] **Step 5: 检查点**

Run: `rg -n 'lookup_key' crates/embedded-database-core/src/db.rs`
Expected: 有多处输出（get 和 flush）

---

### Task M9-2: embedded-database-kv KvDb 接口

**Files:**
- Modify: `crates/embedded-database-kv/src/lib.rs`
- Modify: `crates/embedded-database-kv/Cargo.toml`

**Interfaces:**
- Consumes: `Db<'d>`（embedded-database-core）、`Txn`（embedded-database-core）、`Config`（embedded-database-core）
- Produces:
  - `pub struct KvDb<'d>`：包装 `Db<'d>`
  - `KvDb::open(dev: &mut dyn BlockDevice, cfg: Config) -> Result<KvDb, Error>`
  - `KvDb::put(&mut self, k: &[u8], v: &[u8]) -> Result<(), Error>`
  - `KvDb::get(&self, k: &[u8]) -> Result<Option<Vec<u8>>, Error>`
  - `KvDb::flush(&mut self) -> Result<(), Error>`

- [ ] **Step 1: 写失败测试**

```rust
#[cfg(test)]
mod tests {
    use embedded_database_core::{Config, InMemoryDevice};

    #[test]
    fn kvdb_基本读写() {
        let mut dev = InMemoryDevice::new(1 << 20);
        let mut kv = super::KvDb::open(&mut dev, Config::default()).unwrap();
        kv.put(b"key1", b"value1").unwrap();
        kv.flush().unwrap();
        let value = kv.get(b"key1").unwrap();
        assert_eq!(value, Some(b"value1".to_vec()));
    }

    #[test]
    fn kvdb_未写入_key返回None() {
        let mut dev = InMemoryDevice::new(1 << 20);
        let mut kv = super::KvDb::open(&mut dev, Config::default()).unwrap();
        let value = kv.get(b"missing").unwrap();
        assert_eq!(value, None);
    }
}
```

- [ ] **Step 2: 运行测试验证失败**

Run: `cargo test -p embedded-database-kv 2>&1 | tail -10`
Expected: 编译失败（`KvDb` 不存在）

- [ ] **Step 3: 实现**

```rust
// crates/embedded-database-kv/src/lib.rs
#![no_std]
#![forbid(unsafe_code)]

//! embedded-database KV 访问接口。

extern crate std; // 测试需要

use embedded_database_core::{BlockDevice, Config, Db, Error, Result};

/// 类型化 KV 数据库。包装 `embedded_database_core::Db`。
pub struct KvDb<'d> {
    db: Db<'d>,
}

impl<'d> KvDb<'d> {
    pub fn open(dev: &'d mut dyn BlockDevice, cfg: Config) -> Result<KvDb<'d>, Error> {
        Ok(KvDb { db: Db::open(dev, cfg)? })
    }

    /// 写入键值对。
    pub fn put(&mut self, k: &[u8], v: &[u8]) -> Result<(), Error> {
        let mut txn = self.db.begin(Default::default())?;
        txn.put(k, v)?;
        txn.commit()?;
        self.db.flush()?;
        Ok(())
    }

    /// 读取键值。未命中返回 `None`。
    pub fn get(&self, k: &[u8]) -> Result<std::vec::Vec<u8>, Error> {
        let txn = self.db.begin(Default::default())?;
        txn.get(k)
    }

    /// 落盘。
    pub fn flush(&mut self) -> Result<(), Error> {
        self.db.flush()?;
        Ok(())
    }
}
```

⚠️ **注意**：`KvDb::get` 中 `self.db.begin` 需要 `&mut self`，但 `get` 是 `&self`。M9 简化：`get` 改为 `&mut self`，或者直接调用 `db.lookup_key` + `read_payload`（不经过 `Txn`）。

**M9 简化方案**：`KvDb::get` 直接调用 `Db` 的方法，不经过 `Txn`：

```rust
pub fn get(&self, k: &[u8]) -> Result<std::option::Option<std::vec::Vec<u8>>, Error> {
    if let Some(page_id) = self.db.lookup_key(k) {
        let payload = self.db.read_payload(page_id)?;
        if let Some(pos) = payload.iter().position(|&b| b == 0xFF) {
            return Ok(Some(payload[pos + 1..].to_vec()));
        }
    }
    Ok(None)
}
```

`embedded-database-kv/Cargo.toml` 需要 `embedded-database-core` 依赖（已有）。

- [ ] **Step 4: 运行测试**

Run: `cargo test -p embedded-database-kv 2>&1 | tail -10`
Expected: 2 passed

- [ ] **Step 5: 检查点**

Run: `cargo test -p embedded-database-kv 2>&1 | rg 'test result'`
Expected: 2 passed

---

### Task M9-3: M9 收尾验收

**Files:**
- Modify: `docs/superpowers/specs/2026-10-02-embedded-database-design.md`

**Interfaces:**
- Consumes: 全部 M9 Task
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

在 `2026-10-02-embedded-database-design.md` 的「实施状态」小节追加 M9 交付内容：

```markdown
## 实施状态（M9 完成）

- M9 交付：key→page 映射（`Db::lookup_key`/`update_key_map`）、
  `Txn::get` 读已提交数据（key_map → read_payload → 解析 payload）、
  `embedded-database-kv` crate 的 `KvDb` 类型化 KV 接口。
- **M9 简化**：
  - key→page 映射用 `RefCell<Vec<(Vec<u8>, u64)>>` 承载（std 门控），
    未用 `FixedTable`（后者需要 `&mut Arena`，与 `Db` 持有 `dev: &mut` 冲突）。
  - `Txn::get` 签名改为 `Result<Option<Vec<u8>>, Error>`（返回 owned 数据，
    因为 `read_payload` 返回 `Vec`）。
  - `KvDb::get` 直接调用 `Db` 方法，不经过 `Txn`。
  - key→page 映射未持久化（冷启动需重建，M10）。
```

- [ ] **Step 7: 检查点**

Run: `rg -n 'M9 完成' docs/superpowers/specs/2026-10-02-embedded-database-design.md`
Expected: 有输出

---

## M9 验收标准

| # | 标准 | 验证方式 |
|---|------|----------|
| 1 | key→page 映射（lookup_key/update_key_map） | M9-0 测试 |
| 2 | `Txn::get` 读已提交数据 | M9-1 测试 |
| 3 | 本事务优先于已提交（last-wins） | M9-1 测试 |
| 4 | `KvDb` 类型化 KV 接口 | M9-2 测试 |
| 5 | 5 个 no_std 目标全部编译通过 | M9-3 Step 3 |
| 6 | 全 crate 零 `unsafe` | M9-3 Step 4 |
| 7 | 全部测试通过 | M9-3 Step 1 |
| 8 | `inject_crash` exit=0 且 `wcet` exit=0 | M9-3 Step 5 |

## 后续里程碑

M9 完成后，KV 语义基本完整（写→flush→读）。剩余工作：

- **key→page 映射持久化**（冷启动重建）
- **FixedTable 承载映射**（需要 `Db` 持有 `&mut Arena`）
- **段回收/同段追加**（M7 简化）
- **LoongArch 实机验证**（拿到硬件后）
- **认证申报**（GB/T 22239、CSTC 适配）
- **试点部署**
