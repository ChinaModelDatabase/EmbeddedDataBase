//! CRC-32C（Castagnoli）与事务级 CRC 链。
//!
//! 必须软件查表：x86 SSE4.2 与 ARM CRC 指令时延可变，
//! 违反确定性要求（spec §5.4、约束 C9）。

#[cfg(test)]
mod tests {
    use super::*;
    // no_std crate：Vec / format! 需要显式引入
    use std::format;
    use std::vec::Vec;

    #[test]
    fn 默认值等于新建() {
        // 防止有人把 derive(Default) 改回来——那会让 CRC 静默算错
        assert_eq!(Crc32c::default().finish(), Crc32c::new().finish());
        assert_eq!(Crc32c::default().0, 0xFFFF_FFFF);
        assert_eq!(CrcChain::default().value(), CrcChain::new().value());
    }

    #[test]
    fn 标准检验值() {
        // CRC-32C 标准 check 值
        let mut c = Crc32c::new();
        c.update(b"123456789");
        assert_eq!(c.finish(), 0xE306_9283);
    }

    #[test]
    fn 空输入检验值为零() {
        let mut c = Crc32c::new();
        c.update(b"");
        assert_eq!(c.finish(), 0x0000_0000);
    }

    #[test]
    fn 增量等于一次性() {
        let mut a = Crc32c::new();
        a.update(b"hello ");
        a.update(b"world");
        let mut b = Crc32c::new();
        b.update(b"hello world");
        assert_eq!(a.finish(), b.finish());
    }

    #[test]
    fn 任意切分结果一致() {
        let data: Vec<u8> = (0..=255u8).collect();
        let mut whole = Crc32c::new();
        whole.update(&data);
        for split in 0..=data.len() {
            let mut c = Crc32c::new();
            c.update(&data[..split]);
            c.update(&data[split..]);
            assert_eq!(c.finish(), whole.finish(), "split={split}");
        }
    }

    #[test]
    fn 单比特翻转改变结果() {
        let data: [u8; 64] = core::array::from_fn(|i| i as u8);
        let mut base = Crc32c::new();
        base.update(&data);
        let base_crc = base.finish();
        for bit in 0..8usize {
            let mut d = data;
            d[0] ^= 1 << bit;
            let mut c = Crc32c::new();
            c.update(&d);
            assert_ne!(c.finish(), base_crc, "bit={bit}");
        }
    }

    #[test]
    fn 链对顺序敏感() {
        let mut a = CrcChain::new();
        a.extend(1);
        a.extend(2);
        let mut b = CrcChain::new();
        b.extend(2);
        b.extend(1);
        assert_ne!(a.value(), b.value());
    }

    #[test]
    fn 链传播上游扰动() {
        // 仅最后一个段被改动，也必须导致链值变化
        let mut a = CrcChain::new();
        a.extend(0xAAAA_AAAA);
        a.extend(0xBBBB_BBBB);
        let mut b = CrcChain::new();
        b.extend(0xAAAA_AAAA);
        b.extend(0xBBBB_3333);
        assert_ne!(a.value(), b.value());
    }

    #[test]
    fn 链传播上游扰动_改首段() {
        // 仅第一段被改动、后续段保持不变，也必须导致链值变化。
        // 这条不变量（spec §7.2）是 CrcChain 存在的唯一理由，而上面那条
        // 「改末段」用例覆盖不到它：若 extend 忘了把 self.0 喂进 CRC，
        // 链就退化成「只看最后一个段」，此时改末段与顺序敏感仍照样通过。
        let mut a = CrcChain::new();
        a.extend(1);
        a.extend(2);
        a.extend(3);
        let mut b = CrcChain::new();
        b.extend(99);
        b.extend(2);
        b.extend(3);
        assert_ne!(a.value(), b.value());
    }

    #[test]
    fn 折叠顺序敏感() {
        assert_ne!(fold_crcs(&[1, 2, 3]), fold_crcs(&[3, 2, 1]));
        // 空折叠不得与单段折叠撞上，否则空链会被当成含一段的链
        assert_ne!(fold_crcs(&[]), fold_crcs(&[1]));
    }

    #[test]
    fn 哈希确定且区分大小写() {
        assert_ne!(
            hash_bytes(b"embedded-database"),
            hash_bytes(b"embedded-databasE")
        );
        // 空输入是 FNV 基址本身，不得与任何非空输入撞上。
        // 「确定性」由下面的黄金值锁定用例承担——把同一输入和自身比较
        // 是恒真的断言，改哪个常量都不会失败。
        assert_ne!(hash_bytes(b""), hash_bytes(b"embedded-database"));
    }

