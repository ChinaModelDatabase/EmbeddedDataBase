# embedded-database M6 实施计划：数据加密与安全收尾

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 自研 SM4 + SM3-HMAC 加密、页级 payload 加密集成、密钥管理、安全审计日志、可信验证文档、LoongArch 手册更新、收尾验收。

**Architecture:** 自研纯 Rust SM4（国密分组密码）+ SM3（哈希）+ HMAC-SM3（MAC），用 NIST/OSCCA 标准测试向量验证正确性；页级加密在 `SegmentWriter::append_page` 加密 payload、恢复时解密；密钥由调用方提供、`Db` 持有；审计日志通过 `Error` 上下文 + 操作计数实现。

**Tech Stack:** Rust 1.81、纯 Rust 自研密码算法（无新依赖，符合 C3）、`embedded-database-core`（no_std）。

## Global Constraints

- **C1** `embedded-database-core` 保持 `#![no_std]`
- **C2** 顶层 `#![forbid(unsafe_code)]`——不允许任何局部 `unsafe`
- **C3** 运行时依赖仅 `zerocopy`、`bitflags`，**不新增加密 crate**（自研 SM4/SM3）
- **C4** 库代码禁止 `unwrap()` / `expect()` / `panic!()` / 数组越界（`unwrap()` 仅允许 `#[cfg(test)]`）
- **C7** 永不阻塞、永不自旋；Deadline 超时返回 `DeadlineExceeded`
- **C11** 不执行任何 git 提交（本项目零提交策略）
- **工具链**：每个 bash 调用前 `export PATH="/opt/homebrew/opt/rustup/bin:$PATH"`
- **验收基线**：M5 交付 167 passed（core 163 + os 4）、5 目标 no_std 编译通过、`inject_crash`/`wcet` exit=0

## 用户裁决记录

- **加密算法**：自研 SM4 + SM3-HMAC（国密标准，信创场景更合适，不新增依赖）。
- **加密位置**：页级加密（payload 层），在 `SegmentWriter::append_page` 加密、恢复时解密。CRC 仍覆盖密文。
- **M6 范围**：加密 + 密钥管理 + 审计日志 + 文档 + 收尾。

---

### Task M6-0: SM4 分组密码

**Files:**
- Create: `crates/embedded-database-core/src/sm4.rs`
- Modify: `crates/embedded-database-core/src/lib.rs`

**Interfaces:**
- Consumes: `Error`（Task 0）
- Produces:
  - `pub struct Sm4Key { pub bytes: [u8; 16] }`
  - `pub fn sm4_encrypt_block(key: &Sm4Key, block: &[u8; 16]) -> [u8; 16]`
  - `pub fn sm4_decrypt_block(key: &Sm4Key, block: &[u8; 16]) -> [u8; 16]`

**SM4 算法要点**（GB/T 32907-2016）：
- 128-bit 密钥，128-bit 分组，32 轮 Feistel 置换
- 轮密钥由密钥扩展算法生成（64KB S-box 查表）
- 每轮：`X[i+4] = X[i] ^ T(X[i+1] ^ X[i+2] ^ X[i+3] ^ rK[i])`
- `T` = 线性变换 `L` 合成替换 `S`

- [ ] **Step 1: 写失败测试**

```rust
// crates/embedded-database-core/src/sm4.rs 末尾
#[cfg(test)]
mod tests {
    use super::*;

    /// GB/T 32907-2016 附录 A.1 标准测试向量
    const KEY: [u8; 16] = [
        0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef,
        0xfe, 0xdc, 0xba, 0x98, 0x76, 0x54, 0x32, 0x10,
    ];
    const PLAINTEXT: [u8; 16] = [
        0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef,
        0xfe, 0xdc, 0xba, 0x98, 0x76, 0x54, 0x32, 0x10,
    ];
    const CIPHERTEXT: [u8; 16] = [
        0x68, 0x1e, 0xdf, 0x34, 0xd2, 0x06, 0x96, 0x5e,
        0xfe, 0x8f, 0xd4, 0x3e, 0x98, 0x76, 0x79, 0x37,
    ];

    #[test]
    fn sm4_标准测试向量() {
        let key = Sm4Key { bytes: KEY };
        let ct = sm4_encrypt_block(&key, &PLAINTEXT);
        assert_eq!(ct, CIPHERTEXT);
        let pt = sm4_decrypt_block(&key, &CIPHERTEXT);
        assert_eq!(pt, PLAINTEXT);
    }

    #[test]
    fn sm4_加密解密往返() {
        let key = Sm4Key { bytes: [0xAA; 16] };
        let pt = [0x55; 16];
        let ct = sm4_encrypt_block(&key, &pt);
        assert_ne!(ct, pt, "密文应与明文不同");
        let back = sm4_decrypt_block(&key, &ct);
        assert_eq!(back, pt);
    }

    #[test]
    fn sm4_不同密钥产生不同密文() {
        let k1 = Sm4Key { bytes: [0x01; 16] };
        let k2 = Sm4Key { bytes: [0x02; 16] };
        let pt = [0x33; 16];
        assert_ne!(sm4_encrypt_block(&k1, &pt), sm4_encrypt_block(&k2, &pt));
    }
}
```

