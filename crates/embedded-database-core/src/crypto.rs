//! 加密密钥管理（M6-3）。

use crate::sm4::Sm4Key;

#[cfg(feature = "std")]
use crate::hmac::hmac_sm3;
#[cfg(feature = "std")]
use crate::sm4::{derive_iv, sm4_cbc_decrypt, sm4_cbc_encrypt};
#[cfg(feature = "std")]
use crate::Error;

/// 加密密钥。密钥材料由宿主管理，不落盘。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncryptionKey {
    pub sm4: Sm4Key,
    pub hmac: [u8; 32],
}

#[cfg(feature = "std")]
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
    #[allow(non_snake_case)]
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
        assert!(
            decrypt_page(&key, 1, &tampered).is_err(),
            "篡改应被 HMAC 检出"
        );
    }

    #[test]
    fn 加密页_错误page_id解密失败() {
        let key = EncryptionKey {
            sm4: crate::sm4::Sm4Key { bytes: [0x01; 16] },
            hmac: [0x02; 32],
        };
        let ct = encrypt_page(&key, 1, b"data");
        // 用错误 page_id（不同 IV）解密应失败
        assert!(
            decrypt_page(&key, 2, &ct).is_err(),
            "错误 IV 应导致解密失败"
        );
    }
}
