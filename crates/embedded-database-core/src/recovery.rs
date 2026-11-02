//! 崩溃恢复：扫描段，判定最后一个完整事务的边界。

use crate::crc::{fold_crcs, Crc32c};
use crate::device::BlockDevice;
use crate::segment::{
    CommitRecord, PageHeader, COMMIT_RECORD_SIZE, PAGE_HEADER_SIZE, SEGMENT_HEADER_SIZE,
};
use crate::writer::SegmentConfig;
use crate::Error;

/// 恢复扫描的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryReport {
    /// 最后一个有效提交的段序列号。无有效提交则为 `None`。
    pub last_valid_segment: std::option::Option<u64>,
    /// 第一个有效提交的段序列号。无有效提交则为 `None`。
    pub first_valid_segment: std::option::Option<u64>,
    /// 该段最后一个有效页的物理偏移。
    pub last_valid_offset: u64,
    /// 有效页的 CRC，按写入顺序。用于校验 CRC 链。
    pub page_crcs: std::vec::Vec<u32>,
    /// 加密段解密后的页载荷，按写入顺序。未配置密钥时为空。
    pub page_payloads: std::vec::Vec<std::vec::Vec<u8>>,
    /// 因崩溃被截断的字节数。
    pub truncated_bytes: u64,
    /// 检出损坏时的段序列号。校验通过则为 `None`。
    pub corrupted_segment: std::option::Option<u64>,
    /// 最后一个有效段的 commit CRC。无有效提交则为 0。
    /// 供 `Db::open` 初始化 `last_commit_crc`，后续 flush/compact 作为 `prev_crc`。
    pub last_commit_crc: u32,
}

/// 扫描给定段列表，判定最后一个完整事务。
///
/// 判定链：段头 CRC → 提交记录魔数 + CRC → 页 CRC 折叠值匹配 →
/// 跨段 `prev_crc` 连续性（**盘上** CommitRecord 的 `prev_crc` 必须等于上一段的实际 commit CRC）。
/// 空洞/坏槽（无有效提交的段）被跳过而非终止扫描；跨段链校验只在**段号相邻**时生效，
/// 段号跳变（环回覆盖或中间空洞）视为链起点。因此环回后出现在中间的擦除槽不影响其后的段恢复。
///
/// 链校验读盘上提交记录而非调用方传入的 `SegmentConfig.prev_crc`：
/// 恢复时调用方（如 `Db::open`）尚不知道各段 commit CRC，无法预先填对配置；
/// 盘上 `CommitRecord.prev_crc` 被 `commit_crc` 覆盖，篡改会被解码 CRC 拦下。
pub fn recover(dev: &dyn BlockDevice, segments: &[SegmentConfig]) -> Result<RecoveryReport, Error> {
    let mut report = RecoveryReport {
        last_valid_segment: std::option::Option::None,
        first_valid_segment: std::option::Option::None,
        last_valid_offset: 0,
        page_crcs: std::vec::Vec::new(),
        page_payloads: std::vec::Vec::new(),
        truncated_bytes: 0,
        corrupted_segment: std::option::Option::None,
        last_commit_crc: 0,
    };
    let mut prev_actual_crc: std::option::Option<u32> = None;
    let mut prev_seq: std::option::Option<u64> = None;
    let mut first_valid: std::option::Option<u64> = None;

    for cfg in segments {
        // 链校验只在"物理相邻"时生效：段号跳变（环回/空洞）视为链起点。
        // C4：s + 1 可能溢出 u64，用 saturating_add。
        let adjacent = prev_seq.map_or(true, |s| cfg.segment_seq == s.saturating_add(1));
        match scan_segment(dev, cfg, &mut report) {
            Ok(std::option::Option::Some((actual_crc, disk_prev_crc))) => {
                if adjacent {
                    if let Some(expected) = prev_actual_crc {
                        if disk_prev_crc != expected {
                            report.corrupted_segment = std::option::Option::Some(cfg.segment_seq);
                            return Err(Error::Corruption {
                                segment_seq: cfg.segment_seq,
                            });
                        }
                    }
                }
                prev_actual_crc = std::option::Option::Some(actual_crc);
                prev_seq = std::option::Option::Some(cfg.segment_seq);
                if first_valid.is_none() {
                    first_valid = std::option::Option::Some(cfg.segment_seq);
                }
                report.last_valid_segment = std::option::Option::Some(cfg.segment_seq);
                report.last_commit_crc = actual_crc;
            }
            // 空槽/擦除槽/截断段：跳过而不是停止——环回后它们出现在中间。
            // 重置期望 CRC（链在空洞处断开），但保留段号使后续段的相邻性可判定。
            Ok(std::option::Option::None) => {
                prev_actual_crc = std::option::Option::None;
                prev_seq = std::option::Option::Some(cfg.segment_seq);
            }
            Err(e) => {
                report.corrupted_segment = std::option::Option::Some(cfg.segment_seq);
                return Err(e);
            }
        }
    }
    report.first_valid_segment = first_valid;
    Ok(report)
}

