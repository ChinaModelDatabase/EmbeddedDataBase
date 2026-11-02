# embedded-database M8 实施计划：生产加固

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 加密升级（ECB → CBC + 每页 HMAC-SM3）、审计日志接线、遗留修复（lookup_page 取最新、open 报错、page_id 全局递增）。

**Architecture:** CBC 模式每页独立加密，IV = page_id 派生（确定性，无需存储）；每页密文尾部追加 32 字节 HMAC-SM3；审计计数在 `Txn::get`/`scan`/错误路径更新；`lookup_page` 取 `segment_seq` 最大值；`open` 段超出设备时报错；page_id 用 `Db` 内全局递增计数器。

**Tech Stack:** Rust 1.81、`embedded-database-core`（no_std + std 门控）、自研 SM4 CBC + HMAC-SM3（M6-0/M6-2）。

## Global Constraints

- **C1** `embedded-database-core` 保持 `#![no_std]`
- **C2** 顶层 `#![forbid(unsafe_code)]`——不允许任何局部 `unsafe`
- **C3** 运行时依赖仅 `zerocopy`、`bitflags`，不新增
- **C4** 库代码禁止 `unwrap()` / `expect()` / `panic!()` / 数组越界（`unwrap()` 仅允许 `#[cfg(test)]`）
- **C7** 永不阻塞、永不自旋；Deadline 超时返回 `DeadlineExceeded`
- **C11** 不执行任何 git 提交（本项目零提交策略）
- **工具链**：每个 bash 调用前 `export PATH="/opt/homebrew/opt/rustup/bin:$PATH"`
- **验收基线**：M7 交付 189 passed（core 184 + os 5）、5 目标 no_std 编译通过、`inject_crash`/`wcet` exit=0

## 用户裁决记录

- **CBC IV 来源**：IV = page_id 派生（前 8 字节 page_id，后 8 字节 0）。确定性 IV，无需存储。
- **HMAC 位置**：每页 HMAC-SM3，密文尾部追加 32 字节 HMAC 值。解密前先验证。
- **M8 范围**：加密升级 + 审计接线 + 遗留修复 + 测试 + 收尾。

---

### Task M8-0: SM4 CBC 模式

**Files:**
- Modify: `crates/embedded-database-core/src/sm4.rs`

**Interfaces:**
- Consumes: `Sm4Key`、`sm4_encrypt_block`/`sm4_decrypt_block`（M6-0）
- Produces:
  - `pub fn sm4_cbc_encrypt(key: &Sm4Key, iv: &[u8; 16], plaintext: &[u8]) -> Vec<u8>`
  - `pub fn sm4_cbc_decrypt(key: &Sm4Key, iv: &[u8; 16], ciphertext: &[u8]) -> Result<Vec<u8>, Error>`
  - `pub fn derive_iv(page_id: u64) -> [u8; 16]`

**CBC 算法要点**：
- 加密：`C[i] = SM4(P[i] ^ C[i-1])`，`C[-1] = IV`
- 解密：`P[i] = SM4_dec(C[i]) ^ C[i-1]`
- PKCS#7 填充（与 ECB 相同）

- [ ] **Step 1: 写失败测试**

```rust
// crates/embedded-database-core/src/sm4.rs 测试模块追加
#[test]
fn sm4_cbc_标准测试向量() {
    // GB/T 32907-2016 附录 B CBC 模式测试向量
    // 需查标准获取精确值；此处先验证往返
    let key = Sm4Key { bytes: [0x01; 16] };
    let iv = derive_iv(1);
    let pt = b"hello CBC mode encryption";
    let ct = sm4_cbc_encrypt(&key, &iv, pt);
    assert_ne!(&ct[..pt.len()], pt, "密文应与明文不同");
    let back = sm4_cbc_decrypt(&key, &iv, &ct).unwrap();
    assert_eq!(back, pt);
}

#[test]
fn sm4_cbc_不同IV不同密文() {
    let key = Sm4Key { bytes: [0x01; 16] };
    let pt = b"same plaintext";
    let ct1 = sm4_cbc_encrypt(&key, &derive_iv(1), pt);
    let ct2 = sm4_cbc_encrypt(&key, &derive_iv(2), pt);
    assert_ne!(ct1, ct2, "不同 IV 应产生不同密文");
}

#[test]
fn sm4_cbc_错误IV解密失败() {
    let key = Sm4Key { bytes: [0x01; 16] };
    let ct = sm4_cbc_encrypt(&key, &derive_iv(1), b"data");
    // 用错误 IV 解密应产生错误填充（概率性）
    let result = sm4_cbc_decrypt(&key, &derive_iv(99), &ct);
    // 可能成功（概率极低）或失败；关键是不 panic
    match result {
        Ok(plain) => assert_ne!(plain, b"data"),
        Err(_) => {}
    }
}

#[test]
fn derive_iv_确定性() {
    assert_eq!(derive_iv(42), derive_iv(42));
    assert_ne!(derive_iv(42), derive_iv(43));
}

#[test]
fn sm4_cbc_空明文() {
    let key = Sm4Key { bytes: [0x01; 16] };
    let iv = derive_iv(0);
    // 空明文 → PKCS#7 填充 16 字节 0x10
    let ct = sm4_cbc_encrypt(&key, &iv, b"");
    assert_eq!(ct.len(), 16, "空明文 CBC 密文应为 16 字节（填充块）");
    let back = sm4_cbc_decrypt(&key, &iv, &ct).unwrap();
    assert_eq!(back, b"");
}
```

