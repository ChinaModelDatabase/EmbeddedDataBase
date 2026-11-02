# embedded-database M10 实施计划：key_map 冷启动重建

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `Db::open` 冷启动时从已提交段重建 `key_map`，使重开后 `lookup_key`/`Txn::get` 能读已提交数据。

**Architecture:** `Db::open` 在重建 `page_map` 后，遍历各段页数据，用 `dev.read_at` 读取每页 payload，解析 `key ++ 0xFF ++ value` 格式，调用 `update_key_map` 重建映射。加密路径需先解密。

**Tech Stack:** Rust 1.81、`embedded-database-core`（no_std + std 门控）、`decrypt_page`（M8-1）。

## Global Constraints

- **C1** `embedded-database-core` 保持 `#![no_std]`
- **C2** 顶层 `#![forbid(unsafe_code)]`——不允许任何局部 `unsafe`
- **C3** 运行时依赖仅 `zerocopy`、`bitflags`，不新增
- **C4** 库代码禁止 `unwrap()` / `expect()` / `panic!()` / 数组越界（`unwrap()` 仅允许 `#[cfg(test)]`）
- **C7** 永不阻塞、永不自旋；Deadline 超时返回 `DeadlineExceeded`
- **C11** 不执行任何 git 提交（本项目零提交策略）
- **工具链**：每个 bash 调用前 `export PATH="/opt/homebrew/opt/rustup/bin:$PATH"`
- **验收基线**：M9 交付 210 passed（core 202 + kv 3 + os 5）、5 目标 no_std 编译通过、`inject_crash`/`wcet` exit=0

## 用户裁决记录

- **M10 范围**：key_map 冷启动重建 + 测试 + 收尾验收。

---

### Task M10-0: key_map 冷启动重建

**Files:**
- Modify: `crates/embedded-database-core/src/db.rs`

**Interfaces:**
- Consumes: `Db<'d>`（M4-3，持有 `dev: &'d mut dyn BlockDevice`）、`page_map: Vec<(u64, u64, u64)>`（page_id, segment_seq, offset）、`decrypt_page`（M8-1）、`update_key_map`（M9-0）
- Produces: `Db::open` 重建 `key_map`

**关键设计**：
- `open` 中 `page_map` 重建后，遍历 `page_map` 每个条目。
- 对每个 `(page_id, segment_seq, offset)`，用 `dev.read_at(offset, ...)` 读取 `PageHeader` + payload。
- 若 `cfg.encryption_key` 为 `Some`，先 `decrypt_page`。
- 解析 payload（`key ++ 0xFF ++ value`），提取 key，调用 `update_key_map(key, page_id)`。
- key 含 `0xFF` 时跳过（M9-1 已在 `put` 拒绝，冷启动扫描到旧数据可能含 `0xFF`）。

- [ ] **Step 1: 写失败测试**

```rust
#[test]
fn 冷启动重建_key_map() {
    let mut dev = InMemoryDevice::new(1 << 20);
    // 写数据并 flush
    {
        let mut db = Db::open(&mut dev, Config::default()).unwrap();
        let mut txn = db.begin(TxnOptions::default()).unwrap();
        txn.put(b"persist-key", b"persist-value").unwrap();
        txn.commit().unwrap();
        db.flush().unwrap();
    }
    // 重开，key_map 应重建
    let db = Db::open(&mut dev, Config::default()).unwrap();
    let page_id = db.lookup_key(b"persist-key");
    assert!(page_id.is_some(), "冷启动后 key_map 应包含已提交的 key");
}

#[test]
fn 冷启动重建_重开后get() {
    let mut dev = InMemoryDevice::new(1 << 20);
    {
        let mut db = Db::open(&mut dev, Config::default()).unwrap();
        let mut txn = db.begin(TxnOptions::default()).unwrap();
        txn.put(b"hello", b"world").unwrap();
        txn.commit().unwrap();
        db.flush().unwrap();
    }
    // 重开后 get 应读已提交数据
    let mut db = Db::open(&mut dev, Config::default()).unwrap();
    let txn = db.begin(TxnOptions::default()).unwrap();
    let value = txn.get(b"hello").unwrap();
    assert_eq!(value, Some(b"world".to_vec()), "重开后应能读已提交数据");
}

#[test]
fn 冷启动重建_加密路径() {
    let mut dev = InMemoryDevice::new(1 << 20);
    let key = EncryptionKey {
        sm4: crate::sm4::Sm4Key { bytes: [0x01; 16] },
        hmac: [0x02; 32],
    };
    let cfg = Config {
        encryption_key: Some(key),
        ..Config::default()
    };
    {
        let mut db = Db::open(&mut dev, cfg.clone()).unwrap();
        let mut txn = db.begin(TxnOptions::default()).unwrap();
        txn.put(b"enc-key", b"enc-value").unwrap();
        txn.commit().unwrap();
        db.flush().unwrap();
    }
    // 重开（带相同密钥），key_map 应重建
    let db = Db::open(&mut dev, cfg).unwrap();
    let page_id = db.lookup_key(b"enc-key");
    assert!(page_id.is_some(), "加密路径冷启动后 key_map 应重建");
}
```

- [ ] **Step 2: 运行测试验证失败**

Run: `cargo test -p embedded-database-core --features std db 2>&1 | tail -10`
Expected: `冷启动重建_key_map` 失败（当前 key_map 不参与冷启动重建）

