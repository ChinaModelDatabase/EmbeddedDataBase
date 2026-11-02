//! SM3 哈希（GB/T 32905-2016）。
//!
//! 256-bit 输出，Merkle-Damgård 结构，纯位运算实现，
//! 无分配、无 `unsafe`，可在 `no_std` 路径使用。
//! 常量与轮函数必须与标准一致——任何漂移都会使哈希不可复现。

/// SM3 初始向量 IV（GB/T 32905-2016）。
const SM3_IV: [u32; 8] = [
    0x7380166F, 0x4914B2B9, 0x172442D7, 0xDA8A0600, 0xA96F30BC, 0x163138AA, 0xE38DEE4D, 0xB0FB0E4E,
];

/// 可增量喂入的 SM3 状态（`pub(crate)`，供 HMAC 等拼接分段，无需分配）。
#[derive(Debug, Clone)]
pub(crate) struct Sm3State {
    v: [u32; 8],
    block: [u8; 64],
    block_len: usize,
    total_len: u64,
}

impl Sm3State {
    pub(crate) fn new() -> Self {
        Self {
            v: SM3_IV,
            block: [0u8; 64],
            block_len: 0,
            total_len: 0,
        }
    }

    /// 按序喂入一段消息；等价于拼接后一次哈希。
    pub(crate) fn update(&mut self, mut data: &[u8]) {
        self.total_len = self.total_len.wrapping_add(data.len() as u64);

        if self.block_len > 0 {
            let need = 64 - self.block_len;
            let take = need.min(data.len());
            self.block[self.block_len..self.block_len + take].copy_from_slice(&data[..take]);
            self.block_len += take;
            data = &data[take..];
            if self.block_len == 64 {
                let block = self.block;
                sm3_compress(&mut self.v, &block);
                self.block_len = 0;
            }
        }

        while data.len() >= 64 {
            sm3_compress(&mut self.v, &data[..64]);
            data = &data[64..];
        }

        if !data.is_empty() {
            self.block[..data.len()].copy_from_slice(data);
            self.block_len = data.len();
        }
    }

    /// 结束增量过程并输出摘要。填充规则与 GB/T 32905-2016 一致。
    pub(crate) fn finalize(mut self) -> [u8; 32] {
        let rem = self.block_len;
        self.block[rem] = 0x80;
        for b in self.block[rem + 1..].iter_mut() {
            *b = 0;
        }
        if rem >= 56 {
            let block = self.block;
            sm3_compress(&mut self.v, &block);
            self.block = [0u8; 64];
        }
        let bit_len = self.total_len.wrapping_mul(8);
        self.block[56..64].copy_from_slice(&bit_len.to_be_bytes());
        sm3_compress(&mut self.v, &self.block);

        let mut out = [0u8; 32];
        for (i, word) in self.v.iter().enumerate() {
            out[i * 4..(i + 1) * 4].copy_from_slice(&word.to_be_bytes());
        }
        out
    }
}

impl Default for Sm3State {
    fn default() -> Self {
        Self::new()
    }
}

/// 计算消息的 SM3 哈希（GB/T 32905-2016）。
pub fn sm3_hash(data: &[u8]) -> [u8; 32] {
    let mut state = Sm3State::new();
    state.update(data);
    state.finalize()
}

fn sm3_compress(v: &mut [u32; 8], block: &[u8]) {
    let mut w = [0u32; 68];
    let mut wp = [0u32; 64];
    for i in 0..16 {
        w[i] = u32::from_be_bytes([
            block[i * 4],
            block[i * 4 + 1],
            block[i * 4 + 2],
            block[i * 4 + 3],
        ]);
    }
    for j in 16..68 {
        w[j] = sm3_p1(w[j - 16] ^ w[j - 9] ^ w[j - 3].rotate_left(15))
            ^ w[j - 13].rotate_left(7)
            ^ w[j - 6];
    }
    for j in 0..64 {
        wp[j] = w[j] ^ w[j + 4];
    }

    let (mut a, mut b, mut c, mut d) = (v[0], v[1], v[2], v[3]);
    let (mut e, mut f, mut g, mut h) = (v[4], v[5], v[6], v[7]);

    for j in 0..64 {
        let tj = if j < 16 { 0x79CC4519u32 } else { 0x7A879D8Au32 };
        let a12 = a.rotate_left(12);
        let ss1 = a12
            .wrapping_add(e)
            .wrapping_add(tj.rotate_left(j as u32))
            .rotate_left(7);
        let ss2 = ss1 ^ a12;
        let ff = if j < 16 {
            a ^ b ^ c
        } else {
            (a & b) | (a & c) | (b & c)
        };
        let gg = if j < 16 {
            e ^ f ^ g
        } else {
            (e & f) | (!e & g)
        };
        let tt1 = ff.wrapping_add(d).wrapping_add(ss2).wrapping_add(wp[j]);
        let tt2 = gg.wrapping_add(h).wrapping_add(ss1).wrapping_add(w[j]);
        d = c;
        c = b.rotate_left(9);
        b = a;
        a = tt1;
        h = g;
        g = f.rotate_left(19);
        f = e;
        e = sm3_p0(tt2);
    }

    v[0] ^= a;
    v[1] ^= b;
    v[2] ^= c;
    v[3] ^= d;
    v[4] ^= e;
    v[5] ^= f;
    v[6] ^= g;
    v[7] ^= h;
}

fn sm3_p0(x: u32) -> u32 {
    x ^ x.rotate_left(9) ^ x.rotate_left(17)
}

fn sm3_p1(x: u32) -> u32 {
    x ^ x.rotate_left(15) ^ x.rotate_left(23)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// GB/T 32905-2016 附录 A 标准测试向量。
    /// 简报中 `"abc"` 期望值与标准一致；空消息期望值简报有误，以标准为准。
    const TEST_MSG: &[u8] = b"abc";
    const TEST_HASH: [u8; 32] = [
        0x66, 0xC7, 0xF0, 0xF4, 0x62, 0xEE, 0xED, 0xD9, 0xD1, 0xF2, 0xD4, 0x6B, 0xDC, 0x10, 0xE4,
        0xE2, 0x41, 0x67, 0xC4, 0x87, 0x5C, 0xF2, 0xF7, 0xA2, 0x29, 0x7D, 0xA0, 0x2B, 0x8F, 0x4B,
        0xA8, 0xE0,
    ];

    #[test]
    fn sm3_标准测试向量() {
        let h = sm3_hash(TEST_MSG);
        assert_eq!(h, TEST_HASH);
    }

    #[test]
    fn sm3_空消息() {
        // GB/T 32905-2016 / OpenSSL SM3("").
        // 简报中的期望值为笔误，此处以标准与 OpenSSL 交叉验证值为准。
        const EMPTY_HASH: [u8; 32] = [
            0x1A, 0xB2, 0x1D, 0x83, 0x55, 0xCF, 0xA1, 0x7F, 0x8E, 0x61, 0x19, 0x48, 0x31, 0xE8,
            0x1A, 0x8F, 0x22, 0xBE, 0xC8, 0xC7, 0x28, 0xFE, 0xFB, 0x74, 0x7E, 0xD0, 0x35, 0xEB,
            0x50, 0x82, 0xAA, 0x2B,
        ];
        let h = sm3_hash(b"");
        assert_eq!(h, EMPTY_HASH);
    }

    #[test]
    fn sm3_确定性() {
        assert_eq!(sm3_hash(b"hello"), sm3_hash(b"hello"));
        assert_ne!(sm3_hash(b"hello"), sm3_hash(b"world"));
    }
}