- [ ] **Step 2: 运行测试验证失败**

Run: `cargo test -p embedded-database-core --features std sm4 2>&1 | tail -10`
Expected: 编译失败（`sm4_cbc_encrypt` 不存在）

- [ ] **Step 3: 实现 CBC 模式**

```rust
/// 从 page_id 派生 CBC IV（前 8 字节 page_id，后 8 字节 0）。
pub fn derive_iv(page_id: u64) -> [u8; 16] {
    let mut iv = [0u8; 16];
    iv[..8].copy_from_slice(&page_id.to_le_bytes());
    iv
}

/// SM4-CBC 加密。PKCS#7 填充，密文长度 = ceil((len+1)/16)*16。
#[cfg(feature = "std")]
pub fn sm4_cbc_encrypt(key: &Sm4Key, iv: &[u8; 16], plaintext: &[u8]) -> std::vec::Vec<u8> {
    let pad = 16 - (plaintext.len() % 16);
    let mut padded = std::vec::Vec::with_capacity(plaintext.len() + pad);
    padded.extend_from_slice(plaintext);
    padded.extend_from_slice(&[pad as u8; 16][..pad]);

    let mut prev = *iv;
    let mut out = std::vec::Vec::with_capacity(padded.len());
    for chunk in padded.chunks_exact(16) {
        let mut block = [0u8; 16];
        block.copy_from_slice(chunk);
        // CBC: P[i] ^ C[i-1]
        for j in 0..16 {
            block[j] ^= prev[j];
        }
        let ct = sm4_encrypt_block(key, &block);
        out.extend_from_slice(&ct);
        prev = ct;
    }
    out
}

/// SM4-CBC 解密。密文必须非空且为 16 字节倍数；填充非法返回 `InvalidArgument`。
#[cfg(feature = "std")]
pub fn sm4_cbc_decrypt(
    key: &Sm4Key,
    iv: &[u8; 16],
    ciphertext: &[u8],
) -> Result<std::vec::Vec<u8>, Error> {
    if ciphertext.is_empty() || ciphertext.len() % 16 != 0 {
        return Err(Error::InvalidArgument);
    }
    let mut prev = *iv;
    let mut out = std::vec::Vec::with_capacity(ciphertext.len());
    for chunk in ciphertext.chunks_exact(16) {
        let mut block = [0u8; 16];
        block.copy_from_slice(chunk);
        let pt = sm4_decrypt_block(key, &block);
        // CBC 解密: P[i] = SM4_dec(C[i]) ^ C[i-1]
        for j in 0..16 {
            out.push(pt[j] ^ prev[j]);
        }
        prev = block;
    }
    // PKCS#7 解填充
    let pad = *out.last().ok_or(Error::InvalidArgument)? as usize;
    if pad == 0 || pad > 16 || pad > out.len() {
        return Err(Error::InvalidArgument);
    }
    out.truncate(out.len() - pad);
    Ok(out)
}
```

- [ ] **Step 4: 运行测试**

Run: `cargo test -p embedded-database-core --features std sm4 2>&1 | tail -10`
Expected: 8 passed（3 已有 + 5 新增）

- [ ] **Step 5: 检查点**

Run: `rg -n 'sm4_cbc_encrypt' crates/embedded-database-core/src/sm4.rs`
Expected: 有输出

---

### Task M8-1: 每页 HMAC-SM3

**Files:**
- Modify: `crates/embedded-database-core/src/sm4.rs`
- Modify: `crates/embedded-database-core/src/crypto.rs`