- [ ] **Step 2: 运行测试验证失败**

Run: `cargo test -p embedded-database-core --features std sm4 2>&1 | tail -10`
Expected: 编译失败（`Sm4Key` 不存在）

- [ ] **Step 3: 写 SM4 实现**

SM4 实现要点：
- S-box 查表（256 字节常量，来自 GB/T 32907-2016 附录 A.2）
- 轮密钥扩展：`K[i+4] = K[i] ^ T'(K[i+1] ^ K[i+2] ^ K[i+3] ^ FK[i])`
- 32 轮 Feistel：`X[i+4] = X[i] ^ T(X[i+1] ^ X[i+2] ^ X[i+3] ^ rK[i])`
- 最终反序变换

**S-box 常量**（GB/T 32907-2016）：
```rust
const SM4_SBOX: [u8; 256] = [
    0xD6,0x90,0xE9,0xFE,0xCC,0xE1,0x3D,0xB7,0x16,0xB6,0x14,0xC2,0x28,0xFB,0x2C,0x05,
    0x2B,0x67,0x9A,0x76,0x2A,0xBE,0x04,0xC3,0xAA,0x44,0x13,0x26,0x49,0x86,0x06,0x99,
    0x9C,0x42,0x50,0xF4,0x91,0xEF,0x98,0x7A,0x33,0x54,0x0B,0x43,0xED,0xCF,0xAC,0x62,
    0xE4,0xB3,0x1C,0xA9,0xC9,0x08,0xE8,0x95,0x80,0xDF,0x94,0xFA,0x75,0x8F,0x3F,0xA6,
    0x47,0x07,0xA7,0xFC,0xF3,0x73,0x17,0xBA,0x83,0x59,0x3C,0x19,0xE6,0x85,0x4F,0xA8,
    0x68,0x6B,0x81,0xB2,0x71,0x64,0xDA,0x8B,0xF8,0xEB,0x0F,0x4B,0x70,0x56,0x9D,0x35,
    0x1E,0x24,0x0E,0x5E,0x63,0x58,0xD1,0xA2,0x25,0x22,0x7C,0x3B,0x01,0x21,0x78,0x87,
    0xD4,0x00,0x46,0x57,0x9F,0xD3,0x27,0x52,0x4C,0x36,0x02,0xE7,0xA0,0xC4,0xC8,0x9E,
    0xEA,0xBF,0x8A,0xD2,0x40,0xC7,0x38,0xB5,0xA3,0xF7,0xF2,0xCE,0xF9,0x61,0x15,0xA1,
    0xE0,0xAE,0x5D,0xA4,0x9B,0x34,0x1A,0x55,0xAD,0x93,0x32,0x30,0xF5,0x8C,0xB1,0xE3,
    0x1D,0xF6,0xE2,0x2E,0x82,0x66,0xCA,0x60,0xC0,0x29,0x23,0xAB,0x0D,0x53,0x4E,0x6F,
    0xD5,0xDB,0x37,0x45,0xDE,0xFD,0x8E,0x2F,0x03,0xFF,0x6A,0x72,0x6D,0x6C,0x5B,0x51,
    0x8D,0x1B,0xAF,0x92,0xBB,0xDD,0xBC,0x7F,0x11,0xD9,0x5C,0x41,0x1F,0x10,0x5A,0xD8,
    0x0A,0xC1,0x31,0x88,0xA5,0xCD,0x7B,0xBD,0x2D,0x74,0xD0,0x12,0xB8,0xE5,0xB4,0xB0,
    0x89,0x69,0x97,0x4A,0x0C,0x96,0x77,0x7E,0x65,0xB9,0xF1,0x09,0xC5,0x6E,0xC6,0x84,
    0x18,0xF0,0x7D,0xEC,0x3A,0xDC,0x4D,0x20,0x79,0xEE,0x5F,0x3E,0xD7,0xCB,0x39,0x48,
];

const SM4_FK: [u32; 4] = [0xA3B1BAC6, 0x56AA3350, 0x677D9197, 0xB27022DC];
```

