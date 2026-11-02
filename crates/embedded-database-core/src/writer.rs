//! 段写入器：顺序追加变更页，最后写提交记录。

use crate::crc::{fold_crcs, Crc32c};
use crate::crypto::EncryptionKey;
use crate::device::BlockDevice;
use crate::segment::{
    CommitRecord, PageHeader, SegmentHeader, COMMIT_RECORD_SIZE, PAGE_HEADER_SIZE,
    SEGMENT_HEADER_SIZE,
};
use crate::slot::SlotAllocator;
use crate::Error;

/// 段写入配置。段布局在打开时确定，写入期不可变。
#[derive(Debug, Clone, Copy)]
pub struct SegmentConfig {
    pub segment_seq: u64,
    pub prev_segment: u64,
    pub prev_crc: u32,
    /// 段在设备上的起始偏移。多段场景下各段有独立偏移，
    /// 恢复扫描必须用它而非硬编码 0。
    pub device_offset: u64,
    pub segment_size: usize,
    pub page_size: u32,
    pub slots_per_page: u16,
    pub max_pages: usize,
    /// 页级 payload 加密密钥。`None` 时不加密（向后兼容）。
    pub encryption_key: Option<EncryptionKey>,
}

/// 一次页追加的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AppendedPage {
    pub page_id: u64,
    pub slot_index: u16,
    /// 该页在设备上的物理偏移。
    pub offset: u64,
    pub page_crc: u32,
}

/// 段写入器。
pub struct SegmentWriter<'d> {
    dev: &'d mut dyn BlockDevice,
    cfg: SegmentConfig,
    slots: SlotAllocator,
    /// 下一个可用物理槽的设备偏移。
    cursor: u64,
    page_crcs: std::vec::Vec<u32>,
    committed: bool,
}

impl core::fmt::Debug for SegmentWriter<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("SegmentWriter")
            .field("cfg", &self.cfg)
            .field("slots", &self.slots)
            .field("cursor", &self.cursor)
            .field("page_crcs", &self.page_crcs)
            .field("committed", &self.committed)
            .finish_non_exhaustive()
    }
}

impl<'d> SegmentWriter<'d> {
    pub fn new(dev: &'d mut dyn BlockDevice, cfg: SegmentConfig) -> Result<Self, Error> {
        if cfg.page_size < PAGE_HEADER_SIZE as u32
            || cfg.slots_per_page == 0
            || cfg.segment_size < SEGMENT_HEADER_SIZE + COMMIT_RECORD_SIZE
        {
            return Err(Error::InvalidArgument);
        }
        // C4：run_bytes / max_pages 乘积可能溢出，必须 checked（Config::validate
        // 覆盖同口径，但本函数是独立入口，须自洽校验）。
        let run_bytes = (cfg.page_size as usize)
            .checked_mul(cfg.slots_per_page as usize)
            .ok_or(Error::InvalidArgument)?;
        let total_run_bytes = cfg
            .max_pages
            .checked_mul(run_bytes)
            .ok_or(Error::InvalidArgument)?;

        // 页数据追加区起点：段头 + 预留槽区（每逻辑页 run_bytes）。
        // PageRun 表不写在这里——它写在段头之后的预留区首部，
        // 恢复扫描从 append_start 起，不会把槽表误读为页。
        // max_pages * run_bytes 已 checked_mul 校验；加常量必须 checked_add（C4）。
        let append_start = SEGMENT_HEADER_SIZE
            .checked_add(total_run_bytes)
            .ok_or(Error::InvalidArgument)?;
        let header = SegmentHeader::new(
            cfg.segment_seq,
            cfg.prev_segment,
            cfg.prev_crc,
            cfg.page_size,
            cfg.slots_per_page,
            SEGMENT_HEADER_SIZE as u64,
        );
        dev.write_at(cfg.device_offset, &header.encode())?;
        // 槽位复用防护（CRITICAL-1）：新段可能覆盖上一租期的段，段头之后的
        // PageRun 表预留区若残留陈旧条目，恢复扫描会把陈旧 (page_id, offset)
        // 登记进 page_map——指向已清零/已改写的区域，合并后导致
        // PageNotFound/错读。提交时只写前 committed 项，因此在建段时把整个
        // 表区清零：提交前表区全零（表结束标记语义不变），陈旧条目被清除。
        let run_table_bytes = cfg
            .max_pages
            .checked_mul(crate::segment::PAGE_RUN_ENTRY_SIZE)
            .ok_or(Error::InvalidArgument)?;
        {
            let chunk_size = core::cmp::max(cfg.page_size as usize, 4096);
            let zeros = std::vec![0u8; chunk_size];
            let mut pos = cfg.device_offset + SEGMENT_HEADER_SIZE as u64;
            let mut remaining = run_table_bytes;
            while remaining > 0 {
                let chunk = core::cmp::min(remaining, zeros.len());
                dev.write_at(pos, &zeros[..chunk])?;
                pos += chunk as u64;
                remaining -= chunk;
            }
        }
        // 槽位复用：新段可能覆盖上一租期更短的段，追加区残留旧页会让
        // 恢复扫描读到陈旧页（fold 不符或数据陈旧）。清零最多 max_pages 页
        // 的追加区（追加区只会写到这个长度）；小段配置下钳制到段尾，
        // 不越界。
        let zero_len = total_run_bytes.min(cfg.segment_size.saturating_sub(append_start));
        let chunk_size = core::cmp::max(cfg.page_size as usize, 4096);
        let zeros = std::vec![0u8; chunk_size];
        let mut pos = cfg.device_offset + append_start as u64;
        let mut remaining = zero_len;
        while remaining > 0 {
            let chunk = core::cmp::min(remaining, zeros.len());
            dev.write_at(pos, &zeros[..chunk])?;
            pos += chunk as u64;
            remaining -= chunk;
        }
        dev.sync()?;

        Ok(SegmentWriter {
            dev,
            cfg,
            slots: SlotAllocator::new(cfg.page_size, cfg.slots_per_page, cfg.max_pages)?,
            cursor: append_start as u64,
            page_crcs: std::vec::Vec::new(),
            committed: false,
        })
    }