    #[test]
    fn 黄金值锁定_哈希与折叠() {
        // 这些数值被 Task 4 的哈希表桶定位、以及已落盘段的重新定位依赖：
        // FNV 基址或 CRC 常量一旦漂移，旧数据将无法被定位回原桶。
        // 因此钉死具体数值，而不是只断言「确定」「区分大小写」。
        assert_eq!(hash_bytes(b""), 0x811C_9DC5);
        assert_eq!(hash_bytes(b"embedded-database"), 0x491E_C47B);
        assert_eq!(fold_crcs(&[]), 0x0000_0000);
        assert_eq!(fold_crcs(&[1, 2]), 0x7A73_EC2C);
    }

    #[test]
    fn 黄金值锁定_链() {
        // 同上：链值会随折叠值一起落盘，必须与实现保持一致
        let mut chain = CrcChain::new();
        chain.extend(1);
        chain.extend(2);
        assert_eq!(chain.value(), 0xCC66_6D5B);
    }

    #[test]
    fn 顺序键不产生极端冲突() {
        // FNV-1a 对 "k0".."k999" 的低位分布回归防线。
        // 分布过差会让线性探测退化，违反确定性预期。
        let mut buckets = [0usize; 1024];
        for i in 0..1000u32 {
            let h = hash_bytes(format!("k{i}").as_bytes()) as usize;
            buckets[h % 1024] += 1;
        }
        let max = buckets.iter().copied().max().unwrap_or(0);
        assert!(max < 10, "最大桶 {max} 过多，冲突严重");
    }
}

/// 反射后的 Castagnoli 多项式。
const POLY: u32 = 0x82F6_3B78;

const fn build_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0usize;
    while i < 256 {
        let mut crc = i as u32;
        let mut j = 0usize;
        while j < 8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ POLY
            } else {
                crc >> 1
            };
            j += 1;
        }
        table[i] = crc;
        i += 1;
    }
    table
}

static TABLE: [u32; 256] = build_table();

/// 增量 CRC-32C 计算器。
///
/// **不要用 `#[derive(Default)]`**——派生实现会给出 `Crc32c(0)`，
/// 而正确的初始值是 `0xFFFF_FFFF`（CRC 的预置初值）。
/// 这是一个静默产生错误校验结果的陷阱，因此手写 `Default`。
#[derive(Debug, Clone, Copy)]
pub struct Crc32c(u32);

impl Default for Crc32c {
    fn default() -> Self {
        Self::new()
    }
}

impl Crc32c {
    pub const fn new() -> Self {
        Crc32c(0xFFFF_FFFF)
    }

    pub fn update(&mut self, data: &[u8]) {
        let mut crc = self.0;
        for &b in data {
            crc = (crc >> 8) ^ TABLE[((crc ^ u32::from(b)) & 0xFF) as usize];
        }
        self.0 = crc;
    }

    pub const fn finish(&self) -> u32 {
        self.0 ^ 0xFFFF_FFFF
    }
}

/// 段级 CRC 链：把每段的 fold 值纳入前一段的 CRC，形成链式校验。
///
/// 作用：防止"段 N-2 被静默损坏但 N-1、N 看似正常"（spec §7.2）。
///
/// 此处 `derive(Default)` 是安全的：`CrcChain::new() == 0` 恰好等于派生默认值。
/// `Crc32c` 则不同——CRC 的预置初值是 `0xFFFF_FFFF`，派生会给出 `0` 从而静默算错，
/// 所以它必须手写 `Default`，见本文件上方 `Crc32c` 的文档注释。
#[derive(Debug, Clone, Copy, Default)]
pub struct CrcChain(u32);

impl CrcChain {
    pub const fn new() -> Self {
        CrcChain(0)
    }

    pub fn extend(&mut self, page_crc_fold: u32) {
        let mut c = Crc32c::new();
        c.update(&self.0.to_le_bytes());
        c.update(&page_crc_fold.to_le_bytes());
        self.0 = c.finish();
    }

    pub const fn value(&self) -> u32 {
        self.0
    }
}

/// 把一组 CRC 折叠为单个 u32。顺序敏感。
pub fn fold_crcs(crcs: &[u32]) -> u32 {
    let mut c = Crc32c::new();
    for v in crcs {
        c.update(&v.to_le_bytes());
    }
    c.finish()
}

/// FNV-1a 32 位哈希。固定实现——无随机种子、无地址依赖，
/// 保证同样的键在任何平台得到同样的桶位置。
pub fn hash_bytes(data: &[u8]) -> u32 {
    let mut h: u32 = 0x811c_9dc5;
    for &b in data {
        h ^= u32::from(b);
        h = h.wrapping_mul(0x0100_0193);
    }
    h
}