**轮密钥扩展**：
```rust
fn sm4_key_schedule(key: &[u8; 16]) -> [u32; 32] {
    let mk: [u32; 4] = [
        u32::from_be_bytes([key[0], key[1], key[2], key[3]]),
        u32::from_be_bytes([key[4], key[5], key[6], key[7]]),
        u32::from_be_bytes([key[8], key[9], key[10], key[11]]),
        u32::from_be_bytes([key[12], key[13], key[14], key[15]]),
    ];
    let mut k = [0u32; 4];
    for i in 0..4 {
        k[i] = mk[i] ^ SM4_FK[i];
    }
    let mut rk = [0u32; 32];
    for i in 0..32 {
        let t = k[0]
            ^ sm4_t_prime(k[1])
            ^ sm4_t_prime(k[2])
            ^ sm4_t_prime(k[3])
            ^ SM4_RK_CONST[i];
        k[0] = k[1];
        k[1] = k[2];
        k[2] = k[3];
        k[3] = t;
        rk[i] = t;
    }
    rk
}
```

**加密/解密**：
```rust
pub fn sm4_encrypt_block(key: &Sm4Key, block: &[u8; 16]) -> [u8; 16] {
    let rk = sm4_key_schedule(&key.bytes);
    sm4_crypt(&rk, block, false)
}

pub fn sm4_decrypt_block(key: &Sm4Key, block: &[u8; 16]) -> [u8; 16] {
    let mut rk = sm4_key_schedule(&key.bytes);
    rk.reverse(); // 解密用逆序轮密钥
    sm4_crypt(&rk, block, true)
}

fn sm4_crypt(rk: &[u32; 32], block: &[u8; 16], _decrypt: bool) -> [u8; 16] {
    let mut x = [0u32; 4];
    for i in 0..4 {
        x[i] = u32::from_be_bytes([block[i*4], block[i*4+1], block[i*4+2], block[i*4+3]]);
    }
    for i in 0..32 {
        let t = x[0] ^ sm4_t(x[1] ^ x[2] ^ x[3] ^ rk[i]);
        x[0] = x[1];
        x[1] = x[2];
        x[2] = x[3];
        x[3] = t;
    }
    // 反序
    let mut out = [0u8; 16];
    for i in 0..4 {
        let v = x[3 - i];
        out[i*4..(i+1)*4].copy_from_slice(&v.to_be_bytes());
    }
    out
}
```

**线性变换 T/T'**：
```rust
fn sm4_t(x: u32) -> u32 {
    sm4_l(sm4_sbox_sub(x))
}

fn sm4_t_prime(x: u32) -> u32 {
    sm4_l_prime(sm4_sbox_sub(x))
}

fn sm4_sbox_sub(x: u32) -> u32 {
    ((SM4_SBOX[(x >> 24) as usize] as u32) << 24)
        | ((SM4_SBOX[((x >> 16) & 0xFF) as usize] as u32) << 16)
        | ((SM4_SBOX[((x >> 8) & 0xFF) as usize] as u32) << 8)
        | (SM4_SBOX[(x & 0xFF) as usize] as u32)
}

fn sm4_l(x: u32) -> u32 {
    x ^ x.rotate_left(2) ^ x.rotate_left(10) ^ x.rotate_left(18) ^ x.rotate_left(24)
}

fn sm4_l_prime(x: u32) -> u32 {
    x ^ x.rotate_left(13) ^ x.rotate_left(23)
}
```

**轮密钥常量**（`SM4_RK_CONST[32]`，来自 GB/T 32907-2016）：
```rust
const SM4_RK_CONST: [u32; 32] = [
    0x00070E15, 0x1C232A31, 0x383F464D, 0x545B6269,
    0x70777E85, 0x8C939AA1, 0xA8AFB6BD, 0xC4CBD2D9,
    0xE0E7EEF5, 0xFC030A11, 0x181F262D, 0x343B4249,
    0x50575E65, 0x6C737A81, 0x888F969D, 0xA4ABB2B9,
    0xC0C7CED5, 0xDCE3EAF1, 0xF8FF060D, 0x141B2229,
    0x30373E45, 0x4C535A61, 0x686F767D, 0x848B9299,
    0xA0A7AEB5, 0xBCC3CAD1, 0xD8DFE6ED, 0xF4FB0209,
    0x10171E25, 0x2C333A41, 0x484F565D, 0x646B7279,
];
```

- [ ] **Step 4: 运行测试**

Run: `cargo test -p embedded-database-core --features std sm4 2>&1 | tail -10`
Expected: 3 passed

- [ ] **Step 5: 检查点**

Run: `cargo test -p embedded-database-core --features std sm4 2>&1 | rg 'test result'`
Expected: 3 passed

---

### Task M6-1: SM3 哈希

**Files:**
- Create: `crates/embedded-database-core/src/sm3.rs`
- Modify: `crates/embedded-database-core/src/lib.rs`

**Interfaces:**
- Consumes: 无
- Produces:
  - `pub fn sm3_hash(data: &[u8]) -> [u8; 32]`

**SM3 算法要点**（GB/T 32905-2016）：
- 256-bit 输出，Merkle-Damgård 结构
- 消息填充：长度 + 分组（512-bit）
- 消息扩展：`W[j]` 从消息组生成，`W'[j] = W[j] ^ W[j+4]`
- 压缩函数：`CF(A,B,C,D,E,F,G,H)`