/// 扫描单个段。
///
/// 返回 `Some((commit_crc, 盘上 prev_crc))` 表示该段有有效提交；
/// 返回 `None` 表示该段无有效提交（空洞），`recover` 应跳过并继续扫描后续段。
fn scan_segment(
    dev: &dyn BlockDevice,
    cfg: &SegmentConfig,
    report: &mut RecoveryReport,
) -> Result<std::option::Option<(u32, u32)>, Error> {
    // 0. 非法配置：与 writer.rs `new()` 的校验口径一致（C4：库代码禁止 panic）。
    if cfg.segment_size < SEGMENT_HEADER_SIZE + COMMIT_RECORD_SIZE {
        return Err(Error::InvalidArgument);
    }
    if cfg.page_size < PAGE_HEADER_SIZE as u32 || cfg.slots_per_page == 0 || cfg.max_pages == 0 {
        return Err(Error::InvalidArgument);
    }
    let run_bytes = (cfg.page_size as usize)
        .checked_mul(cfg.slots_per_page as usize)
        .ok_or(Error::InvalidArgument)?;
    if cfg.max_pages.checked_mul(run_bytes).is_none() {
        return Err(Error::InvalidArgument);
    }

    // 1. 段头必须完整且 CRC 正确
    let mut header_buf = [0u8; crate::segment::SEGMENT_HEADER_SIZE];
    dev.read_at(cfg.device_offset, &mut header_buf)?;
    let header = match crate::segment::SegmentHeader::decode(&header_buf) {
        Ok(h) => h,
        Err(_) => return Ok(std::option::Option::None),
    };
    if header.segment_seq != cfg.segment_seq {
        // 段头自洽但序号与预期不符：该槽位不是我们要找的段
        //（多段未写入/重叠配置），视为无有效提交并跳过。
        return Ok(std::option::Option::None);
    }

    // 2. 提交记录：魔数或 CRC 不符即视为未提交（写入被截断）
    let mut rec_buf = [0u8; COMMIT_RECORD_SIZE];
    let rec_at = (cfg.segment_size - COMMIT_RECORD_SIZE) as u64;
    let rec_off = cfg
        .device_offset
        .checked_add(rec_at)
        .ok_or(Error::InvalidArgument)?;
    dev.read_at(rec_off, &mut rec_buf)?;
    let record = match CommitRecord::decode(&rec_buf) {
        Ok(r) if r.segment_seq == cfg.segment_seq => r,
        _ => {
            report.truncated_bytes = report
                .truncated_bytes
                .checked_add(cfg.segment_size as u64)
                .ok_or(Error::InvalidArgument)?;
            return Ok(std::option::Option::None);
        }
    };

    // 3. 逐页校验：页头有效且整槽 CRC 与提交记录中的 fold 匹配。
    //    C4：run_bytes / max_pages 乘积与加常量都必须 checked（极端 max_pages
    //    可使 append_start 接近 u64::MAX，后续 at + page_size 同样会溢出）。
    let append_start = cfg
        .max_pages
        .checked_mul(run_bytes)
        .and_then(|bytes| (bytes as u64).checked_add(SEGMENT_HEADER_SIZE as u64))
        .ok_or(Error::InvalidArgument)?;
    let mut at = append_start;
    let mut crcs: std::vec::Vec<u32> = std::vec::Vec::new();
    // M8：密文与 page_id 一并收集——CBC IV 由 page_id 派生，解密必须用对页号。
    let mut ciphertexts: std::vec::Vec<(u64, std::vec::Vec<u8>)> = std::vec::Vec::new();
    let mut last_offset = 0u64;
    let page_size = u64::from(cfg.page_size);

    loop {
        let next_at = at.checked_add(page_size).ok_or(Error::InvalidArgument)?;
        if next_at > rec_at {
            break;
        }
        let slot_off = cfg
            .device_offset
            .checked_add(at)
            .ok_or(Error::InvalidArgument)?;
        let mut slot = std::vec![0u8; cfg.page_size as usize];
        dev.read_at(slot_off, &mut slot)?;

        // 全零槽 = 追加区结束
        if slot.iter().all(|b| *b == 0) {
            break;
        }
        // 页头校验只作为结构哨兵；数据有效性由整槽 CRC 折叠值判定。
        let ph = match PageHeader::decode(&slot) {
            Ok(h) => h,
            Err(_) => break,
        };
        let mut c = Crc32c::new();
        c.update(&slot);
        crcs.push(c.finish());
        if cfg.encryption_key.is_some() {
            let plen = ph.payload_len as usize;
            if PAGE_HEADER_SIZE + plen <= slot.len() {
                ciphertexts.push((
                    ph.page_id,
                    slot[PAGE_HEADER_SIZE..PAGE_HEADER_SIZE + plen].to_vec(),
                ));
            }
        }
        last_offset = at;
        at = next_at;
    }

    if fold_crcs(&crcs) != record.page_crc_fold {
        return Err(Error::Corruption {
            segment_seq: cfg.segment_seq,
        });
    }

    // 4. 段内一致性通过。返回实际 commit_crc 与盘上 prev_crc，
    //    由 recover 用作下一段链校验的期望值。
    report.page_crcs = crcs;
    report.last_valid_offset = last_offset;
    if let Some(key) = &cfg.encryption_key {
        let mut decrypted = std::vec::Vec::with_capacity(ciphertexts.len());
        for (page_id, ct) in ciphertexts {
            // M8：验证 HMAC + CBC 解密（IV = page_id 派生）。
            decrypted.push(crate::crypto::decrypt_page(key, page_id, &ct).map_err(|_| {
                Error::Corruption {
                    segment_seq: cfg.segment_seq,
                }
            })?);
        }
        report.page_payloads = decrypted;
    }
    Ok(std::option::Option::Some((
        record.commit_crc,
        record.prev_crc,
    )))
}