    pub fn segment_seq(&self) -> u64 {
        self.cfg.segment_seq
    }

    pub fn cursor(&self) -> u64 {
        self.cursor
    }

    pub fn page_crcs(&self) -> &[u32] {
        &self.page_crcs
    }

    pub fn slots(&self) -> &SlotAllocator {
        &self.slots
    }

    /// 从已有追加状态恢复写入器（M12 同段追加）。
    ///
    /// 不重写段头（段头已在首次 `new` 时落盘）；从 `cursor` 继续追加。
    /// `page_crcs` / `slots` 为该段已累计的状态，保证 commit 的 fold
    /// 与 PageRun 表覆盖段内全部页。
    pub fn resume(
        dev: &'d mut dyn BlockDevice,
        cfg: SegmentConfig,
        cursor: u64,
        page_crcs: std::vec::Vec<u32>,
        slots: SlotAllocator,
    ) -> Result<Self, Error> {
        if cfg.page_size < PAGE_HEADER_SIZE as u32
            || cfg.slots_per_page == 0
            || cfg.segment_size < SEGMENT_HEADER_SIZE + COMMIT_RECORD_SIZE
        {
            return Err(Error::InvalidArgument);
        }
        // 槽表参数与配置一致性由调用方保证（Db 用同一 cfg 构造）。
        if cursor > cfg.segment_size as u64 {
            return Err(Error::InvalidArgument);
        }
        Ok(SegmentWriter {
            dev,
            cfg,
            slots,
            cursor,
            page_crcs,
            committed: false,
        })
    }