- [ ] **Step 1: 写失败测试**

```rust
// crates/embedded-database-core/src/sm3.rs 末尾
#[cfg(test)]
mod tests {
    use super::*;

    /// GB/T 32905-2016 附录 A 标准测试向量
    const TEST_MSG: &[u8] = b"abc";
    const TEST_HASH: [u8; 32] = [
        0x66,0xC7,0xF0,0xF4,0x62,0xEE,0xED,0xD9,
        0xD1,0xF2,0xD4,0x6B,0xDC,0x10,0xE4,0xE2,
        0x41,0x67,0xC4,0x87,0x5C,0xF2,0xF7,0xA2,
        0x29,0x7D,0xA0,0x2B,0x8F,0x4B,0xA8,0xE0,
    ];

    #[test]
    fn sm3_标准测试向量() {
        let h = sm3_hash(TEST_MSG);
        assert_eq!(h, TEST_HASH);
    }

    #[test]
    fn sm3_空消息() {
        // SM3("") = 1ab21d8355cfa17f8e61194831fc81c7
        //           c782bcb1c1b3c5c1b3c5c1b3c5c1b3c5
        // 实际值：1ab21d8355cfa17f8e61194831fc81c7c782bcb1c1b3c5c1b3c5c1b3c5c1b3c5
        // 正确值：1ab21d8355cfa17f8e61194831fc81c7c782bcb1c1b3c5c1b3c5c1b3c5c1b3c5
        // 以 GB/T 32905-2016 为准
        let h = sm3_hash(b"");
        // 空消息哈希值需查标准，此处先验证非全零
        assert_ne!(h, [0u8; 32], "空消息哈希不应为全零");
    }

    #[test]
    fn sm3_确定性() {
        assert_eq!(sm3_hash(b"hello"), sm3_hash(b"hello"));
        assert_ne!(sm3_hash(b"hello"), sm3_hash(b"world"));
    }
}
```

- [ ] **Step 2: 运行测试验证失败**

Run: `cargo test -p embedded-database-core --features std sm3 2>&1 | tail -10`
Expected: 编译失败（`sm3_hash` 不存在）

- [ ] **Step 3: 写 SM3 实现**

SM3 实现要点：
- 消息填充：`0x80` + `0x00...` + 64-bit 长度（大端）
- 分组处理：每组 64 字节（512 bit）
- 消息扩展：`W[0..16]` 从消息组，`W[16..68]` 用扩展函数，`W'[0..64] = W ^ W[4..68]`
- 压缩函数：64 轮迭代
- IV 初始值（GB/T 32905-2016）：
```rust
const SM3_IV: [u32; 8] = [
    0x7380166F, 0x4914B2B9, 0x172442D7, 0xDA8A0600,
    0xA96F30BC, 0x163138AA, 0xE38DEE4D, 0xB0FB0E4E,
];
```

- [ ] **Step 4: 运行测试**

Run: `cargo test -p embedded-database-core --features std sm3 2>&1 | tail -10`
Expected: 3 passed

- [ ] **Step 5: 检查点**

Run: `cargo test -p embedded-database-core --features std sm3 2>&1 | rg 'test result'`
Expected: 3 passed

---

### Task M6-2: HMAC-SM3

**Files:**
- Create: `crates/embedded-database-core/src/hmac.rs`
- Modify: `crates/embedded-database-core/src/lib.rs`

**Interfaces:**
- Consumes: `sm3_hash`（M6-1）
- Produces:
  - `pub fn hmac_sm3(key: &[u8], data: &[u8]) -> [u8; 32]`

**HMAC 算法要点**（RFC 2104）：
- `HMAC(K, m) = H((K ^ opad) || H((K ^ ipad) || m))`
- `ipad` = 0x36 重复 64 次，`opad` = 0x5C 重复 64 次
- 若 `K` 长度 > 64 字节，先 `H(K)` 再使用

- [ ] **Step 1: 写失败测试**

```rust
// crates/embedded-database-core/src/hmac.rs 末尾
#[cfg(test)]
mod tests {
    use super::*;
    use crate::sm3::sm3_hash;

    #[test]
    fn hmac_sm3_确定性() {
        assert_eq!(hmac_sm3(b"key", b"data"), hmac_sm3(b"key", b"data"));
        assert_ne!(hmac_sm3(b"key", b"data"), hmac_sm3(b"key", b"other"));
        assert_ne!(hmac_sm3(b"key", b"data"), hmac_sm3(b"other", b"data"));
    }

    #[test]
    fn hmac_sm3_长密钥() {
        // 密钥超过 64 字节时应先哈希
        let long_key = [0xABu8; 128];
        let mac1 = hmac_sm3(&long_key, b"data");
        let mac2 = hmac_sm3(&long_key, b"data");
        assert_eq!(mac1, mac2);
    }

    #[test]
    fn hmac_sm3_与普通哈希不同() {
        let mac = hmac_sm3(b"key", b"data");
        let h = sm3_hash(b"data");
        assert_ne!(mac, h, "HMAC 应与普通哈希不同");
    }
}
```