#[cfg(all(test, feature = "std"))]
mod tests {
    use super::*;
    use crate::device::InMemoryDevice;
    use crate::writer::SegmentWriter;

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

    /// 正常写一个段并提交，返回设备与配置。
    fn write_one_segment(seq: u64, pages: &[&[u8]]) -> InMemoryDevice {
        let mut dev = InMemoryDevice::new(SEGMENT_SIZE);
        let mut w = SegmentWriter::new(&mut dev, cfg(seq)).unwrap();
        for (i, p) in pages.iter().enumerate() {
            w.append_page(i as u64, p).unwrap();
        }
        w.commit(1_700_000_000).unwrap();
        dev
    }

    #[test]
    fn 正常提交被完整识别() {
        let dev = write_one_segment(1, &[b"a", b"b", b"c"]);
        let r = recover(&dev, &[cfg(1)]).unwrap();
        assert_eq!(r.last_valid_segment, Some(1));
        assert_eq!(r.first_valid_segment, Some(1));
        assert_eq!(r.page_crcs.len(), 3);
        assert_eq!(r.corrupted_segment, None);
    }

    #[test]
    fn 未提交段被忽略() {
        // 写页但崩溃——没有 commit
        let mut dev = InMemoryDevice::new(SEGMENT_SIZE);
        {
            let mut w = SegmentWriter::new(&mut dev, cfg(1)).unwrap();
            w.append_page(0, b"a").unwrap();
            // 故意不 commit
        }
        let r = recover(&dev, &[cfg(1)]).unwrap();
        assert_eq!(r.last_valid_segment, None);
        assert_eq!(r.first_valid_segment, None);
        assert!(r.page_crcs.is_empty());
    }

