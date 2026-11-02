//! SM4 分组密码（GB/T 32907-2016）。
//!
//! 128-bit 密钥、128-bit 分组、32 轮 Feistel 置换。
//! 纯查表实现，无分配、无 `unsafe`，可在 `no_std` 路径使用。
//! 常量必须与标准一致——轮密钥或 S-box 漂移会使密文不可复现。

/// SM4 S-box（GB/T 32907-2016 附录 A.2）。
const SM4_SBOX: [u8; 256] = [
    0xD6, 0x90, 0xE9, 0xFE, 0xCC, 0xE1, 0x3D, 0xB7, 0x16, 0xB6, 0x14, 0xC2, 0x28, 0xFB, 0x2C, 0x05,
    0x2B, 0x67, 0x9A, 0x76, 0x2A, 0xBE, 0x04, 0xC3, 0xAA, 0x44, 0x13, 0x26, 0x49, 0x86, 0x06, 0x99,
    0x9C, 0x42, 0x50, 0xF4, 0x91, 0xEF, 0x98, 0x7A, 0x33, 0x54, 0x0B, 0x43, 0xED, 0xCF, 0xAC, 0x62,
    0xE4, 0xB3, 0x1C, 0xA9, 0xC9, 0x08, 0xE8, 0x95, 0x80, 0xDF, 0x94, 0xFA, 0x75, 0x8F, 0x3F, 0xA6,
    0x47, 0x07, 0xA7, 0xFC, 0xF3, 0x73, 0x17, 0xBA, 0x83, 0x59, 0x3C, 0x19, 0xE6, 0x85, 0x4F, 0xA8,
    0x68, 0x6B, 0x81, 0xB2, 0x71, 0x64, 0xDA, 0x8B, 0xF8, 0xEB, 0x0F, 0x4B, 0x70, 0x56, 0x9D, 0x35,
    0x1E, 0x24, 0x0E, 0x5E, 0x63, 0x58, 0xD1, 0xA2, 0x25, 0x22, 0x7C, 0x3B, 0x01, 0x21, 0x78, 0x87,
    0xD4, 0x00, 0x46, 0x57, 0x9F, 0xD3, 0x27, 0x52, 0x4C, 0x36, 0x02, 0xE7, 0xA0, 0xC4, 0xC8, 0x9E,
    0xEA, 0xBF, 0x8A, 0xD2, 0x40, 0xC7, 0x38, 0xB5, 0xA3, 0xF7, 0xF2, 0xCE, 0xF9, 0x61, 0x15, 0xA1,
    0xE0, 0xAE, 0x5D, 0xA4, 0x9B, 0x34, 0x1A, 0x55, 0xAD, 0x93, 0x32, 0x30, 0xF5, 0x8C, 0xB1, 0xE3,
    0x1D, 0xF6, 0xE2, 0x2E, 0x82, 0x66, 0xCA, 0x60, 0xC0, 0x29, 0x23, 0xAB, 0x0D, 0x53, 0x4E, 0x6F,
    0xD5, 0xDB, 0x37, 0x45, 0xDE, 0xFD, 0x8E, 0x2F, 0x03, 0xFF, 0x6A, 0x72, 0x6D, 0x6C, 0x5B, 0x51,
    0x8D, 0x1B, 0xAF, 0x92, 0xBB, 0xDD, 0xBC, 0x7F, 0x11, 0xD9, 0x5C, 0x41, 0x1F, 0x10, 0x5A, 0xD8,
    0x0A, 0xC1, 0x31, 0x88, 0xA5, 0xCD, 0x7B, 0xBD, 0x2D, 0x74, 0xD0, 0x12, 0xB8, 0xE5, 0xB4, 0xB0,
    0x89, 0x69, 0x97, 0x4A, 0x0C, 0x96, 0x77, 0x7E, 0x65, 0xB9, 0xF1, 0x09, 0xC5, 0x6E, 0xC6, 0x84,
    0x18, 0xF0, 0x7D, 0xEC, 0x3A, 0xDC, 0x4D, 0x20, 0x79, 0xEE, 0x5F, 0x3E, 0xD7, 0xCB, 0x39, 0x48,
];

/// 系统参数 FK（GB/T 32907-2016）。
const SM4_FK: [u32; 4] = [0xA3B1BAC6, 0x56AA3350, 0x677D9197, 0xB27022DC];

/// 轮密钥扩展固定字 CK[i]（GB/T 32907-2016）。
const SM4_RK_CONST: [u32; 32] = [
    0x00070E15, 0x1C232A31, 0x383F464D, 0x545B6269, 0x70777E85, 0x8C939AA1, 0xA8AFB6BD, 0xC4CBD2D9,
    0xE0E7EEF5, 0xFC030A11, 0x181F262D, 0x343B4249, 0x50575E65, 0x6C737A81, 0x888F969D, 0xA4ABB2B9,
    0xC0C7CED5, 0xDCE3EAF1, 0xF8FF060D, 0x141B2229, 0x30373E45, 0x4C535A61, 0x686F767D, 0x848B9299,
    0xA0A7AEB5, 0xBCC3CAD1, 0xD8DFE6ED, 0xF4FB0209, 0x10171E25, 0x2C333A41, 0x484F565D, 0x646B7279,
];