**Interfaces:**
- Consumes: `hmac_sm3`（M6-2）、`sm4_cbc_encrypt`/`sm4_cbc_decrypt`（M8-0）
- Produces:
  - `pub fn encrypt_page(key: &EncryptionKey, page_id: u64, payload: &[u8]) -> Vec<u8>`：CBC 加密 + HMAC 追加
  - `pub fn decrypt_page(key: &EncryptionKey, page_id: u64, ciphertext: &[u8]) -> Result<Vec<u8>, Error>`：验证 HMAC + CBC 解密
  - 密文格式：`CBC密文 || HMAC-SM3(key.hmac, IV||密文)`

- [ ] **Step 1: 写失败测试**

```rust
// crates/embedded-database-core/src/crypto.rs 测试模块追加
#[test]
fn 加密页_往返() {
    let key = EncryptionKey {
        sm4: crate::sm4::Sm4Key { bytes: [0x01; 16] },
        hmac: [0x02; 32],
    };
    let payload = b"page payload data";
    let ct = encrypt_page(&key, 1, payload);
    assert!(ct.len() > payload.len(), "密文应包含 HMAC 尾部");
    let back = decrypt_page(&key, 1, &ct).unwrap();
    assert_eq!(back, payload);
}

#[test]
fn 加密页_HMAC验证() {
    let key = EncryptionKey {
        sm4: crate::sm4::Sm4Key { bytes: [0x01; 16] },
        hmac: [0x02; 32],
    };
    let ct = encrypt_page(&key, 1, b"data");
    // 篡改密文最后一个字节（HMAC 部分）
    let mut tampered = ct.clone();
    let last = tampered.len() - 1;
    tampered[last] ^= 0xFF;
    assert!(decrypt_page(&key, 1, &tampered).is_err(), "篡改应被 HMAC 检出");
}

#[test]
fn 加密页_错误page_id解密失败() {
    let key = EncryptionKey {
        sm4: crate::sm4::Sm4Key { bytes: [0x01; 16] },
        hmac: [0x02; 32],
    };
    let ct = encrypt_page(&key, 1, b"data");
    // 用错误 page_id（不同 IV）解密应失败
    assert!(decrypt_page(&key, 2, &ct).is_err(), "错误 IV 应导致解密失败");
}
```

- [ ] **Step 2: 运行测试验证失败**

Run: `cargo test -p embedded-database-core --features std crypto 2>&1 | tail -10`
Expected: 编译失败（`encrypt_page` 不存在）

- [ ] **Step 3: 实现**

```rust
// crates/embedded-database-core/src/crypto.rs 追加
use crate::hmac::hmac_sm3;
use crate::sm4::{derive_iv, sm4_cbc_decrypt, sm4_cbc_encrypt, Sm4Key};

const HMAC_SIZE: usize = 32;

/// 加密页：CBC 加密 + HMAC 追加。密文格式 = CBC密文 || HMAC。
#[cfg(feature = "std")]
pub fn encrypt_page(key: &EncryptionKey, page_id: u64, payload: &[u8]) -> std::vec::Vec<u8> {
    let iv = derive_iv(page_id);
    let ct = sm4_cbc_encrypt(&key.sm4, &iv, payload);
    // HMAC over IV || ciphertext
    let mut hmac_input = std::vec::Vec::with_capacity(16 + ct.len());
    hmac_input.extend_from_slice(&iv);
    hmac_input.extend_from_slice(&ct);
    let mac = hmac_sm3(&key.hmac, &hmac_input);
    let mut out = ct;
    out.extend_from_slice(&mac);
    out
}

/// 解密页：验证 HMAC + CBC 解密。
#[cfg(feature = "std")]
pub fn decrypt_page(
    key: &EncryptionKey,
    page_id: u64,
    ciphertext: &[u8],
) -> Result<std::vec::Vec<u8>, Error> {
    if ciphertext.len() <= HMAC_SIZE || (ciphertext.len() - HMAC_SIZE) % 16 != 0 {
        return Err(Error::InvalidArgument);
    }
    let (ct, mac) = ciphertext.split_at(ciphertext.len() - HMAC_SIZE);
    let iv = derive_iv(page_id);
    // 验证 HMAC
    let mut hmac_input = std::vec::Vec::with_capacity(16 + ct.len());
    hmac_input.extend_from_slice(&iv);
    hmac_input.extend_from_slice(ct);
    let expected = hmac_sm3(&key.hmac, &hmac_input);
    if mac != expected {
        return Err(Error::InvalidArgument);
    }
    sm4_cbc_decrypt(&key.sm4, &iv, ct)
}
```

