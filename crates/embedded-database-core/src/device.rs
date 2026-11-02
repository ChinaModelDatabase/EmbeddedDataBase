//! 存储设备抽象与可故障注入的内存实现。

use crate::{Error, Result};

/// 固定尺寸的块设备抽象。
///
/// 实现方必须满足：
/// - 所有偏移越界返回 `Error::InvalidArgument`，**不得 panic**。
/// - `write_at` 后数据对 `read_at` 立即可见（写穿透语义）；
///   掉电安全性由 `sync()` 界定。
/// - `sync()` 保证此前写入已落到稳定介质，返回后才算持久。
/// - 永不阻塞、永不自旋。设备忙时返回 `Error::Busy`。
pub trait BlockDevice {
    /// 设备容量（字节）。
    fn size(&self) -> u64;

    /// 读 `buf.len()` 字节到 `buf`。`offset + buf.len()` 越界则失败。
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()>;

    /// 从 `data` 写 `data.len()` 字节。
    fn write_at(&mut self, offset: u64, data: &[u8]) -> Result<()>;

    /// 将此前写入刷到稳定介质。持久性边界。
    fn sync(&mut self) -> Result<()>;

    /// 通知设备 `[offset, offset + len)` 的内容已作废，可回收（TRIM/discard）。
    ///
    /// 默认实现是空操作，既有实现无需改动。设备**不得**因 discard 改变
    /// 读出的数据（语义只是"提示可回收"），越界失败返回
    /// [`Error::InvalidArgument`]。
    fn discard(&mut self, offset: u64, len: u64) -> Result<()> {
        let end = offset.checked_add(len).ok_or(Error::InvalidArgument)?;
        if end > self.size() {
            return Err(Error::InvalidArgument);
        }
        let _ = (offset, len);
        Ok(())
    }
}

/// 内存设备，支持按需注入数据损坏与写失败，用于崩溃恢复测试。
///
/// 承载 `Vec<u8>`，因此仅在 `std` 下可用。
#[cfg(feature = "std")]
#[derive(Debug, Clone)]
pub struct InMemoryDevice {
    data: std::vec::Vec<u8>,
    /// (offset, len)：这些区间每次写入都会被篡改，模拟位翻转。
    corrupt: std::vec::Vec<(usize, usize)>,
    /// 剩余多少次写操作后开始失败。`None` 表示不注入失败。
    fail_after: std::option::Option<u64>,
    writes_seen: u64,
    /// 写高水位：历史 `write_at` 触及的最大结束偏移（`start + len`）。
    /// 高水位语义，从 0 起随成功写入单调不减——不是「还能写多少字节」。
    writable_bytes: usize,
    stats: WriteStats,
    discarded: std::vec::Vec<(u64, u64)>,
}

/// 写操作计数，用于验证故障注入确实生效。
#[cfg(feature = "std")]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WriteStats {
    pub successful_writes: u64,
    pub failed_writes: u64,
}

#[cfg(feature = "std")]
impl InMemoryDevice {
    /// 创建指定容量的内存设备。
    pub fn new(size: usize) -> InMemoryDevice {
        InMemoryDevice {
            data: std::vec![0u8; size],
            corrupt: std::vec::Vec::new(),
            fail_after: std::option::Option::None,
            writes_seen: 0,
            writable_bytes: 0,
            stats: WriteStats::default(),
            discarded: std::vec::Vec::new(),
        }
    }

    /// 把 `[offset, offset+len)` 标记为损坏区：每次写入该区都会写错。
    pub fn corrupt_at(&mut self, offset: usize, len: usize) {
        self.corrupt.push((offset, len));
    }

    /// 注入写失败：成功 `n` 次写之后，所有写返回 `Error::Io`。
    pub fn inject_write_failure(&mut self, after_writes: u64) {
        self.fail_after = std::option::Option::Some(after_writes);
        self.writes_seen = 0;
    }

    pub fn clear_failures(&mut self) {
        self.fail_after = std::option::Option::None;
    }

    /// 只读打开设备：所有写返回 `Error::Io`。
    pub fn set_read_only(&mut self) {
        self.fail_after = std::option::Option::Some(0);
    }

    pub fn stats(&self) -> WriteStats {
        self.stats
    }

    /// 写高水位：历史写入触及的最大结束偏移。
    ///
    /// 高水位语义：从 0 开始，每次成功写入后取 `max(writable_bytes, end)`。
    /// 用于恢复测试断言写入范围，不是「还能写多少字节」。
    pub fn writable_bytes(&self) -> usize {
        self.writable_bytes
    }

    /// 直接读取原始字节，供恢复测试断言段内容。
    pub fn raw(&self) -> &[u8] {
        &self.data
    }

    /// 可变原始字节访问，仅供故障注入测试使用。
    pub fn raw_mut(&mut self) -> &mut [u8] {
        &mut self.data
    }

    /// 已收到的 discard 区间（按调用顺序），供测试断言。
    pub fn discarded_ranges(&self) -> &[(u64, u64)] {
        &self.discarded
    }
}

#[cfg(feature = "std")]
impl BlockDevice for InMemoryDevice {
    fn size(&self) -> u64 {
        self.data.len() as u64
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        let start = usize::try_from(offset).map_err(|_| Error::InvalidArgument)?;
        let end = start.checked_add(buf.len()).ok_or(Error::InvalidArgument)?;
        if end > self.data.len() {
            return Err(Error::InvalidArgument);
        }
        buf.copy_from_slice(&self.data[start..end]);
        Ok(())
    }