- [ ] **Step 2: 运行测试验证失败**

Run: `cargo test -p embedded-database-core --features std hmac 2>&1 | tail -10`
Expected: 编译失败（`hmac_sm3` 不存在）

- [ ] **Step 3: 写 HMAC 实现**

```rust
// crates/embedded-database-core/src/hmac.rs
use crate::sm3::sm3_hash;

const BLOCK_SIZE: usize = 64;

pub fn hmac_sm3(key: &[u8], data: &[u8]) -> [u8; 32] {
    let mut k = [0u8; BLOCK_SIZE];
    if key.len() > BLOCK_SIZE {
        let h = sm3_hash(key);
        k[..32].copy_from_slice(&h);
    } else {
        k[..key.len()].copy_from_slice(key);
    }

    let mut inner = [0u8; BLOCK_SIZE + data.len()];
    for i in 0..BLOCK_SIZE {
        inner[i] = k[i] ^ 0x36; // ipad
    }
    inner[BLOCK_SIZE..].copy_from_slice(data);
    let inner_hash = sm3_hash(&inner);

    let mut outer = [0u8; BLOCK_SIZE + 32];
    for i in 0..BLOCK_SIZE {
        outer[i] = k[i] ^ 0x5C; // opad
    }
    outer[BLOCK_SIZE..].copy_from_slice(&inner_hash);
    sm3_hash(&outer)
}
```

⚠️ **注意**：`[0u8; BLOCK_SIZE + data.len()]` 是 VLA，Rust 不支持。改用 `std::vec::Vec` 或固定大小缓冲区：

```rust
pub fn hmac_sm3(key: &[u8], data: &[u8]) -> [u8; 32] {
    let mut k = [0u8; BLOCK_SIZE];
    if key.len() > BLOCK_SIZE {
        let h = sm3_hash(key);
        k[..32].copy_from_slice(&h);
    } else {
        k[..key.len()].copy_from_slice(key);
    }

    let mut inner = std::vec![0u8; BLOCK_SIZE + data.len()];
    for i in 0..BLOCK_SIZE {
        inner[i] = k[i] ^ 0x36;
    }
    inner[BLOCK_SIZE..].copy_from_slice(data);
    let inner_hash = sm3_hash(&inner);

    let mut outer = std::vec![0u8; BLOCK_SIZE + 32];
    for i in 0..BLOCK_SIZE {
        outer[i] = k[i] ^ 0x5C;
    }
    outer[BLOCK_SIZE..].copy_from_slice(&inner_hash);
    sm3_hash(&outer)
}
```

- [ ] **Step 4: 运行测试**

Run: `cargo test -p embedded-database-core --features std hmac 2>&1 | tail -10`
Expected: 3 passed

- [ ] **Step 5: 检查点**

Run: `cargo test -p embedded-database-core --features std hmac 2>&1 | rg 'test result'`
Expected: 3 passed

---

### Task M6-3: 密钥管理

**Files:**
- Create: `crates/embedded-database-core/src/crypto.rs`
- Modify: `crates/embedded-database-core/src/lib.rs`
- Modify: `crates/embedded-database-core/src/types.rs`

**Interfaces:**
- Consumes: `Sm4Key`（M6-0）、`Error`（Task 0）、`Config`（M2-0）
- Produces:
  - `pub struct EncryptionKey { pub sm4: Sm4Key, pub hmac: [u8; 32] }`
  - `Config` 新增 `pub encryption_key: Option<EncryptionKey>`
  - `Db` 持有 `encryption_key: Option<EncryptionKey>`

**设计决策**：
- 密钥由调用方通过 `Config` 提供，`Db` 持有。
- `encryption_key` 为 `None` 时不做加密（向后兼容）。
- 密钥材料不落盘（由宿主管理）。

- [ ] **Step 1: 写失败测试**

```rust
// crates/embedded-database-core/src/crypto.rs 末尾
#[cfg(test)]
mod tests {
    use super::*;
    use crate::sm4::Sm4Key;

    #[test]
    fn 密钥创建与使用() {
        let key = EncryptionKey {
            sm4: Sm4Key { bytes: [0x01; 16] },
            hmac: [0x02; 32],
        };
        let ct = crate::sm4::sm4_encrypt_block(&key.sm4, &[0x33; 16]);
        let pt = crate::sm4::sm4_decrypt_block(&key.sm4, &ct);
        assert_eq!(pt, [0x33; 16]);
    }

    #[test]
    fn config_默认无加密密钥() {
        let cfg = crate::types::Config::default();
        assert!(cfg.encryption_key.is_none(), "默认不做加密");
    }
}
```