    /// 追加一个变更页。
    ///
    /// 写入页头 + 载荷 + 全槽零填充。零填充使槽 CRC 覆盖整个槽，
    /// 因此 padding 损坏也能被发现。
    pub fn append_page(&mut self, page_id: u64, payload: &[u8]) -> Result<AppendedPage, Error> {
        if self.committed {
            return Err(Error::InvalidArgument);
        }

        // M8：CBC + HMAC（IV = page_id 派生，HMAC 追加在密文尾部）。
        // 密文格式 = CBC密文 || HMAC-SM3，见 crypto::encrypt_page。
        let payload_enc = if let Some(key) = &self.cfg.encryption_key {
            crate::crypto::encrypt_page(key, page_id, payload)
        } else {
            payload.to_vec()
        };
        if payload_enc.len() > self.cfg.page_size as usize - PAGE_HEADER_SIZE {
            return Err(Error::InvalidArgument);
        }

        let alloc = self.slots.allocate(page_id)?;

        // M5：首次分配该页时设置槽组起始偏移（物理设备绝对偏移）。
        // 简报公式 SEGMENT_HEADER_SIZE + page_id * run_bytes 假设固定页布局，
        // 与本写入器的顺序追加区布局不符；此处用实际 cursor 位置。
        if alloc.slot_index == 0 {
            let page_run_off = self
                .cfg
                .device_offset
                .checked_add(self.cursor)
                .ok_or(Error::CapacityExhausted)?;
            self.slots.set_run_offset(page_id, page_run_off)?;
        }

        // 追加区必须给提交记录留出 40 字节
        let append_end = self
            .cursor
            .checked_add(u64::from(self.cfg.page_size))
            .ok_or(Error::QuotaExceeded)?;
        let commit_at = (self.cfg.segment_size - COMMIT_RECORD_SIZE) as u64;
        if append_end > commit_at {
            return Err(Error::QuotaExceeded);
        }

        let version = (self.page_crcs.len() + 1) as u32;
        let header = PageHeader::new(page_id, version, payload_enc.len() as u32);

        let mut slot = std::vec![0u8; self.cfg.page_size as usize];
        slot[..PAGE_HEADER_SIZE].copy_from_slice(&header.encode());
        slot[PAGE_HEADER_SIZE..PAGE_HEADER_SIZE + payload_enc.len()].copy_from_slice(&payload_enc);

        self.dev
            .write_at(self.cfg.device_offset + self.cursor, &slot)?;
        let mut c = Crc32c::new();
        c.update(&slot);
        let page_crc = c.finish();

        let offset = self.cfg.device_offset + self.cursor;
        self.cursor = append_end;
        self.page_crcs.push(page_crc);
        Ok(AppendedPage {
            page_id,
            slot_index: alloc.slot_index,
            offset,
            page_crc,
        })
    }

    /// 写提交记录，事务持久化。
    ///
    /// 单次定长写 40 字节到段尾固定偏移——这是原子性的来源。
    pub fn commit(&mut self, timestamp: u64) -> Result<CommitRecord, Error> {
        if self.committed {
            return Err(Error::InvalidArgument);
        }
        // PageRun 表：写在段头之后的预留区（不与追加区页数据重合）。
        // 若写在追加区尾部，恢复扫描会把槽表条目误读为页头
        //（PageRun 与 PageHeader 的 [0,20) 布局重叠，run_crc 恰好
        // 等于同字节上的 PageHeader.page_crc），导致折叠值失配。
        let run_table = self.build_page_run_table();
        // C4：reserved = max_pages * PAGE_RUN_ENTRY_SIZE 可能溢出，必须 checked。
        let reserved = self
            .cfg
            .max_pages
            .checked_mul(crate::segment::PAGE_RUN_ENTRY_SIZE)
            .ok_or(Error::InvalidArgument)?;
        if run_table.len() > reserved {
            return Err(Error::QuotaExceeded);
        }
        if !run_table.is_empty() {
            self.dev.write_at(
                self.cfg.device_offset + SEGMENT_HEADER_SIZE as u64,
                &run_table,
            )?;
        }

        let fold = fold_crcs(&self.page_crcs);
        let record = CommitRecord::new(self.cfg.segment_seq, self.cfg.prev_crc, fold, timestamp);
        self.dev.write_at(
            self.cfg.device_offset + (self.cfg.segment_size - COMMIT_RECORD_SIZE) as u64,
            &record.encode(),
        )?;
        self.dev.sync()?;
        self.committed = true;
        Ok(record)
    }

    /// 构建 PageRun 表。导出段内全部已分配页（M12：page_id 为全局计数器，
    /// 不能按 0..max_pages 遍历，改为遍历自由表中已占用槽位）。
    fn build_page_run_table(&self) -> std::vec::Vec<u8> {
        let mut out = std::vec::Vec::new();
        for run in self.slots.allocated_page_runs() {
            out.extend_from_slice(&run.encode());
        }
        out
    }
}

#[cfg(all(test, feature = "std"))]
mod tests {
    use super::*;
    use crate::device::InMemoryDevice;