    fn write_at(&mut self, offset: u64, data: &[u8]) -> Result<()> {
        let start = usize::try_from(offset).map_err(|_| Error::InvalidArgument)?;
        let end = start
            .checked_add(data.len())
            .ok_or(Error::InvalidArgument)?;
        if end > self.data.len() {
            // ⚠️ 越界是**调用方编程错误**，不是设备故障，不得计入 failed_writes。
            // Task 7 的崩溃恢复要靠 stats 区分「设备写失败」与「上层算错了偏移」，
            // 把 InvalidArgument 混进 failed_writes 会让那个判断失去意义。
            return Err(Error::InvalidArgument);
        }

        if let Some(limit) = self.fail_after {
            if self.writes_seen >= limit {
                self.stats.failed_writes += 1;
                return Err(Error::Io);
            }
        }
        self.writes_seen += 1;

        self.data[start..end].copy_from_slice(data);
        self.writable_bytes = self.writable_bytes.max(end);

        // 注入损坏：翻转写入内容的每一位
        for (c_off, c_len) in &self.corrupt {
            let lo = start.max(*c_off);
            let hi = end.min((*c_off).saturating_add(*c_len));
            for i in lo..hi {
                self.data[i] ^= 0xFF;
            }
        }

        self.stats.successful_writes += 1;
        Ok(())
    }

    fn sync(&mut self) -> Result<()> {
        Ok(())
    }

    fn discard(&mut self, offset: u64, len: u64) -> Result<()> {
        let end = offset.checked_add(len).ok_or(Error::InvalidArgument)?;
        if end > self.size() {
            return Err(Error::InvalidArgument);
        }
        self.discarded.push((offset, len));
        Ok(())
    }
}

#[cfg(all(test, feature = "std"))]
mod tests {
    use super::*;

    #[test]
    fn 读回写入内容() {
        let mut d = InMemoryDevice::new(1024);
        d.write_at(10, b"EmbedDB").unwrap();
        let mut buf = [0u8; 7];
        d.read_at(10, &mut buf).unwrap();
        assert_eq!(&buf, b"EmbedDB");
        assert_eq!(d.size(), 1024);
    }

    #[test]
    fn 越界读写被拒() {
        let mut d = InMemoryDevice::new(16);
        assert_eq!(d.write_at(16, b"x"), Err(Error::InvalidArgument));
        let mut buf = [0u8; 4];
        assert_eq!(d.read_at(14, &mut buf), Err(Error::InvalidArgument));
    }

    #[test]
    fn 写注入失败_达阈值后开始报错() {
        let mut d = InMemoryDevice::new(64);
        d.inject_write_failure(2);
        assert!(d.write_at(0, b"a").is_ok());
        assert!(d.write_at(1, b"b").is_ok());
        assert_eq!(d.write_at(2, b"c"), Err(Error::Io));
        assert_eq!(
            d.stats(),
            WriteStats {
                successful_writes: 2,
                failed_writes: 1
            }
        );
    }

    #[test]
    fn 清除失败后恢复() {
        let mut d = InMemoryDevice::new(64);
        d.inject_write_failure(0);
        assert!(d.write_at(0, b"a").is_err());
        d.clear_failures();
        assert!(d.write_at(0, b"a").is_ok());
    }

    #[test]
    fn 损坏注入确实翻转数据() {
        let mut d = InMemoryDevice::new(64);
        d.corrupt_at(0, 8);
        d.write_at(0, b"embedded-database!").unwrap();
        assert_ne!(&d.raw()[..7], b"EmbedDB");
    }

    #[test]
    fn 越界写不计入设备失败计数() {
        // 越界是调用方 bug，不是设备故障；两者必须能在 stats 里区分开，
        // 否则 Task 7 无法判断该重试还是该上报 bug。
        let mut d = InMemoryDevice::new(16);
        assert_eq!(d.write_at(16, b"x"), Err(Error::InvalidArgument));
        assert_eq!(
            d.stats(),
            WriteStats {
                successful_writes: 0,
                failed_writes: 0
            }
        );
        // 真正的设备故障才计入
        d.inject_write_failure(0);
        assert_eq!(d.write_at(0, b"y"), Err(Error::Io));
        assert_eq!(d.stats().failed_writes, 1);
    }

    #[test]
    fn 只读模式拒绝一切写入() {
        let mut d = InMemoryDevice::new(16);
        d.write_at(0, b"ok").unwrap();
        d.set_read_only();
        assert_eq!(d.write_at(0, b"no"), Err(Error::Io));
        // 只读只挡写，读仍可用（恢复扫描依赖读）
        let mut buf = [0u8; 2];
        assert!(d.read_at(0, &mut buf).is_ok());
        assert_eq!(&buf, b"ok");
        // 被拒的写不得改动数据
        assert_eq!(&d.raw()[..2], b"ok");
    }

    #[test]
    fn 未触及末尾时高水位不虚高() {
        let mut d = InMemoryDevice::new(4096);
        d.write_at(0, b"x").unwrap();
        assert_eq!(d.writable_bytes(), 1);
    }

    #[test]
    fn discard_越界失败() {
        let mut dev = crate::device::InMemoryDevice::new(4096);
        assert!(dev.discard(0, 4096).is_ok());
        assert_eq!(dev.discard(0, 4097), Err(crate::Error::InvalidArgument));
        assert_eq!(dev.discard(4096, 1), Err(crate::Error::InvalidArgument));
    }

    #[test]
    fn 内存设备记录_discard_区间且不改数据() {
        let mut dev = InMemoryDevice::new(8192);
        dev.write_at(0, &[1, 2, 3, 4]).unwrap();
        dev.discard(0, 4).unwrap();
        dev.discard(4096, 4096).unwrap();
        let mut got = [0u8; 4];
        dev.read_at(0, &mut got).unwrap();
        assert_eq!(got, [1, 2, 3, 4]);
        assert_eq!(dev.discarded_ranges(), &[(0, 4), (4096, 4096)]);
    }
}