- [ ] **Step 4: 运行测试**

Run: `cargo test -p embedded-database-core --features std crypto 2>&1 | tail -10`
Expected: 5 passed（2 已有 + 3 新增）

- [ ] **Step 5: 检查点**

Run: `rg -n 'fn encrypt_page' crates/embedded-database-core/src/crypto.rs`
Expected: 有输出

---

### Task M8-2: 审计日志接线

**Files:**
- Modify: `crates/embedded-database-core/src/db.rs`

**Interfaces:**
- Consumes: `DbStats`（M6-5）、`Db<'d>`（M4-3）
- Produces: `read_count`/`error_count`/`last_error` 在对应路径更新

- [ ] **Step 1: 写失败测试**

```rust
#[test]
fn 审计_读取计数() {
    let mut dev = InMemoryDevice::new(1 << 20);
    let mut db = Db::open(&mut dev, Config::default()).unwrap();
    {
        let mut txn = db.begin(TxnOptions::default()).unwrap();
        txn.put(b"k", b"v").unwrap();
        txn.commit().unwrap();
    }
    db.flush().unwrap();
    let before = db.stats().read_count;
    {
        let _txn = db.begin(TxnOptions::default()).unwrap();
        // get 不在本事务变更中，会调用 read_path（M8 接线后）
    }
    let after = db.stats().read_count;
    assert!(after > before, "读取操作应更新 read_count");
}

#[test]
fn 审计_错误计数() {
    let mut dev = InMemoryDevice::new(1 << 20);
    let mut db = Db::open(&mut dev, Config::default()).unwrap();
    let before = db.stats().error_count;
    // 尝试读不存在的页
    let _ = db.read_payload(999);
    let after = db.stats().error_count;
    assert!(after > before, "错误操作应更新 error_count");
}
```

- [ ] **Step 2: 运行测试验证失败**

Run: `cargo test -p embedded-database-core --features std db 2>&1 | tail -10`
Expected: `审计_读取计数` 失败（read_count 未接线）

- [ ] **Step 3: 实现**

在 `Db` 的读路径更新计数：
- `read_payload` 成功 → `read_count += 1`
- `read_payload` 失败 → `error_count += 1`，`last_error` 更新
- `Txn::get` 调用 `read_payload` 时同样更新

```rust
// read_payload 成功后
self.stats.borrow_mut().read_count += 1;

// read_payload 失败时
self.stats.borrow_mut().error_count += 1;
```

- [ ] **Step 4: 运行测试**

Run: `cargo test -p embedded-database-core --features std db 2>&1 | tail -10`
Expected: 37 passed（35 已有 + 2 新增）

- [ ] **Step 5: 检查点**

Run: `rg -n 'read_count' crates/embedded-database-core/src/db.rs`
Expected: 有输出

---

### Task M8-3: 遗留修复

**Files:**
- Modify: `crates/embedded-database-core/src/db.rs`

**Interfaces:**
- Consumes: `Db<'d>`（M4-3）、`Error`（Task 0）
- Produces:
  - `lookup_page` 取 `segment_seq` 最大值（非最早命中）
  - `open` 段超出设备时报 `InvalidArgument`（非静默截断）
  - page_id 全局递增计数器（非从 0 重复）

- [ ] **Step 1: 写失败测试**

```rust
#[test]
fn lookup_page_取最新版本() {
    let mut dev = InMemoryDevice::new(1 << 20 * 3);
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
    let db = Db::open(&mut dev, Config::default()).unwrap();
    let (seq, _) = db.lookup_page(0).unwrap();
    assert_eq!(seq, 2, "lookup_page 应返回最新段的版本");
}

#[test]
fn open_设备过小报错() {
    let mut dev = InMemoryDevice::new(100); // 极小设备
    let cfg = Config { segment_size: 1 << 20, ..Config::default() };
    // 设备小于一个段，应报错而非静默截断
    assert!(Db::open(&mut dev, cfg).is_err());
}
```

- [ ] **Step 2: 运行测试验证失败**

Run: `cargo test -p embedded-database-core --features std db 2>&1 | tail -10`
Expected: `lookup_page_取最新版本` 失败（当前返回最早命中）

- [ ] **Step 3: 实现**

1. `lookup_page` 改为取 `segment_seq` 最大值：
```rust
pub fn lookup_page(&self, page_id: u64) -> Option<(u64, u64)> {
    self.page_map
        .iter()
        .filter(|(pid, _, _)| *pid == page_id)
        .max_by_key(|(_, seq, _)| *seq)
        .map(|(_, seq, off)| (*seq, *off))
}
```