- [ ] **Step 3: 实现**

在 `Db::open` 的 `page_map` 重建后、`Db` 构造前，追加 key_map 重建逻辑：

```rust
// 冷启动重建 key_map：遍历 page_map，读取每页 payload，解析 key。
let key_map = if cfg.encryption_key.is_some() || !page_map.is_empty() {
    let mut km: std::vec::Vec<(std::vec::Vec<u8>, u64)> = std::vec::Vec::new();
    for (page_id, segment_seq, offset) in &page_map {
        // 读页头
        let mut header_buf = [0u8; PAGE_HEADER_SIZE];
        if dev.read_at(*offset, &mut header_buf).is_err() {
            continue;
        }
        let header = match PageHeader::decode(&header_buf) {
            Ok(h) => h,
            Err(_) => continue,
        };
        if header.page_id != *page_id {
            continue;
        }
        // 读 payload
        let payload_len = header.payload_len as usize;
        let payload_off = match offset.checked_add(PAGE_HEADER_SIZE as u64) {
            Some(v) => v,
            None => continue,
        };
        let mut payload = std::vec![0u8; payload_len];
        if dev.read_at(payload_off, &mut payload).is_err() {
            continue;
        }
        // 解密（若有密钥）
        let payload = if let Some(k) = &cfg.encryption_key {
            match crate::crypto::decrypt_page(k, *page_id, &payload) {
                Ok(p) => p,
                Err(_) => continue,
            }
        } else {
            payload
        };
        // 解析 key ++ 0xFF ++ value
        if let Some(pos) = payload.iter().position(|&b| b == 0xFF) {
            let key = &payload[..pos];
            // key 含 0xFF 时跳过（M9-1 已在 put 拒绝，旧数据可能含）
            if !key.contains(&0xFF) {
                // 同键覆盖（后写胜出，按 segment_seq 降序遍历可保证）
                if let Some(entry) = km.iter_mut().find(|(k, _)| k.as_slice() == key) {
                    entry.1 = *page_id;
                } else {
                    km.push((key.to_vec(), *page_id));
                }
            }
        }
    }
    km
} else {
    std::vec::Vec::new()
};
```

在 `Db` 构造中用 `key_map` 替换 `core::cell::RefCell::new(std::vec::Vec::new())`。

⚠️ **注意**：`page_map` 中同一 page_id 可能出现多次（多段）。重建时应按 `segment_seq` 降序遍历，确保后写胜出（最新段的 key 覆盖旧段）。或者：先按 `segment_seq` 升序遍历，后写自然覆盖。

- [ ] **Step 4: 运行测试**

Run: `cargo test -p embedded-database-core --features std db 2>&1 | tail -10`
Expected: 48 passed（45 已有 + 3 新增）

Run: `cargo test -p embedded-database-core --features std 2>&1 | rg 'test result' | head -1`
Expected: 213 passed

- [ ] **Step 5: 检查点**

Run: `rg -n 'key_map' crates/embedded-database-core/src/db.rs | head -5`
Expected: 有多处输出（open 中重建、lookup_key、update_key_map）

---

### Task M10-1: M10 收尾验收

**Files:**
- Modify: `docs/superpowers/specs/2026-10-02-embedded-database-design.md`

**Interfaces:**
- Consumes: 全部 M10 Task
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

在 `2026-10-02-embedded-database-design.md` 的「实施状态」小节追加 M10 交付内容：

```markdown
## 实施状态（M10 完成）

- M10 交付：`Db::open` 冷启动重建 `key_map`（扫描已提交段页数据，解析 payload，
  提取 key→page_id）。重开后 `lookup_key`/`Txn::get` 能读已提交数据。
- **M10 简化**：
  - key_map 用 `RefCell<Vec<(Vec<u8>, u64)>>` 承载（std 门控）。
  - 冷启动扫描按 `segment_seq` 升序遍历，后写自然覆盖。
  - key 含 `0xFF` 时跳过（M9-1 已在 put 拒绝，旧数据可能含）。
  - 加密路径先 `decrypt_page` 再解析 payload。
```

- [ ] **Step 7: 检查点**

Run: `rg -n 'M10 完成' docs/superpowers/specs/2026-10-02-embedded-database-design.md`
Expected: 有输出

---

## M10 验收标准

| # | 标准 | 验证方式 |
|---|------|----------|
| 1 | 冷启动后 `lookup_key` 返回正确 page_id | M10-0 测试 |
| 2 | 重开后 `Txn::get` 能读已提交数据 | M10-0 测试 |
| 3 | 加密路径冷启动重建 | M10-0 测试 |
| 4 | 5 个 no_std 目标全部编译通过 | M10-1 Step 3 |
| 5 | 全 crate 零 `unsafe` | M10-1 Step 4 |
| 6 | 全部测试通过 | M10-1 Step 1 |
| 7 | `inject_crash` exit=0 且 `wcet` exit=0 | M10-1 Step 5 |

## 后续里程碑

M10 完成后，KV 数据库耐久性故事完整（写→flush→重开→读）。剩余工作：

- **段回收/同段追加**（M7 简化）
- **`FixedTable` 承载映射**（需要 `Db` 持有 `&mut Arena`）
- **LoongArch 实机验证**（拿到硬件后）
- **认证申报**（GB/T 22239、CSTC 适配）
- **试点部署**