    const SEGMENT_SIZE: usize = 1 << 20;
    const PAGE_SIZE: u32 = 4096;
    const SLOTS_PER_PAGE: u16 = 4;
    const MAX_PAGES: usize = 8;

    fn cfg(seq: u64) -> SegmentConfig {
        SegmentConfig {
            segment_seq: seq,
            prev_segment: seq.saturating_sub(1),
            prev_crc: 0,
            device_offset: 0,
            segment_size: SEGMENT_SIZE,
            page_size: PAGE_SIZE,
            slots_per_page: SLOTS_PER_PAGE,
            max_pages: MAX_PAGES,
            encryption_key: None,
        }
    }

    #[test]
    fn 写入后可读回页数据() {
        let mut dev = InMemoryDevice::new(SEGMENT_SIZE);
        let mut w = SegmentWriter::new(&mut dev, cfg(1)).unwrap();
        let a = w.append_page(0, b"hello").unwrap();
        assert_eq!(a.page_id, 0);
        assert_eq!(a.slot_index, 0);
        // 读回：页头 + 载荷
        let mut buf = std::vec![0u8; PAGE_SIZE as usize];
        dev.read_at(a.offset, &mut buf).unwrap();
        let ph = PageHeader::decode(&buf).unwrap();
        assert_eq!(ph.page_id, 0);
        assert_eq!(ph.payload_len, 5);
        assert_eq!(&buf[PAGE_HEADER_SIZE..PAGE_HEADER_SIZE + 5], b"hello");
    }

    #[test]
    fn 提交后记录可解码() {
        let mut dev = InMemoryDevice::new(SEGMENT_SIZE);
        let mut w = SegmentWriter::new(&mut dev, cfg(1)).unwrap();
        w.append_page(0, b"a").unwrap();
        let rec = w.commit(1_700_000_000).unwrap();
        let mut buf = [0u8; COMMIT_RECORD_SIZE];
        dev.read_at((SEGMENT_SIZE - COMMIT_RECORD_SIZE) as u64, &mut buf)
            .unwrap();
        assert_eq!(CommitRecord::decode(&buf).unwrap(), rec);
    }

    #[test]
    fn 提交后不可再写页() {
        let mut dev = InMemoryDevice::new(SEGMENT_SIZE);
        let mut w = SegmentWriter::new(&mut dev, cfg(1)).unwrap();
        w.append_page(0, b"a").unwrap();
        w.commit(1).unwrap();
        assert_eq!(w.append_page(1, b"b"), Err(Error::InvalidArgument));
    }

    #[test]
    fn 重复提交被拒() {
        let mut dev = InMemoryDevice::new(SEGMENT_SIZE);
        let mut w = SegmentWriter::new(&mut dev, cfg(1)).unwrap();
        w.commit(1).unwrap();
        assert_eq!(w.commit(2), Err(Error::InvalidArgument));
    }

    #[test]
    fn 超配额返回配额超限() {
        let mut dev = InMemoryDevice::new(SEGMENT_SIZE);
        // 段很小，装不下几个页
        let small = SegmentConfig {
            segment_size: 8192,
            ..cfg(1)
        };
        let mut w = SegmentWriter::new(&mut dev, small).unwrap();
        let mut err = None;
        for i in 0..10 {
            if let Err(e) = w.append_page(i % MAX_PAGES as u64, &std::vec![7u8; 1024]) {
                err = Some(e);
                break;
            }
        }
        assert_eq!(err, Some(Error::QuotaExceeded));
    }

    #[test]
    fn 页槽用尽后拒绝追加() {
        let mut dev = InMemoryDevice::new(SEGMENT_SIZE);
        let mut w = SegmentWriter::new(&mut dev, cfg(1)).unwrap();
        for _ in 0..SLOTS_PER_PAGE {
            w.append_page(0, b"v").unwrap();
        }
        assert_eq!(
            w.append_page(0, b"v"),
            Err(Error::SlotFrozen { page_id: 0 })
        );
    }

    #[test]
    fn 载荷超页容量被拒() {
        let mut dev = InMemoryDevice::new(SEGMENT_SIZE);
        let mut w = SegmentWriter::new(&mut dev, cfg(1)).unwrap();
        let big = std::vec![0u8; PAGE_SIZE as usize];
        assert_eq!(w.append_page(0, &big), Err(Error::InvalidArgument));
    }