2. `open` 段超出设备时报错（改 `break` 为 `return Err`）：
```rust
if end > dev_size {
    return Err(Error::InvalidArgument);
}
```

3. page_id 全局递增：`Db` 加 `next_page_id: AtomicU64` 字段，`Txn::commit` 用它分配 page_id。

- [ ] **Step 4: 运行测试**

Run: `cargo test -p embedded-database-core --features std db 2>&1 | tail -10`
Expected: 39 passed（37 已有 + 2 新增）

- [ ] **Step 5: 检查点**

Run: `rg -n 'max_by_key' crates/embedded-database-core/src/db.rs`
Expected: 有输出

---

### Task M8-4: 加密集成测试

**Files:**
- Modify: `crates/embedded-database-core/src/writer.rs`
- Modify: `crates/embedded-database-core/src/db.rs`

**Interfaces:**
- Consumes: `encrypt_page`/`decrypt_page`（M8-1）、`SegmentWriter`（M1 Task 7）、`Db`（M4-3）
- Produces: 加密写入 → 解密读回的端到端测试

- [ ] **Step 1: 写测试**

```rust
#[test]
fn 加密端到端_CBC_HMAC() {
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
        txn.put(b"secret-key", b"secret-value").unwrap();
        txn.commit().unwrap();
    }
    db.flush().unwrap();
    // 读回并解密
    let payload = db.read_payload(0).unwrap();
    assert!(payload.windows(10).any(|w| w == b"secret-key"), "解密后应包含 key");
    assert!(payload.windows(12).any(|w| w == b"secret-value"), "解密后应包含 value");
}
```

- [ ] **Step 2: 运行测试**

Run: `cargo test -p embedded-database-core --features std db 2>&1 | tail -10`
Expected: 40 passed

- [ ] **Step 3: 检查点**

Run: `cargo test -p embedded-database-core --features std 2>&1 | rg 'test result' | head -1`
Expected: 193 passed

---

### Task M8-5: M8 收尾验收

**Files:**
- Modify: `docs/superpowers/specs/2026-10-02-embedded-database-design.md`
- Modify: `docs/superpowers/specs/2026-10-02-embedded-database-security-design.md`

**Interfaces:**
- Consumes: 全部 M8 Task
- Produces: 设计文档与安全设计文档更新

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

- [ ] **Step 6: 更新文档**

在设计文档追加「实施状态（M8 完成）」小节：
```markdown
## 实施状态（M8 完成）

- M8 交付：SM4-CBC（IV = page_id 派生）、每页 HMAC-SM3、审计日志接线
  （read_count/error_count/last_error）、遗留修复（lookup_page 取最新、
  open 报错、page_id 全局递增）。
- **M8 简化**：
  - CBC IV 为确定性（page_id 派生），相同明文+相同 page_id 产生相同密文。
  - HMAC 值追加在密文尾部（每页额外 32 字节）。
  - page_id 全局递增，但 key→page 映射仍未实现（KV 语义需 M9）。
```

在安全设计文档更新「数据保密性」章节（ECB → CBC + HMAC）。

- [ ] **Step 7: 检查点**

Run: `rg -n 'M8 完成' docs/superpowers/specs/2026-10-02-embedded-database-design.md`
Expected: 有输出

---

## M8 验收标准

| # | 标准 | 验证方式 |
|---|------|----------|
| 1 | SM4-CBC 加密解密往返 | M8-0 测试 |
| 2 | 每页 HMAC 验证（篡改检出） | M8-1 测试 |
| 3 | 审计计数接线 | M8-2 测试 |
| 4 | lookup_page 取最新 | M8-3 测试 |
| 5 | open 设备过小报错 | M8-3 测试 |
| 6 | 加密端到端（写入→解密读回） | M8-4 测试 |
| 7 | 5 个 no_std 目标全部编译通过 | M8-5 Step 3 |
| 8 | 全 crate 零 `unsafe` | M8-5 Step 4 |
| 9 | 全部测试通过 | M8-5 Step 1 |
| 10 | `inject_crash` exit=0 且 `wcet` exit=0 | M8-5 Step 5 |

## 后续里程碑

M8 完成后，加密达到生产级安全（CBC + HMAC）。剩余工作：

- **key→page 映射**（KV 语义完整化）
- **段回收/同段追加**（M7 简化）
- **LoongArch 实机验证**（拿到硬件后）
- **认证申报**（GB/T 22239、CSTC 适配）
- **试点部署**