    #[test]
    fn 提交记录被截断判定为未提交() {
        let mut dev = write_one_segment(1, &[b"a", b"b"]);
        // 破坏提交记录：翻转尾部字节
        let at = SEGMENT_SIZE - COMMIT_RECORD_SIZE;
        dev.raw_mut()[at + 30] ^= 0xFF; // single segment, device_offset=0
        let r = recover(&dev, &[cfg(1)]).unwrap();
        assert_eq!(r.last_valid_segment, None);
    }

    #[test]
    fn 页数据损坏被检出() {
        let mut dev = write_one_segment(1, &[b"a", b"b"]);
        // 破坏第二个页槽的头部
        let run_bytes = PAGE_SIZE as usize * SLOTS_PER_PAGE as usize;
        let append_start = crate::segment::SEGMENT_HEADER_SIZE + MAX_PAGES * run_bytes;
        dev.raw_mut()[append_start + PAGE_SIZE as usize + 2] ^= 0xFF;
        let r = recover(&dev, &[cfg(1)]);
        assert!(matches!(r, Err(Error::Corruption { .. })), "应报损坏");
    }

    #[test]
    fn 静默位翻转在追加区被检出() {
        let mut dev = write_one_segment(1, &[b"payload-a"]);
        let run_bytes = PAGE_SIZE as usize * SLOTS_PER_PAGE as usize;
        let append_start = crate::segment::SEGMENT_HEADER_SIZE + MAX_PAGES * run_bytes;
        // 翻转载荷区一个字节
        dev.raw_mut()[append_start + 64] ^= 0x01;
        assert!(recover(&dev, &[cfg(1)]).is_err());
    }

    #[test]
    fn 槽区零填充损坏也被检出() {
        // 槽 CRC 覆盖整个槽，因此 padding 损坏必须被发现
        let mut dev = write_one_segment(1, &[b"a"]);
        let run_bytes = PAGE_SIZE as usize * SLOTS_PER_PAGE as usize;
        let append_start = crate::segment::SEGMENT_HEADER_SIZE + MAX_PAGES * run_bytes;
        // 翻转第一个页槽的尾部 padding
        dev.raw_mut()[append_start + PAGE_SIZE as usize - 1] ^= 0xFF;
        assert!(recover(&dev, &[cfg(1)]).is_err());
    }

    #[test]
    fn 段头损坏判定为无提交() {
        let mut dev = write_one_segment(1, &[b"a"]);
        dev.raw_mut()[10] ^= 0xFF; // 破坏段头 magic 之外的字段
        let r = recover(&dev, &[cfg(1)]).unwrap();
        assert_eq!(r.last_valid_segment, None);
        assert_eq!(r.first_valid_segment, None);
    }