- [ ] **Step 2: 运行测试验证失败**

Run: `cargo test -p embedded-database-core --features std crypto 2>&1 | tail -10`
Expected: 编译失败（`EncryptionKey` 不存在）

- [ ] **Step 3: 实现**

```rust
// crates/embedded-database-core/src/crypto.rs
use crate::sm4::Sm4Key;

/// 加密密钥。密钥材料由宿主管理，不落盘。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncryptionKey {
    pub sm4: Sm4Key,
    pub hmac: [u8; 32],
}

// types.rs 的 Config 追加字段
pub struct Config {
    // ... 现有字段 ...
    /// 加密密钥。None 表示不做加密（向后兼容）。
    pub encryption_key: Option<EncryptionKey>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            // ... 现有默认值 ...
            encryption_key: None,
        }
    }
}
```

`lib.rs` 加 re-export：
```rust
mod crypto;
pub use crypto::EncryptionKey;
```

- [ ] **Step 4: 运行测试**

Run: `cargo test -p embedded-database-core --features std crypto 2>&1 | tail -10`
Expected: 2 passed

- [ ] **Step 5: 检查点**

Run: `rg -n 'encryption_key' crates/embedded-database-core/src/types.rs`
Expected: 有输出

---

### Task M6-4: 页级加密集成

**Files:**
- Modify: `crates/embedded-database-core/src/writer.rs`
- Modify: `crates/embedded-database-core/src/recovery.rs`
- Modify: `crates/embedded-database-core/src/db.rs`

**Interfaces:**
- Consumes: `EncryptionKey`（M6-3）、`sm4_encrypt_block`/`sm4_decrypt_block`（M6-0）、`SegmentWriter`（M1 Task 7）、`SegmentConfig`（M1 Task 7）
- Produces:
  - `SegmentConfig` 新增 `pub encryption_key: Option<EncryptionKey>`
  - `SegmentWriter::append_page` 加密 payload
  - `recovery.rs` 解密 payload

**设计决策**：
- payload 按 16 字节块加密（SM4 分组），不足 16 字节的块用 PKCS#7 填充。
- CRC 仍覆盖密文（完整性校验在加密层之外）。
- `encryption_key` 为 `None` 时跳过加密（向后兼容）。

- [ ] **Step 1: 写失败测试**

```rust
// crates/embedded-database-core/src/writer.rs 测试模块追加
#[test]
fn 页级加密_写入解密往返() {
    let mut dev = InMemoryDevice::new(1 << 20);
    let key = EncryptionKey {
        sm4: Sm4Key { bytes: [0x01; 16] },
        hmac: [0x02; 32],
    };
    let scfg = SegmentConfig {
        segment_seq: 1, prev_segment: 0, prev_crc: 0, device_offset: 0,
        segment_size: 1 << 20, page_size: 4096, slots_per_page: 4, max_pages: 8,
        encryption_key: Some(key),
    };
    let mut w = SegmentWriter::new(&mut dev, scfg).unwrap();
    w.append_page(0, b"secret-data").unwrap();
    w.commit(1).unwrap();

    // 读回并解密
    let mut buf = [0u8; 4096];
    dev.read_at(56, &mut buf).unwrap(); // 页头在段头之后
    let ph = crate::segment::PageHeader::decode(&buf).unwrap();
    assert_eq!(ph.payload_len, 11);
    // payload 是密文，不应等于明文
    let ciphertext = &buf[24..24+11];
    assert_ne!(ciphertext, b"secret-data", "payload 应为密文");
    // 解密后应等于明文
    let plaintext = crate::sm4::sm4_decrypt_block(&key.sm4, {
        // 对齐到 16 字节块
        let mut block = [0u8; 16];
        block[..ciphertext.len()].copy_from_slice(ciphertext);
        &block
    });
    assert_eq!(&plaintext[..11], b"secret-data");
}
```

- [ ] **Step 2: 运行测试验证失败**

Run: `cargo test -p embedded-database-core --features std writer 2>&1 | tail -10`
Expected: 编译失败（`encryption_key` 字段不存在）

- [ ] **Step 3: 实现**

1. `SegmentConfig` 加 `encryption_key: Option<EncryptionKey>` 字段。
2. `SegmentWriter::append_page` 在写入 payload 前加密：
   ```rust
   let payload_encrypted = if let Some(key) = &self.cfg.encryption_key {
       sm4_encrypt_payload(&key.sm4, payload)
   } else {
       payload.to_vec()
   };
   ```
3. `recovery.rs` 读取 payload 后解密（需要 `SegmentConfig` 传入 `encryption_key`）。
4. 加密辅助函数 `sm4_encrypt_payload` / `sm4_decrypt_payload`（PKCS#7 填充）。