    #[test]
    fn 非法配置被拒() {
        let mut dev = InMemoryDevice::new(SEGMENT_SIZE);
        assert!(SegmentWriter::new(
            &mut dev,
            SegmentConfig {
                page_size: 8,
                ..cfg(1)
            }
        )
        .is_err());
        assert!(SegmentWriter::new(
            &mut dev,
            SegmentConfig {
                slots_per_page: 0,
                ..cfg(1)
            }
        )
        .is_err());
        assert!(SegmentWriter::new(
            &mut dev,
            SegmentConfig {
                segment_size: 16,
                ..cfg(1)
            }
        )
        .is_err());
    }

    #[test]
    fn 页序号随追加递增() {
        let mut dev = InMemoryDevice::new(SEGMENT_SIZE);
        let mut w = SegmentWriter::new(&mut dev, cfg(1)).unwrap();
        let a = w.append_page(0, b"a").unwrap();
        let b = w.append_page(1, b"b").unwrap();
        assert!(b.offset > a.offset, "页按顺序追加");
        let mut buf = [0u8; PAGE_HEADER_SIZE];
        dev.read_at(b.offset, &mut buf).unwrap();
        assert_eq!(PageHeader::decode(&buf).unwrap().version, 2);
    }

    #[test]
    fn 多次提交只认最后一次() {
        // 语义锁定：一个 writer 只提交一次；多事务由上层换 writer 实现
        let mut dev = InMemoryDevice::new(SEGMENT_SIZE);
        let mut w = SegmentWriter::new(&mut dev, cfg(1)).unwrap();
        w.append_page(0, b"a").unwrap();
        assert!(w.commit(1).is_ok());
        assert!(w.commit(2).is_err());
    }

    #[test]
    fn 溢出的max_pages加法返回非法参数而非panic() {
        let mut dev = InMemoryDevice::new(SEGMENT_SIZE);
        let mut bad = cfg(1);
        // page_size=24 = PAGE_HEADER_SIZE 最小合法值；slots_per_page=1 → run_bytes=24
        // max_pages=768614336404564649
        //   max_pages * 24 = 18446744073709551576 ≤ usize::MAX（checked_mul 通过）
        //   SEGMENT_HEADER_SIZE(56) + 乘积 = 18446744073709551632 > usize::MAX（加法溢出 17）
        // checked_mul 只护乘积；加常量必须 checked_add，否则 debug/release overflow-checks 下 panic（C4）。
        bad.page_size = 24;
        bad.slots_per_page = 1;
        bad.max_pages = 768614336404564649;
        // segment_size 保持 SEGMENT_SIZE ≥ SEGMENT_HEADER_SIZE + COMMIT_RECORD_SIZE，能过入口校验
        let r = SegmentWriter::new(&mut dev, bad);
        assert_eq!(r.err(), Some(Error::InvalidArgument));
    }

    #[test]
    fn 页级加密_写入解密往返() {
        let mut dev = InMemoryDevice::new(1 << 20);
        let key = crate::crypto::EncryptionKey {
            sm4: crate::sm4::Sm4Key { bytes: [0x01; 16] },
            hmac: [0x02; 32],
        };
        let scfg = SegmentConfig {
            segment_seq: 1,
            prev_segment: 0,
            prev_crc: 0,
            device_offset: 0,
            segment_size: 1 << 20,
            page_size: 4096,
            slots_per_page: 4,
            max_pages: 8,
            encryption_key: Some(key),
        };
        let mut w = SegmentWriter::new(&mut dev, scfg).unwrap();
        let a = w.append_page(0, b"secret-data").unwrap();
        w.commit(1).unwrap();

        let mut buf = [0u8; 4096];
        dev.read_at(a.offset, &mut buf).unwrap();
        let ph = PageHeader::decode(&buf).unwrap();
        // M8：CBC PKCS#7（11→16）+ HMAC-SM3（32）= 48 字节
        assert_eq!(ph.payload_len, 48);
        let ciphertext = &buf[PAGE_HEADER_SIZE..PAGE_HEADER_SIZE + 48];
        assert_ne!(ciphertext, b"secret-data", "payload 应为密文");
        let plaintext = crate::crypto::decrypt_page(&key, 0, ciphertext).unwrap();
        assert_eq!(plaintext, b"secret-data");
    }
}