    #[test]
    fn 空洞不终止扫描_后续段仍可见() {
        // 环语义：中间空洞（无有效提交）只跳过当前段并重置链，不终止扫描，
        // 其后的段仍被完整恢复。段 3 的盘上 prev_crc 与段 1 的 commit CRC 不同，
        // 因空洞导致链重置（段 3 与空洞段 2 相邻、但期望 CRC 已清空）故不判损坏。
        let mut dev = InMemoryDevice::new(SEGMENT_SIZE * 3);
        let off2 = SEGMENT_SIZE as u64;
        let off3 = (SEGMENT_SIZE * 2) as u64;
        let mk = |seq: u64, device_offset: u64| SegmentConfig {
            segment_seq: seq,
            prev_segment: seq.saturating_sub(1),
            prev_crc: 0,
            device_offset,
            segment_size: SEGMENT_SIZE,
            page_size: PAGE_SIZE,
            slots_per_page: SLOTS_PER_PAGE,
            max_pages: MAX_PAGES,
            encryption_key: None,
        };
        {
            let mut w = SegmentWriter::new(&mut dev, mk(1, 0)).unwrap();
            w.append_page(0, b"a").unwrap();
            w.commit(1).unwrap();
        }
        // 段 2 不写入：空洞。
        {
            let mut w = SegmentWriter::new(&mut dev, mk(3, off3)).unwrap();
            w.append_page(0, b"b").unwrap();
            w.commit(3).unwrap();
        }
        let r = recover(&dev, &[mk(1, 0), mk(2, off2), mk(3, off3)]).unwrap();
        assert_eq!(r.first_valid_segment, Some(1));
        assert_eq!(r.last_valid_segment, Some(3));
        assert_eq!(r.page_crcs.len(), 1, "应恢复空洞之后段 3 的页");
    }

    #[test]
    fn 恢复的页crc与写入器一致() {
        let mut dev = InMemoryDevice::new(SEGMENT_SIZE);
        let mut w = SegmentWriter::new(&mut dev, cfg(1)).unwrap();
        w.append_page(0, b"a").unwrap();
        w.append_page(1, b"b").unwrap();
        let written: std::vec::Vec<u32> = w.page_crcs().to_vec();
        w.commit(1).unwrap();
        let r = recover(&dev, &[cfg(1)]).unwrap();
        assert_eq!(r.page_crcs, written);
    }

    #[test]
    fn 空设备无有效段() {
        let dev = InMemoryDevice::new(SEGMENT_SIZE);
        let r = recover(&dev, &[cfg(1)]).unwrap();
        assert_eq!(r.last_valid_segment, None);
        assert_eq!(r.first_valid_segment, None);
        assert_eq!(r.last_valid_offset, 0);
    }

    #[test]
    fn 过小的段大小返回非法参数而非panic() {
        let dev = InMemoryDevice::new(SEGMENT_SIZE);
        let mut bad = cfg(1);
        // 48 >= COMMIT_RECORD_SIZE(40) 但 < SEGMENT_HEADER_SIZE(56) + COMMIT_RECORD_SIZE(40) = 96
        // 旧校验（< 40）会放过，新校验（与 writer.rs 对齐）必须拒绝。
        bad.segment_size = 48;
        let r = recover(&dev, &[bad]);
        assert_eq!(r, Err(Error::InvalidArgument));
    }

    #[test]
    fn 溢出的max_pages乘积返回非法参数而非panic() {
        let dev = InMemoryDevice::new(SEGMENT_SIZE);
        let mut bad = cfg(1);
        // page_size=2^30 >= PAGE_HEADER_SIZE(24)；slots_per_page=2^10 != 0
        // run_bytes = 2^40；max_pages=2^25；max_pages * run_bytes = 2^65 超出 u64
        bad.page_size = 1 << 30;
        bad.slots_per_page = 1 << 10;
        bad.max_pages = 1 << 25;
        // segment_size 保持 SEGMENT_SIZE = 2^20 >= 96，能通过段头校验
        let r = recover(&dev, &[bad]);
        assert_eq!(r, Err(Error::InvalidArgument));
    }