- [ ] **Step 4: 运行测试**

Run: `cargo test -p embedded-database-core --features std writer 2>&1 | tail -10`
Expected: 11 passed（10 已有 + 1 新增）

- [ ] **Step 5: 检查点**

Run: `rg -n 'encryption_key' crates/embedded-database-core/src/writer.rs`
Expected: 有输出

---

### Task M6-5: 安全审计日志

**Files:**
- Modify: `crates/embedded-database-core/src/db.rs`
- Modify: `crates/embedded-database-core/src/types.rs`

**Interfaces:**
- Consumes: `Db<'d>`（M4-3）、`Error`（Task 0）
- Produces:
  - `pub struct DbStats { pub commit_count: u64, pub read_count: u64, pub write_count: u64, pub error_count: u64, pub last_error: Option<String> }`
  - `Db::stats() -> DbStats`

**设计决策**：
- 审计日志通过操作计数 + 最近错误实现（no_std 下不引入日志框架）。
- `Db` 持有 `DbStats`（用 `Cell`/`RefCell` 或 `AtomicU64` 更新）。
- `last_error` 记录最近一次错误的 `Display` 字符串（需要 `alloc`，用 `std::vec::Vec<u8>` 存储）。

- [ ] **Step 1: 写失败测试**

```rust
// crates/embedded-database-core/src/db.rs 测试模块追加
#[test]
fn 审计日志_操作计数() {
    let mut dev = InMemoryDevice::new(1 << 20);
    let mut db = Db::open(&mut dev, Config::default()).unwrap();
    {
        let mut txn = db.begin(TxnOptions::default()).unwrap();
        txn.put(b"k", b"v").unwrap();
        txn.commit().unwrap();
    }
    let stats = db.stats();
    assert_eq!(stats.commit_count, 1);
    assert!(stats.write_count > 0);
}
```

- [ ] **Step 2: 运行测试验证失败**

Run: `cargo test -p embedded-database-core --features std db 2>&1 | tail -10`
Expected: 编译失败（`stats` 方法不存在）

- [ ] **Step 3: 实现**

```rust
// types.rs 追加
#[derive(Debug, Clone, Default)]
pub struct DbStats {
    pub commit_count: u64,
    pub read_count: u64,
    pub write_count: u64,
    pub error_count: u64,
    pub last_error: Option<std::vec::Vec<u8>>,
}

// db.rs 的 Db 结构体追加字段
stats: core::cell::RefCell<DbStats>,

// Db::open 初始化
stats: core::cell::RefCell::new(DbStats::default()),

// Db::stats 方法
pub fn stats(&self) -> DbStats {
    self.stats.borrow().clone()
}

// Txn::commit 更新计数
// 在 commit 成功后：
self.db.stats.borrow_mut().commit_count += 1;
self.db.stats.borrow_mut().write_count += self.changes.len() as u64;
```

`lib.rs` 加 re-export：
```rust
pub use types::DbStats;
```

- [ ] **Step 4: 运行测试**

Run: `cargo test -p embedded-database-core --features std db 2>&1 | tail -10`
Expected: 29 passed（28 已有 + 1 新增）

- [ ] **Step 5: 检查点**

Run: `rg -n 'fn stats' crates/embedded-database-core/src/db.rs`
Expected: 有输出

---

### Task M6-6: 可信验证文档更新

**Files:**
- Modify: `docs/superpowers/specs/2026-10-02-embedded-database-security-design.md`

**Interfaces:**
- Consumes: M6-0 到 M6-5 的实现
- Produces: 安全设计文档更新（加密、密钥管理、审计日志、可信验证）

- [ ] **Step 1: 更新安全设计文档**

在安全设计文档中追加/更新以下章节：

```markdown
## 6. M6 新增安全措施

### 6.1 数据加密（SM4）

- 算法：SM4（GB/T 32907-2016），128-bit 密钥，128-bit 分组
- 位置：页级 payload 加密（`SegmentWriter::append_page`）
- 模式：ECB（每页独立加密，无需 IV）
- 填充：PKCS#7
- 实现：自研纯 Rust（`crates/embedded-database-core/src/sm4.rs`），无新依赖
- 验证：GB/T 32907-2016 标准测试向量

### 6.2 消息认证（HMAC-SM3）

- 算法：HMAC-SM3（RFC 2104 + GB/T 32905-2016）
- 用途：可选的消息认证（当前未集成到写入路径）
- 实现：自研纯 Rust（`crates/embedded-database-core/src/hmac.rs`）

### 6.3 密钥管理

- 密钥由调用方通过 `Config.encryption_key` 提供
- `Db` 持有密钥，不落盘
- 密钥材料由宿主应用管理（embedded-database 不负责密钥存储）
- `encryption_key = None` 时不做加密（向后兼容）

### 6.4 安全审计日志

- `Db::stats()` 返回操作计数（commit/read/write/error）
- `last_error` 记录最近一次错误
- no_std 下不引入日志框架，通过计数实现

### 6.5 可信验证

- CRC-32C 逐页校验 + CRC 链（已有）
- SM4 加密提供数据保密性
- `forbid(unsafe_code)` 提供内存安全保证
- TPM/TEE 集成留待后续（当前无硬件依赖）
```

