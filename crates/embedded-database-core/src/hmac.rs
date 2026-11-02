//! HMAC-SM3（RFC 2104）。
//!
//! `HMAC(K, m) = H((K ^ opad) || H((K ^ ipad) || m))`，其中 `H` 为 SM3。
//! 密钥长度超过分组大小（64 字节）时先哈希再使用。
//! 纯位运算、无分配、无 `unsafe`，可在 `no_std` 路径使用。

use crate::sm3::{sm3_hash, Sm3State};

/// SM3-HMAC 分组大小（字节）。
const BLOCK_SIZE: usize = 64;

/// SM3-HMAC 摘要长度（字节）。
const DIGEST_SIZE: usize = 32;

/// 计算消息的 HMAC-SM3（RFC 2104）。
///
/// 无分配、无 `unsafe`：内部用定长 ipad/opad 与增量 SM3 状态，
/// 避免为 `ipad || data` 构造变长中间缓冲。
pub fn hmac_sm3(key: &[u8], data: &[u8]) -> [u8; DIGEST_SIZE] {
    let mut k = [0u8; BLOCK_SIZE];
    if key.len() > BLOCK_SIZE {
        let h = sm3_hash(key);
        k[..DIGEST_SIZE].copy_from_slice(&h);
    } else {
        k[..key.len()].copy_from_slice(key);
    }

    let mut ipad = [0u8; BLOCK_SIZE];
    for (i, ki) in k.iter().enumerate() {
        ipad[i] = ki ^ 0x36;
    }
    let mut inner = Sm3State::new();
    inner.update(&ipad);
    inner.update(data);
    let inner_hash = inner.finalize();

    let mut opad = [0u8; BLOCK_SIZE];
    for (i, ki) in k.iter().enumerate() {
        opad[i] = ki ^ 0x5C;
    }
    let mut outer = Sm3State::new();
    outer.update(&opad);
    outer.update(&inner_hash);
    outer.finalize()
}

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