    #[test]
    fn 溢出的max_pages加法返回非法参数而非panic() {
        // 先用正常配置写一个已提交段（同 segment_size、同 segment_seq=1）
        let dev = write_one_segment(1, &[b"a"]);
        let mut bad = cfg(1);
        // page_size=24 = PAGE_HEADER_SIZE 最小合法值；slots_per_page=1 → run_bytes=24
        // max_pages=768614336404564649
        //   max_pages * 24 = 18446744073709551576 ≤ usize::MAX（checked_mul 通过）
        //   SEGMENT_HEADER_SIZE(56) + 乘积 = 18446744073709551632 > usize::MAX（加法溢出 17）
        // 段头/提交记录解码通过（只校验 segment_seq），在 append_start 处裸加法 panic（C4）。
        bad.page_size = 24;
        bad.slots_per_page = 1;
        bad.max_pages = 768614336404564649;
        // segment_size 保持 SEGMENT_SIZE，与盘上段一致
        let r = recover(&dev, &[bad]);
        assert_eq!(r, Err(Error::InvalidArgument));
    }

    #[test]
    fn 极端max_pages在追加区加法溢出返回非法参数而非panic() {
        // 先用正常配置写一个已提交段（同 segment_size、同 segment_seq=1），
        // 使段头/提交记录解码通过，扫描进入逐页循环。
        let dev = write_one_segment(1, &[b"a"]);
        let mut bad = cfg(1);
        // page_size=24 = PAGE_HEADER_SIZE 最小合法值；slots_per_page=1 → run_bytes=24
        // max_pages=768614336404564648
        //   max_pages * 24 = 18446744073709551552 ≤ usize::MAX（入口 checked_mul 通过）
        //   SEGMENT_HEADER_SIZE(56) + 乘积 = 18446744073709551608 ≤ u64::MAX
        //     （append_start checked_add 通过）
        //   at + page_size = 18446744073709551632 > u64::MAX
        //     （scan_segment 逐页循环裸加法 panic，C4）
        bad.page_size = 24;
        bad.slots_per_page = 1;
        bad.max_pages = 768614336404564648;
        let r = recover(&dev, &[bad]);
        assert_eq!(r, Err(Error::InvalidArgument));
    }

    // 测试名按 M2-3 简报逐字采用（含缩写 CRC），故豁免 non_snake_case。
    #[allow(non_snake_case)]
    #[test]
    fn 跨段CRC链断裂被检出() {
        let seg_size = 1 << 20;
        let mut dev = InMemoryDevice::new(seg_size * 2);
        let cfg1 = SegmentConfig {
            segment_seq: 1,
            prev_segment: 0,
            prev_crc: 0,
            device_offset: 0,
            segment_size: seg_size,
            page_size: 4096,
            slots_per_page: 4,
            max_pages: 8,
            encryption_key: None,
        };
        let cfg2 = SegmentConfig {
            segment_seq: 2,
            prev_segment: 1,
            prev_crc: 0xDEAD_BEEF,
            device_offset: seg_size as u64,
            segment_size: seg_size,
            page_size: 4096,
            slots_per_page: 4,
            max_pages: 8,
            encryption_key: None,
        };

        // 写段 1
        {
            let mut w = SegmentWriter::new(&mut dev, cfg1).unwrap();
            w.append_page(0, b"a").unwrap();
            w.commit(1).unwrap();
        }
        // 写段 2（prev_crc 故意错误）
        {
            let mut w = SegmentWriter::new(&mut dev, cfg2).unwrap();
            w.append_page(0, b"b").unwrap();
            w.commit(2).unwrap();
        }

        // 恢复应检出 CRC 链断裂
        let r = recover(&dev, &[cfg1, cfg2]);
        assert!(r.is_err(), "CRC 链断裂应被检出");
    }
}