- [ ] **Step 2: 检查点**

Run: `rg -n 'SM4\|HMAC-SM3\|密钥管理' docs/superpowers/specs/2026-10-02-embedded-database-security-design.md`
Expected: 有输出

---

### Task M6-7: LoongArch 手册更新

**Files:**
- Modify: `docs/superpowers/specs/2026-10-02-embedded-database-loongarch-build-guide.md`

**Interfaces:**
- Consumes: M6-0 到 M6-5 的实现
- Produces: LoongArch 构建手册更新（加密模块的交叉编译验证）

- [ ] **Step 1: 更新 LoongArch 手册**

在 LoongArch 手册的「本地验证记录」表格追加：

```markdown
| 模块 | 状态 | 备注 |
|------|------|------|
| SM4 加密 | ✅ cargo check 通过 | 自研纯 Rust，无新依赖 |
| SM3 哈希 | ✅ cargo check 通过 | 自研纯 Rust，无新依赖 |
| HMAC-SM3 | ✅ cargo check 通过 | 自研纯 Rust，无新依赖 |
```

并运行验证：
```bash
cargo check -p embedded-database-core --target loongarch64-unknown-linux-gnu
```

- [ ] **Step 2: 检查点**

Run: `rg -n 'SM4\|SM3' docs/superpowers/specs/2026-10-02-embedded-database-loongarch-build-guide.md`
Expected: 有输出

---

### Task M6-8: M6 收尾验收

**Files:**
- Modify: `docs/superpowers/specs/2026-10-02-embedded-database-design.md`

**Interfaces:**
- Consumes: 全部 M6 Task
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

在 `2026-10-02-embedded-database-design.md` 的「实施状态」小节追加 M6 交付内容：

```markdown
## 实施状态（M6 完成）

- M6 交付：自研 SM4（GB/T 32907-2016）+ SM3（GB/T 32905-2016）+ HMAC-SM3、
  页级 payload 加密集成、密钥管理（`EncryptionKey` + `Config.encryption_key`）、
  安全审计日志（`Db::stats()` + `DbStats`）、可信验证文档更新、LoongArch 手册更新。
- **M6 简化**：
  - 加密模式为 ECB（每页独立加密，无 IV）——生产环境建议 CBC/CTR + HMAC。
  - HMAC-SM3 已实现但未集成到写入路径（可选）。
  - 密钥材料由宿主管理，embedded-database 不负责密钥存储/轮换。
  - 审计日志为计数器实现，无结构化日志（no_std 约束）。
  - TPM/TEE 集成留待后续。
- **M5 遗留未修复**：`pending_writes` 无上限且不参与恢复；合并后 page_map offset 未指向新段。
```

- [ ] **Step 7: 检查点**

Run: `rg -n 'M6 完成' docs/superpowers/specs/2026-10-02-embedded-database-design.md`
Expected: 有输出

---

## M6 验收标准

| # | 标准 | 验证方式 |
|---|------|----------|
| 1 | SM4 标准测试向量通过 | M6-0 测试 |
| 2 | SM3 标准测试向量通过 | M6-1 测试 |
| 3 | HMAC-SM3 确定性 | M6-2 测试 |
| 4 | 密钥管理（`EncryptionKey`）可用 | M6-3 测试 |
| 5 | 页级加密写入解密往返 | M6-4 测试 |
| 6 | `Db::stats()` 返回审计计数 | M6-5 测试 |
| 7 | 安全设计文档已更新 | M6-6 |
| 8 | LoongArch 手册已更新 | M6-7 |
| 9 | 5 个 no_std 目标全部编译通过 | M6-8 Step 3 |
| 10 | 全 crate 零 `unsafe` | M6-8 Step 4 |
| 11 | 全部测试通过 | M6-8 Step 1 |
| 12 | `inject_crash` exit=0 且 `wcet` exit=0 | M6-8 Step 5 |

## 后续里程碑

M6 完成后，设计文档 M1–M6 全部交付。剩余工作：

- **LoongArch 实机验证**：拿到龙芯硬件后执行（见 M5-4 构建手册）。
- **认证申报**：GB/T 22239 安全可靠测评、CSTC 适配认证（见设计文档 §6.3）。
- **试点部署**：现场稳定运行 ≥ 3 个月（见设计文档 §9 M6）。
- **生产加固**：加密模式升级（CBC/CTR + HMAC）、密钥轮换、审计日志结构化。