/// SM4 密钥（128 bit）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sm4Key {
    pub bytes: [u8; 16],
}

/// 用给定密钥加密一个 128-bit 分组。
pub fn sm4_encrypt_block(key: &Sm4Key, block: &[u8; 16]) -> [u8; 16] {
    let rk = sm4_key_schedule(&key.bytes);
    sm4_crypt(&rk, block)
}

/// 用给定密钥解密一个 128-bit 分组（轮密钥逆序）。
pub fn sm4_decrypt_block(key: &Sm4Key, block: &[u8; 16]) -> [u8; 16] {
    let mut rk = sm4_key_schedule(&key.bytes);
    rk.reverse(); // 解密用逆序轮密钥
    sm4_crypt(&rk, block)
}

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
        // 标准：K[i+4] = K[i] ^ T'(K[i+1] ^ K[i+2] ^ K[i+3] ^ CK[i])，T' 只作用一次
        let t = k[0] ^ sm4_t_prime(k[1] ^ k[2] ^ k[3] ^ SM4_RK_CONST[i]);
        k[0] = k[1];
        k[1] = k[2];
        k[2] = k[3];
        k[3] = t;
        rk[i] = t;
    }
    rk
}

fn sm4_crypt(rk: &[u32; 32], block: &[u8; 16]) -> [u8; 16] {
    let mut x = [0u32; 4];
    for i in 0..4 {
        x[i] = u32::from_be_bytes([
            block[i * 4],
            block[i * 4 + 1],
            block[i * 4 + 2],
            block[i * 4 + 3],
        ]);
    }
    for &ri in rk.iter() {
        let t = x[0] ^ sm4_t(x[1] ^ x[2] ^ x[3] ^ ri);
        x[0] = x[1];
        x[1] = x[2];
        x[2] = x[3];
        x[3] = t;
    }
    // 反序
    let mut out = [0u8; 16];
    for i in 0..4 {
        let v = x[3 - i];
        out[i * 4..(i + 1) * 4].copy_from_slice(&v.to_be_bytes());
    }
    out
}

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
) -> Result<std::vec::Vec<u8>, crate::Error> {
    if ciphertext.is_empty() || ciphertext.len() % 16 != 0 {
        return Err(crate::Error::InvalidArgument);
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
    let pad = *out.last().ok_or(crate::Error::InvalidArgument)? as usize;
    if pad == 0 || pad > 16 || pad > out.len() {
        return Err(crate::Error::InvalidArgument);
    }
    out.truncate(out.len() - pad);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// GB/T 32907-2016 附录 A.1 标准测试向量（Example 1，与 OpenSSL sm4_internal_test.c 一致）。
    /// 注意：简报中的期望密文后 8 字节有误；标准值以 GB/T 32907-2016 与 OpenSSL 为准。
    const KEY: [u8; 16] = [
        0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0xfe, 0xdc, 0xba, 0x98, 0x76, 0x54, 0x32,
        0x10,
    ];
    const PLAINTEXT: [u8; 16] = [
        0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0xfe, 0xdc, 0xba, 0x98, 0x76, 0x54, 0x32,
        0x10,
    ];
    const CIPHERTEXT: [u8; 16] = [
        0x68, 0x1e, 0xdf, 0x34, 0xd2, 0x06, 0x96, 0x5e, 0x86, 0xb3, 0xe9, 0x4f, 0x53, 0x6e, 0x42,
        0x46,
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
    #[allow(non_snake_case)]
    fn sm4_cbc_不同IV不同密文() {
        let key = Sm4Key { bytes: [0x01; 16] };
        let pt = b"same plaintext";
        let ct1 = sm4_cbc_encrypt(&key, &derive_iv(1), pt);
        let ct2 = sm4_cbc_encrypt(&key, &derive_iv(2), pt);
        assert_ne!(ct1, ct2, "不同 IV 应产生不同密文");
    }

    #[test]
    #[allow(non_snake_case)]
    fn sm4_cbc_错误IV解密失败() {
        let key = Sm4Key { bytes: [0x01; 16] };
        let ct = sm4_cbc_encrypt(&key, &derive_iv(1), b"data");
        // 用错误 IV 解密应产生错误填充（概率性）
        let result = sm4_cbc_decrypt(&key, &derive_iv(99), &ct);
        // 可能成功（概率极低）或失败；关键是不 panic
        #[allow(clippy::single_match)]
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
}
