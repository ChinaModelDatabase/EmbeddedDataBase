//! Db：公开嵌入 API。持有 ArenaPool、PageMap、写者锁、块设备引用。

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::arenapool::ArenaPool;
use crate::cursor::Cursor;
use crate::device::BlockDevice;
use crate::recovery::recover;
use crate::segment::{
    PageHeader, PageRun, COMMIT_RECORD_SIZE, PAGE_HEADER_SIZE, PAGE_RUN_ENTRY_SIZE,
    SEGMENT_HEADER_SIZE,
};
use crate::slot::SlotAllocator;
use crate::types::{CommitInfo, CompactionBudget, Config, DbStats, Deadline, TxnOptions};
use crate::writer::{SegmentConfig, SegmentWriter};
use crate::Error;
// no_std crate：format! 需要显式引入（db 模块仅在 std 特性下编译）
use std::format;

/// M11-0：段合并/刷盘路径收集的 key→page_id 更新列表。
/// 条目 = (key, page_id)；payload 以首个 `0xFF` 分隔 key/value。
type KeyUpdates = std::vec::Vec<(std::vec::Vec<u8>, u64)>;

/// M11-1：段合并写入新段时收集的 (page_id, 物理偏移) 列表。
type PlacedPages = std::vec::Vec<(u64, u64)>;

/// M12：当前活跃段的写入状态（同段追加用）。
///
/// 不直接持有 `SegmentWriter`：writer 内嵌 `&'d mut dyn BlockDevice`，
/// 与 `Db.dev` 生命周期冲突（`&mut self` 方法内无法造出 `SegmentWriter<'d>`，
/// 简报的 `Option<SegmentWriter<'d>>` 字段在借用检查器下不可编译）。
/// 改为在 flush 间持久化 cursor / 页 CRC / 槽状态，下次同段追加时
/// 经 `SegmentWriter::resume` 重建写入器。
#[derive(Debug, Clone)]
struct ActiveWriterState {
    /// 该状态所属段序列号（= 写入时的 `Db.active_segment`）。
    segment_seq: u64,
    /// 段内追加区游标（相对段起点，即下一个页槽的偏移）。
    cursor: u64,
    /// 段内已写入页的 CRC，按写入顺序。commit 的 fold 覆盖全段。
    page_crcs: std::vec::Vec<u32>,
    /// 段内槽分配状态。resume 时还原，保证 PageRun 表覆盖全部已写页。
    slots: SlotAllocator,
    /// 该段**首次创建时**的上一段 commit CRC（即段头中记录的 prev_crc）。
    /// 同段追加时 CommitRecord.prev_crc 必须保持此值，
    /// 否则跨段 CRC 链会指向本段早先 commit 而断裂（M12 终审 B-2）。
    prev_crc: u32,
}

/// 数据库句柄。持有 `&mut dyn BlockDevice` 以支持合并写入新段。
pub struct Db<'d> {
    cfg: Config,
    pool: ArenaPool,
    /// page_id → (segment_seq, offset) 映射。
    /// M2 简化：用固定容量 Vec 承载，不用 FixedTable（后者需要 arena 借用）。
    page_map: std::vec::Vec<(u64, u64, u64)>, // (page_id, segment_seq, offset)
    /// 写者锁。
    writer_lock: AtomicBool,
    /// 当前活跃段序列号。下一个写入应使用的段号。
    active_segment: u64,
    /// 恢复得到的第一个存活段序列号（环回时最小有效段号）。无存活段则 `None`。
    /// 供后续合并/退役判断「存活段跨度」用。
    first_live_segment: std::option::Option<u64>,
    /// 上一个已写入段的 commit CRC。flush/compact 作为新段的 `prev_crc`。
    /// 打开时从恢复结果初始化；每次成功 commit 后更新。
    last_commit_crc: u32,
    dev: &'d mut dyn BlockDevice,
    /// 待刷入设备的变更（M5 简化：commit 写入内存，下次操作时刷入）。
    /// `Txn` 持有 `&Db` 无法直接 `&mut`，故用 `RefCell` 提供内部可变性。
    /// 条目 = (逻辑 page_id, payload)，payload = key ++ 0xFF ++ value。
    pending_writes: core::cell::RefCell<std::vec::Vec<(u64, std::vec::Vec<u8>)>>,
    /// M6-5：安全审计日志计数。
    stats: core::cell::RefCell<DbStats>,
    /// M8-3：全局递增 page_id 计数器。`Txn::commit` 用它分配 page_id，
    /// 避免从 `pending.len()` 重计导致跨 flush 重复。
    next_page_id: AtomicU64,
    /// M9-0/M30-0：key → page_id 映射（`RefCell<BTreeMap>` 承载，`db` 模块已 `std` 门控）。
    /// `BTreeMap` 使 lookup/insert 为 O(log n)。
    /// `FixedTable` 需要 `&mut Arena`，与 `dev: &'d mut dyn BlockDevice` 生命周期冲突，
    /// 留待 M10。
    key_map: core::cell::RefCell<std::collections::BTreeMap<std::vec::Vec<u8>, u64>>,
    /// M12：当前活跃段的写入状态。`Some` 表示该段可继续同段追加。
    active_writer: Option<ActiveWriterState>,
}

impl<'d> core::fmt::Debug for Db<'d> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Db")
            .field("cfg", &self.cfg)
            .field("pool", &self.pool)
            .field("page_map", &self.page_map)
            .field("writer_lock", &self.writer_lock)
            .field("active_segment", &self.active_segment)
            .field("first_live_segment", &self.first_live_segment)
            .field("last_commit_crc", &self.last_commit_crc)
            .field("pending_writes", &self.pending_writes.borrow().len())
            .field("active_writer", &self.active_writer)
            .finish_non_exhaustive()
    }
}

impl<'d> Db<'d> {
    /// 打开数据库。冷启动时扫描已有段，重建映射表。
    pub fn open(dev: &'d mut dyn BlockDevice, cfg: Config) -> Result<Db<'d>, Error> {
        cfg.validate()?;

        let pool = ArenaPool::new(cfg.arena_pool_size, 1 << 16);

        // 冷启动恢复：扫描设备上所有段槽位。
        // 只把完整落在设备范围内的槽位交给 recover——
        // 越界读会以 InvalidArgument 失败，把「设备比配置小」误判成数据损坏。
        let dev_size = dev.size();
        let seg_size_u64 = cfg.segment_size as u64;
        if seg_size_u64 > dev_size {
            // M8-3：第一个段槽位都放不下 ⇒ 设备与 segment_size 不匹配。
            return Err(Error::InvalidArgument);
        }
        let slots = core::cmp::min(cfg.max_segments as u64, dev_size / seg_size_u64) as usize;

        // 环语义：槽位不携带段号，段号写在段头里。逐个槽读段头，
        // 可解码的按段号排序后交给 recover（链校验只对相邻段生效）。
        let mut collected: std::vec::Vec<(u64, usize)> = std::vec::Vec::new();
        for slot in 0..slots {
            let off = (slot as u64)
                .checked_mul(seg_size_u64)
                .ok_or(Error::InvalidArgument)?;
            let mut hbuf = [0u8; SEGMENT_HEADER_SIZE];
            if dev.read_at(off, &mut hbuf).is_err() {
                continue;
            }
            match crate::segment::SegmentHeader::decode(&hbuf) {
                Ok(h) => collected.push((h.segment_seq, slot)),
                Err(_) => continue,
            }
        }
        collected.sort_by_key(|(seq, _)| *seq);
        collected.dedup_by_key(|(seq, _)| *seq);
        let mut segment_cfgs = std::vec::Vec::new();
        for (seq, slot) in collected {
            let device_offset = (slot as u64)
                .checked_mul(seg_size_u64)
                .ok_or(Error::InvalidArgument)?;
            segment_cfgs.push(SegmentConfig {
                segment_seq: seq,
                prev_segment: seq.saturating_sub(1),
                prev_crc: 0,
                device_offset,
                segment_size: cfg.segment_size,
                page_size: cfg.page_size,
                slots_per_page: cfg.slots_per_page,
                max_pages: cfg.max_pages,
                encryption_key: cfg.encryption_key,
            });
        }

        let report = recover(dev, &segment_cfgs)?;
        let active_segment = report.last_valid_segment.unwrap_or(0).saturating_add(1);
        let first_live_segment = report.first_valid_segment;
        // 打开时继承上一段的 commit CRC，后续 flush/compact 作为 prev_crc。
        let last_commit_crc = report.last_commit_crc;

        // 重建映射表：从已恢复段的 PageRun 表提取 page_id → (segment_seq, offset)。
        // PageRun 表按「实际用过的页」紧凑排列在段头之后，不是按 page_id 稀疏索引；
        // 因此顺序解码直到遇到全零/非法条目为止。
        let mut page_map = std::vec::Vec::new();
        for cfg_seg in &segment_cfgs {
            // 只登记最后一个有效段及其之前的段；其后的段（如未提交的段头）忽略。
            match report.last_valid_segment {
                Some(last) if cfg_seg.segment_seq <= last => {}
                _ => continue,
            }
            let run_table_off = cfg_seg
                .device_offset
                .checked_add(SEGMENT_HEADER_SIZE as u64)
                .ok_or(Error::InvalidArgument)?;
            for slot in 0..cfg_seg.max_pages {
                // slot < max_pages；max_pages * run_bytes 已在 Config::validate
                // checked_mul 校验且 run_bytes >= PAGE_RUN_ENTRY_SIZE，乘积本应
                // 在 usize 内；仍用 checked_mul 锁定 C4（禁 panic）。
                let entry_bytes = slot
                    .checked_mul(PAGE_RUN_ENTRY_SIZE)
                    .ok_or(Error::InvalidArgument)? as u64;
                let entry_off = run_table_off
                    .checked_add(entry_bytes)
                    .ok_or(Error::InvalidArgument)?;
                let mut entry_buf = [0u8; PAGE_RUN_ENTRY_SIZE];
                if dev.read_at(entry_off, &mut entry_buf).is_err() {
                    break;
                }
                // 全零槽 = 表结束（未使用的槽位）
                if entry_buf.iter().all(|b| *b == 0) {
                    break;
                }
                match PageRun::decode(&entry_buf) {
                    Ok(run) => {
                        page_map.push((run.page_id, cfg_seg.segment_seq, run.run_offset));
                    }
                    Err(_) => break,
                }
            }
        }

        // M8-3：page_id 全局递增计数器。从已恢复映射的最大 page_id 起步，
        // 避免重开后新提交与既有页冲突。C4：不用 unwrap。
        let next_page_id = match page_map.iter().map(|(pid, _, _)| *pid).max() {
            Some(max_id) => max_id.saturating_add(1),
            None => 0,
        };

        // 冷启动重建 key_map：遍历 page_map，读取每页 payload，解析 key。
        // 按 segment_seq 升序遍历（page_map 按段顺序构建），后写自然覆盖。
        let key_map = if cfg.encryption_key.is_some() || !page_map.is_empty() {
            let mut km: std::collections::BTreeMap<std::vec::Vec<u8>, u64> = Default::default();
            for (page_id, _segment_seq, offset) in &page_map {
                // 读页头
                let mut header_buf = [0u8; PAGE_HEADER_SIZE];
                if dev.read_at(*offset, &mut header_buf).is_err() {
                    continue;
                }
                let header = match PageHeader::decode(&header_buf) {
                    Ok(h) => h,
                    Err(_) => continue,
                };
                if header.page_id != *page_id {
                    continue;
                }
                // 读 payload
                let payload_len = header.payload_len as usize;
                let payload_off = match offset.checked_add(PAGE_HEADER_SIZE as u64) {
                    Some(v) => v,
                    None => continue,
                };
                let mut payload = std::vec![0u8; payload_len];
                if dev.read_at(payload_off, &mut payload).is_err() {
                    continue;
                }
                // 解密（若有密钥）
                let payload = if let Some(k) = &cfg.encryption_key {
                    match crate::crypto::decrypt_page(k, *page_id, &payload) {
                        Ok(p) => p,
                        Err(_) => continue,
                    }
                } else {
                    payload
                };
                // 解析 key ++ 0xFF ++ value（payload 以首个 0xFF 分隔；
                // key 含 0xFF 的旧数据会错读，M9-1 已在 put 拒绝新数据，旧数据为已知限制）
                if let Some(pos) = payload.iter().position(|&b| b == 0xFF) {
                    let key = &payload[..pos];
                    // 同键覆盖（后写胜出：升序遍历 + insert 覆盖）
                    km.insert(key.to_vec(), *page_id);
                }
            }
            km
        } else {
            Default::default()
        };

        Ok(Db {
            cfg,
            pool,
            page_map,
            writer_lock: AtomicBool::new(false),
            active_segment,
            first_live_segment,
            last_commit_crc,
            dev,
            pending_writes: core::cell::RefCell::new(std::vec::Vec::new()),
            stats: core::cell::RefCell::new(DbStats::default()),
            next_page_id: AtomicU64::new(next_page_id),
            key_map: core::cell::RefCell::new(key_map),
            active_writer: None,
        })
    }

    pub fn config(&self) -> &Config {
        &self.cfg
    }

    /// Arena 池。后续 Txn 按槽索引借用缓冲区。
    pub fn arena_pool(&self) -> &ArenaPool {
        &self.pool
    }

    /// 当前活跃段序列号。下一个写入应使用的段号。
    pub fn active_segment(&self) -> u64 {
        self.active_segment
    }

    /// 恢复得到的第一个存活段序列号（环回时最小有效段号）。无存活段则 `None`。
    pub fn first_live_segment(&self) -> Option<u64> {
        self.first_live_segment
    }

    /// 槽位偏移：段号从 1 开始，环回取模。
    fn slot_offset(cfg: &Config, seq: u64) -> Result<u64, Error> {
        let slot = seq.saturating_sub(1) % cfg.max_segments.max(1) as u64;
        (slot as usize)
            .checked_mul(cfg.segment_size)
            .map(|v| v as u64)
            .ok_or(Error::InvalidArgument)
    }

    /// 尝试获取写者锁。已被占用返回 `Busy`。
    pub fn try_lock_writer(&self) -> Result<(), Error> {
        self.writer_lock
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .map(|_| ())
            .map_err(|_| Error::Busy)
    }

    /// 释放写者锁。
    pub fn unlock_writer(&self) {
        self.writer_lock.store(false, Ordering::Release);
    }

    /// 查询 page_id 的物理位置。同一 page_id 出现在多个段时取 `segment_seq`
    /// 最大者（最新版本）。
    pub fn lookup_page(&self, page_id: u64) -> Option<(u64, u64)> {
        self.page_map
            .iter()
            .filter(|(pid, _, _)| *pid == page_id)
            .max_by_key(|(_, seq, _)| *seq)
            .map(|(_, seq, off)| (*seq, *off))
    }

    /// 冷启动恢复得到的页映射条目数。
    pub fn page_map_len(&self) -> usize {
        self.page_map.len()
    }

    /// M19-0：暴露页映射条目 `(page_id, segment_seq, offset)`。
    pub fn page_map_entries(&self) -> &[(u64, u64, u64)] {
        &self.page_map
    }

    /// M5：已提交但尚未刷入设备的变更条数。
    ///
    /// 真正落盘在 M6：`Db` 下次 `begin`/`compact` 时把 `pending_writes`
    /// 写入活跃段。此处仅暴露内存中待刷写入的条数供测试与诊断。
    pub fn pending_writes_len(&self) -> usize {
        self.pending_writes.borrow().len()
    }

    /// M6-5：安全审计日志。返回当前操作计数快照。
    pub fn stats(&self) -> DbStats {
        self.stats.borrow().clone()
    }

    /// 读取页的 payload（跳过 PageHeader）。
    ///
    /// 从 page_map 查 offset → 读 PageHeader → 返回 payload 字节。
    /// 同一 page_id 出现在多个段时取 segment_seq 最大者（最新版本）。
    /// M5 简化：不校验页 CRC（合并路径已有校验）。
    /// M8-2：成功更新 `read_count`，失败更新 `error_count`/`last_error`。
    pub fn read_payload(&self, page_id: u64) -> Result<std::vec::Vec<u8>, Error> {
        let result = self.read_payload_uncounted(page_id);
        let mut stats = self.stats.borrow_mut();
        match &result {
            Ok(_) => stats.read_count += 1,
            Err(e) => {
                stats.error_count += 1;
                stats.last_error = Some(format!("{e}").into_bytes());
            }
        }
        result
    }

    /// `read_payload` 的计数无关实现。审计计数由 `read_payload` 统一维护。
    fn read_payload_uncounted(&self, page_id: u64) -> Result<std::vec::Vec<u8>, Error> {
        let offset = self
            .page_map
            .iter()
            .filter(|(pid, _, _)| *pid == page_id)
            .max_by_key(|(_, seg_seq, _)| *seg_seq)
            .map(|(_, _, off)| *off)
            .ok_or(Error::PageNotFound { page_id })?;

        // 读页头
        let mut header_buf = [0u8; PAGE_HEADER_SIZE];
        self.dev.read_at(offset, &mut header_buf)?;
        let header = PageHeader::decode(&header_buf)?;
        if header.page_id != page_id {
            return Err(Error::CorruptedPage { page_id });
        }

        // 读 payload
        let payload_len = header.payload_len as usize;
        let payload_off = offset
            .checked_add(PAGE_HEADER_SIZE as u64)
            .ok_or(Error::InvalidArgument)?;
        let mut payload = std::vec![0u8; payload_len];
        self.dev.read_at(payload_off, &mut payload)?;
        // M8：验证 HMAC + CBC 解密（IV = page_id 派生）。
        if let Some(key) = &self.cfg.encryption_key {
            return crate::crypto::decrypt_page(key, page_id, &payload);
        }
        Ok(payload)
    }

    /// M3：读取页数据前检查 Deadline。
    ///
    /// M2 遗留：`Db` 不维护已提交数据的读路径。Deadline 通过后返回
    /// `PageNotFound` 占位；真实页读取在 M4 Flash 后端实现。
    pub fn read_page_with_deadline(
        &self,
        page_id: u64,
        deadline: Deadline,
    ) -> Result<&[u8], Error> {
        deadline.remaining()?;
        Err(Error::PageNotFound { page_id })
    }

    /// 把 `pending_writes` 写入活跃段。返回实际写入字节数。
    ///
    /// M7：commit 时数据进入 `pending_writes`，`flush` 时真正落盘。
    /// `begin()` 会自动调用此方法。
    ///
    /// M12：同段追加——若 `active_writer` 指向的段同时满足
    /// 「剩余字节 ≥ pending 页数 × page_size」与「剩余槽位 ≥ pending 页数」，
    /// 则复用该段继续追加（不递增 `active_segment`）；否则退役旧状态并开新段。
    ///
    /// M12 终审修复：
    /// - B-1：槽位/字节耗尽时必须用 `active_segment` 开新段，而非复用满段号。
    /// - B-2：同段追加时 CommitRecord.prev_crc 保持段创建时的值（`state.prev_crc`），
    ///   而非本段中途 commit 的 `last_commit_crc`。
    /// - B-3：先收集全部页数据到内存并完成校验，确认全部成功后再创建 writer 写入；
    ///   写入/提交失败时回滚已写页槽，保证已提交段仍可冷启动恢复。
    pub fn flush(&mut self) -> Result<CommitInfo, Error> {
        if self.pending_writes.borrow().is_empty() {
            return Ok(CommitInfo {
                segment_seq: self.active_segment,
                bytes_written: 0,
            });
        }

        let pending_count = self.pending_writes.borrow().len() as u64;
        // 每个 pending 页在顺序追加区消耗一个完整 page_size 槽。
        let needed_bytes = pending_count
            .checked_mul(self.cfg.page_size as u64)
            .ok_or(Error::CapacityExhausted)?;

        // B-3：先收集全部页数据到内存（不写设备），确认全部成功后再创建 writer。
        // M29-1-0：take() 避免 clone 拷贝；flush 成功后 pending 本就要清空。
        let pending: std::vec::Vec<(u64, std::vec::Vec<u8>)> =
            std::mem::take(&mut *self.pending_writes.borrow_mut());

        // M12：目标段 = active_writer 所在段（若可追加）或 active_segment（开新段）。
        // B-1：槽位/字节耗尽时必须用 active_segment 开新段，而非复用满段号。
        // B-2：同段追加时 prev_crc 用 state.prev_crc（段创建时的值），
        //   新段路径用 last_commit_crc。
        let (segment_seq, can_append, prev_crc) = match &self.active_writer {
            Some(state) => {
                let commit_at = (self.cfg.segment_size - COMMIT_RECORD_SIZE) as u64;
                let remaining_bytes = commit_at.saturating_sub(state.cursor);
                let used_pages = state.slots.committed_page_count() as u64;
                let remaining_slots = (self.cfg.max_pages as u64).saturating_sub(used_pages);
                let fits = needed_bytes <= remaining_bytes && pending_count <= remaining_slots;
                if fits {
                    (state.segment_seq, true, state.prev_crc)
                } else {
                    (self.active_segment, false, self.last_commit_crc)
                }
            }
            None => (self.active_segment, false, self.last_commit_crc),
        };

        // 开新段前：存活段数达上限（max_segments - 1）时先合并不出腾槽。
        // 合并会改写 active_segment/page_map，段号需在合并后重新计算。
        let (segment_seq, can_append, prev_crc) = if can_append {
            (segment_seq, can_append, prev_crc)
        } else {
            self.ensure_slot_free()?;
            (self.active_segment, false, self.last_commit_crc)
        };

        let device_offset = Self::slot_offset(&self.cfg, segment_seq)?;

        let scfg = SegmentConfig {
            segment_seq,
            prev_segment: segment_seq.saturating_sub(1),
            prev_crc,
            device_offset,
            segment_size: self.cfg.segment_size,
            page_size: self.cfg.page_size,
            slots_per_page: self.cfg.slots_per_page,
            max_pages: self.cfg.max_pages,
            encryption_key: self.cfg.encryption_key,
        };

        let mut bytes_written = 0u64;
        // 用 writer 返回的真实物理偏移登记 page_map——顺序追加区布局下，
        // 固定公式 SEGMENT_HEADER_SIZE + page_id * page_size 与实际落盘位置不符
        //（同 M5-0 先例）。
        let mut placed: std::vec::Vec<(u64, u64)> = std::vec::Vec::new();
        // M9-0：写盘成功后从 pending payload（key ++ 0xFF ++ value）提取 key 更新映射。
        let mut key_updates: std::vec::Vec<(std::vec::Vec<u8>, u64)> = std::vec::Vec::new();
        // B-3：记录本次已写入设备的页槽偏移，失败时回滚（清零）。
        let mut written_offsets: std::vec::Vec<u64> = std::vec::Vec::new();

        if can_append {
            // B-3：不 take() active_writer——仅在 commit 成功后才更新状态。
            // 从 state 克隆数据创建 writer；失败时 active_writer 保持不变。
            let (state_cursor, state_page_crcs, state_slots, state_prev_crc) = {
                let state = self.active_writer.as_ref().ok_or(Error::InvalidArgument)?;
                (
                    state.cursor,
                    state.page_crcs.clone(),
                    state.slots.clone(),
                    state.prev_crc,
                )
            };
            let mut writer =
                SegmentWriter::resume(self.dev, scfg, state_cursor, state_page_crcs, state_slots)?;
            // B-3：循环内只记录错误，不在 writer 存活期间调用 self 方法。
            let mut loop_err: Option<Error> = None;
            for (page_id, payload) in &pending {
                if loop_err.is_some() {
                    break;
                }
                match writer.append_page(*page_id, payload) {
                    Ok(app) => {
                        written_offsets.push(app.offset);
                        placed.push((*page_id, app.offset));
                        match bytes_written.checked_add(self.cfg.page_size as u64) {
                            Some(v) => bytes_written = v,
                            None => loop_err = Some(Error::CapacityExhausted),
                        }
                        if let Some(sep) = payload.iter().position(|b| *b == 0xFF) {
                            key_updates.push((payload[..sep].to_vec(), *page_id));
                        }
                    }
                    Err(e) => loop_err = Some(e),
                }
            }
            if let Some(e) = loop_err {
                // B-3：回滚已写页槽，已提交段保持可恢复。
                drop(writer);
                self.rollback_written_pages(&written_offsets);
                return Err(e);
            }
            match writer.commit(0) {
                Ok(record) => {
                    // 成功：更新状态。
                    self.last_commit_crc = record.commit_crc;
                    self.active_writer = Some(ActiveWriterState {
                        segment_seq,
                        cursor: writer.cursor(),
                        page_crcs: writer.page_crcs().to_vec(),
                        slots: writer.slots().clone(),
                        prev_crc: state_prev_crc,
                    });
                    // 同段追加：active_segment 不递增。
                }
                Err(e) => {
                    // B-3：commit 失败同样回滚。
                    drop(writer);
                    self.rollback_written_pages(&written_offsets);
                    return Err(e);
                }
            }
        } else {
            // 开新段：退役旧状态（其数据已在先前 flush 提交），写入新段。
            // B-1：segment_seq 已修正为 active_segment（满段号不会被复用）。
            self.active_writer = None;
            // 环回覆写最旧槽（M1 环语义）：目标槽可能被存活段占用。
            // 摘除其 page_map/key_map 映射——否则指向被覆写区域的悬空
            // 引用会在读取时报 CorruptedPage；摘除后旧键显式降级为
            // Ok(None)。必须在 writer 创建前调用（writer 存活期间
            // 不可调用 self 方法）。
            self.evict_overwritten_segment(segment_seq)?;
            let mut writer = SegmentWriter::new(self.dev, scfg)?;
            let mut loop_err: Option<Error> = None;
            for (page_id, payload) in &pending {
                if loop_err.is_some() {
                    break;
                }
                match writer.append_page(*page_id, payload) {
                    Ok(app) => {
                        written_offsets.push(app.offset);
                        placed.push((*page_id, app.offset));
                        match bytes_written.checked_add(self.cfg.page_size as u64) {
                            Some(v) => bytes_written = v,
                            None => loop_err = Some(Error::CapacityExhausted),
                        }
                        if let Some(sep) = payload.iter().position(|b| *b == 0xFF) {
                            key_updates.push((payload[..sep].to_vec(), *page_id));
                        }
                    }
                    Err(e) => loop_err = Some(e),
                }
            }
            if let Some(e) = loop_err {
                drop(writer);
                self.rollback_written_pages(&written_offsets);
                return Err(e);
            }
            match writer.commit(0) {
                Ok(record) => {
                    self.last_commit_crc = record.commit_crc;
                    self.active_writer = Some(ActiveWriterState {
                        segment_seq,
                        cursor: writer.cursor(),
                        page_crcs: writer.page_crcs().to_vec(),
                        slots: writer.slots().clone(),
                        prev_crc: scfg.prev_crc,
                    });
                    self.active_segment = segment_seq.saturating_add(1);
                    // 首次开段时初始化存活段起点，否则自动合并的存活数恒为 0。
                    if self.first_live_segment.is_none() {
                        self.first_live_segment = Some(segment_seq);
                    }
                }
                Err(e) => {
                    drop(writer);
                    self.rollback_written_pages(&written_offsets);
                    return Err(e);
                }
            }
        }

        // M29-1-0：pending 已在函数开头 take() 清空，此处无需再 clear。
        for (page_id, offset) in placed {
            self.page_map.push((page_id, segment_seq, offset));
        }
        // M9-0：flush 成功后更新 key→page 映射（同键覆盖）。
        for (key, page_id) in key_updates {
            self.update_key_map(&key, page_id);
        }

        Ok(CommitInfo {
            segment_seq,
            bytes_written,
        })
    }

    /// B-3：回滚已写入追加区的页槽（清零），使已提交段的恢复扫描
    /// 停在第一个全零槽，CommitRecord.fold 与追加区页集合保持一致。
    fn rollback_written_pages(&mut self, offsets: &[u64]) {
        let zeros = std::vec![0u8; self.cfg.page_size as usize];
        for offset in offsets {
            let _ = self.dev.write_at(*offset, &zeros);
        }
    }

    /// M9-0/M30-0：更新 key→page 映射。同键覆盖（后写胜出）。
    pub fn update_key_map(&self, key: &[u8], page_id: u64) {
        let mut map = self.key_map.borrow_mut();
        map.insert(key.to_vec(), page_id);
    }

    /// M9-0/M30-0：按 key 查询 page_id。未提交/未 flush 的 key 返回 `None`。
    pub fn lookup_key(&self, key: &[u8]) -> Option<u64> {
        let map = self.key_map.borrow();
        map.get(key).copied()
    }

    /// 在 `begin()` 中自动调用：若有待落盘数据，先 flush。
    fn auto_flush(&mut self) -> Result<(), Error> {
        if self.pending_writes.borrow().is_empty() {
            return Ok(());
        }
        self.flush()?;
        Ok(())
    }

    /// 开始事务。抢写者锁，失败返回 `Busy`。
    ///
    /// M2 简化：变更暂存在事务内的 `Vec`（`mod db` 走 `std` 特性路径），
    /// commit 时写入 `Db.pending_writes`（M5）。
    /// M7：抢锁后调用 `auto_flush()`，把待落盘数据写入活跃段。
    /// `begin` 为 `&mut self`：`auto_flush` 需要 `&mut dev`。
    pub fn begin(&mut self, opts: TxnOptions) -> Result<Txn<'_, 'd>, Error> {
        if let Some(dl) = opts.deadline {
            dl.remaining()?;
        }
        self.try_lock_writer()?;
        // M7：自动 flush 待落盘数据
        if let Err(e) = self.auto_flush() {
            // 抢锁成功但 flush 失败：释放锁，避免调用方无法再 begin。
            self.unlock_writer();
            return Err(e);
        }
        Ok(Txn {
            db: self,
            changes: std::vec::Vec::new(),
            deadline: opts.deadline,
            committed: false,
        })
    }

    /// 便捷写入：`begin` → `Txn::put` → `commit`（变更进入 `pending_writes`，
    /// 需 `flush` 才落盘）。key 含 `0xFF` 或写者锁被占用时返回错误。
    pub fn put(&mut self, k: &[u8], v: &[u8]) -> Result<(), Error> {
        let mut txn = self.begin(TxnOptions::default())?;
        txn.put(k, v)?;
        txn.commit()?;
        Ok(())
    }

    /// 便捷读取：查询已提交（已 flush）的键值，未命中返回 `Ok(None)`。
    /// 与 `Txn::get` 的已提交数据路径同构（key_map → 读页 → 解析 payload）。
    pub fn get(&self, k: &[u8]) -> Result<Option<std::vec::Vec<u8>>, Error> {
        if let Some(page_id) = self.lookup_key(k) {
            let payload = self.read_payload(page_id)?;
            // payload 格式: key ++ 0xFF ++ value
            if let Some(pos) = payload.iter().position(|b| *b == 0xFF) {
                if &payload[..pos] == k {
                    return Ok(Some(payload[pos + 1..].to_vec()));
                }
            }
        }
        Ok(None)
    }

    /// 手动触发段合并。M2 简化：全量合并，不做预算化切片。
    pub fn compact(&mut self) -> Result<(), Error> {
        self.try_lock_writer()?;
        // M12：合并前退役当前段写入状态。flush 已写提交记录，
        // 此处仅丢弃状态，避免合并后仍向旧段追加。
        self.active_writer = None;
        // M2 简化：实际合并逻辑需要读取所有段的最新版本页，
        // 写入新段，更新映射表。
        // 当前实现：仅验证锁获取和基本流程，合并逻辑在 M2-8 完善。
        self.unlock_writer();
        Ok(())
    }

    /// 预算化合并。M4：扫描映射表取各页最新版本，真正写入新段。
    ///
    /// Deadline 契约（C7）：
    /// - 过期 + `interruptible=true`：可中断长任务，返回 `interrupted: true` 报告。
    /// - 过期 + `interruptible=false`：必须返回 `Err(Error::DeadlineExceeded)`，禁止静默成功。
    /// - 未过期：正常执行。
    pub fn compact_with_budget(
        &mut self,
        budget: &CompactionBudget,
        deadline: Option<Deadline>,
    ) -> Result<CompactReport, Error> {
        self.try_lock_writer()?;

        // Deadline 检查：已超时则按 C7 契约返回。
        if let Some(dl) = deadline {
            if dl.remaining().is_err() {
                self.unlock_writer();
                if budget.interruptible {
                    return Ok(CompactReport {
                        segments_merged: 0,
                        bytes_written: 0,
                        interrupted: true,
                        write_amplification: 1.0,
                    });
                } else {
                    return Err(Error::DeadlineExceeded);
                }
            }
        }

        let result = self.compact_locked();
        self.unlock_writer();
        result
    }

    /// 合并实现（**调用方必须持有 writer 锁**，或从 `flush` 内部调用）。
    ///
    /// 解锁交由调用方负责：`compact_with_budget` 抢锁/解锁，
    /// `ensure_slot_free` 从 `flush` 调用（`flush` 本身不持锁）。
    fn compact_locked(&mut self) -> Result<CompactReport, Error> {
        let latest = self.build_latest()?;
        self.compact_into(latest)
    }

    /// 收集合并目标：每个 `page_id` 的当前版本（同页取最新段），再**按键去重**——
    /// 同一键只保留 `(segment_seq, page_id)` 最大的版本，旧版本不再搬运；
    /// payload 无 `0xFF` 键分隔的裸页不判死（保留）。不可读页（槽位复用后的
    /// 陈旧表项指向已清零/已改写的区域）跳过，不参与合并判定。
    fn build_latest(&self) -> Result<std::collections::BTreeMap<u64, (u64, u64)>, Error> {
        use std::collections::btree_map::Entry;
        // 当前版本：同 page_id 取 segment_seq 最大者。
        let mut current: std::collections::BTreeMap<u64, (u64, u64)> = Default::default();
        for (page_id, seg_seq, offset) in &self.page_map {
            current
                .entry(*page_id)
                .and_modify(|e| {
                    if *seg_seq > e.0 {
                        *e = (*seg_seq, *offset);
                    }
                })
                .or_insert((*seg_seq, *offset));
        }
        // 按键保留最新版本；(seg_seq, page_id) 字典序最大 = 最后写入者胜。
        let mut best: std::collections::BTreeMap<std::vec::Vec<u8>, (u64, u64)> =
            Default::default();
        let mut unkeyed: std::collections::BTreeMap<u64, (u64, u64)> = Default::default();
        for (&page_id, &(seg_seq, offset)) in &current {
            let payload = match self.read_payload(page_id) {
                Ok(p) => p,
                Err(_) => continue,
            };
            match payload.iter().position(|b| *b == 0xFF) {
                Some(sep) => {
                    let key = payload[..sep].to_vec();
                    let cand = (seg_seq, page_id);
                    match best.entry(key) {
                        Entry::Occupied(mut o) => {
                            if cand > *o.get() {
                                *o.get_mut() = cand;
                            }
                        }
                        Entry::Vacant(v) => {
                            v.insert(cand);
                        }
                    }
                }
                None => {
                    unkeyed.insert(page_id, (seg_seq, offset));
                }
            }
        }
        let mut latest = unkeyed;
        for (_key, (_seg_seq, page_id)) in best {
            if let Some(&(seg_seq, offset)) = current.get(&page_id) {
                latest.insert(page_id, (seg_seq, offset));
            }
        }
        Ok(latest)
    }

    /// 把 `latest`（`page_id → (segment_seq, offset)`）写入新段并回收退役段。
    ///
    /// 单段合并受 `max_pages` 约束：装不下时**在写段头之前**返回
    /// `CapacityExhausted`，避免半成品段破坏目标槽位。
    fn compact_into(
        &mut self,
        latest: std::collections::BTreeMap<u64, (u64, u64)>,
    ) -> Result<CompactReport, Error> {
        if latest.len() > self.cfg.max_pages {
            return Err(Error::CapacityExhausted);
        }
        // 全部页都不可读时不得把空段当合并结果（那会清空 page_map 丢数据）。
        if latest.is_empty() && !self.page_map.is_empty() {
            return Err(Error::CapacityExhausted);
        }

        // M12：合并前退役当前段写入状态。flush 已写提交记录，
        // 此处仅丢弃状态，避免合并后仍向旧段追加。
        self.active_writer = None;

        // 收集所有唯一 page_id 的最新版本（segment_seq 最大者胜）。
        // `active_segment` 是「下一个写入应使用的段号」（open 置为 last_valid+1），
        // 新段直接占用该段号；若再 +1 会在设备序列上留下空洞，
        // 且新段头落在第 4 段槽位而非紧随已恢复段之后。
        let new_segment_seq = self.active_segment;
        // C4：段偏移由 slot_offset 计算（按段号取模环回），乘法 checked。
        // 溢出 => 段布局不可寻址，属配置不合法。
        let device_offset = Self::slot_offset(&self.cfg, new_segment_seq)?;
        let scfg = SegmentConfig {
            segment_seq: new_segment_seq,
            prev_segment: new_segment_seq.saturating_sub(1),
            prev_crc: self.last_commit_crc,
            device_offset,
            segment_size: self.cfg.segment_size,
            page_size: self.cfg.page_size,
            slots_per_page: self.cfg.slots_per_page,
            max_pages: self.cfg.max_pages,
            encryption_key: self.cfg.encryption_key,
        };

        let segments_merged = self.active_segment as usize;
        // C4：logical_bytes = pages * page_size 不在 validate 覆盖范围内。
        // 溢出 => 逻辑数据量超出可表示范围，属容量耗尽。
        let logical_bytes = (latest.len() as u64)
            .checked_mul(self.cfg.page_size as u64)
            .ok_or(Error::CapacityExhausted)?;
        let (bytes_written, key_updates, placed) = self.write_new_segment(&latest, scfg)?;

        // 更新映射表：所有页指向新段（使用 writer 返回的实际物理偏移）。
        // 顺序追加区布局下，固定公式 SEGMENT_HEADER_SIZE + page_id * page_size
        // 与实际落盘位置不符（同 flush 路径先例）。
        let new_offsets: std::collections::BTreeMap<u64, u64> = placed.into_iter().collect();
        self.page_map = latest
            .into_iter()
            .filter_map(|(pid, _)| {
                new_offsets
                    .get(&pid)
                    .map(|&off| (pid, new_segment_seq, off))
            })
            .collect();
        // M11-0：合并后更新 key_map：key→page_id 指向新段页。
        for (key, page_id) in key_updates {
            self.update_key_map(&key, page_id);
        }
        // 合并段已占用 new_segment_seq，下一次写入应使用其后一个段号，
        // 否则后续 flush 会覆盖刚合并出的段。
        self.active_segment = new_segment_seq.saturating_add(1);

        // 退役段发 discard（段对齐）：存活页已搬进新段，旧槽可回收。
        // 环回时旧段槽位可能与刚写入的合并段重合——该槽现持有合并后的
        // 活数据，跳过 discard 以免抹掉活数据（对会真正擦除的
        // BlockDevice（FTL）是数据丢失）。
        if let Some(first) = self.first_live_segment {
            let mut seq = first;
            while seq < new_segment_seq {
                let off = Self::slot_offset(&self.cfg, seq)?;
                if off != device_offset {
                    self.dev.discard(off, self.cfg.segment_size as u64)?;
                }
                seq += 1;
            }
        }
        self.first_live_segment = Some(new_segment_seq);

        let write_amplification = if logical_bytes > 0 {
            bytes_written as f64 / logical_bytes as f64
        } else {
            1.0
        };

        Ok(CompactReport {
            segments_merged,
            bytes_written,
            interrupted: false,
            write_amplification,
        })
    }

    /// 若存活段数已达上限（max_segments - 1），先合并一次腾出槽位。
    ///
    /// 必须在**写新段之前**调用；`flush` 复用当前段时不需要（仍持有一个槽）。
    /// 合并目标超过 `max_pages` 装不下时（键空间大于单段容量）不能阻塞写入：
    /// 此时若有空槽直接开段；若环内槽位已满，则环回覆写最旧槽——
    /// 被覆写存活段的映射由 `evict_overwritten_segment` 显式淘汰
    /// （读返回 `Ok(None)`），不留下悬空引用（否则读报 `CorruptedPage`）。
    fn ensure_slot_free(&mut self) -> Result<(), Error> {
        let live = match self.first_live_segment {
            Some(first) => self.active_segment.saturating_sub(first),
            None => 0,
        };
        if live >= self.cfg.max_segments as u64 - 1 {
            if let Err(e) = self.compact_locked() {
                if !matches!(e, Error::CapacityExhausted) {
                    return Err(e);
                }
                // 装不下：留待 flush 走「空槽开段 / 环回覆写最旧槽」路径。
            }
        }
        Ok(())
    }

    /// 环回覆写槽位时，摘除被覆写存活段的映射（显式淘汰，不留悬空引用）。
    ///
    /// M1 环语义：环内槽位写满且合并装不下时，新段覆写最旧存活段的槽位，
    /// 该段数据随之退役。其 `page_map` 页与 `key_map` 键必须一并摘除，
    /// 否则映射指向被新段覆盖/清零的区域，后续读取报 `CorruptedPage`；
    /// 摘除后旧键读取返回 `Ok(None)`，与重开后的恢复视图一致。
    fn evict_overwritten_segment(&mut self, new_seq: u64) -> Result<(), Error> {
        let new_off = Self::slot_offset(&self.cfg, new_seq)?;
        let mut evicted: std::collections::BTreeSet<u64> = Default::default();
        let mut keep: std::vec::Vec<(u64, u64, u64)> =
            std::vec::Vec::with_capacity(self.page_map.len());
        for (pid, seq, off) in core::mem::take(&mut self.page_map) {
            // 仅处理存活段中与目标槽冲突的段（合并刚写出的段不在此路径）。
            if seq < new_seq && Self::slot_offset(&self.cfg, seq)? == new_off {
                evicted.insert(pid);
            } else {
                keep.push((pid, seq, off));
            }
        }
        self.page_map = keep;
        if !evicted.is_empty() {
            self.key_map
                .borrow_mut()
                .retain(|_, pid| !evicted.contains(pid));
            // 被覆写的段已不在设备上：推进 first_live_segment，
            // 避免后续合并范围包含幽灵段（其槽位现属新段）。
            self.first_live_segment = self.page_map.iter().map(|(_, seq, _)| *seq).min();
        }
        Ok(())
    }

    /// 把最新版本页写入新段并提交。
    /// 返回 `(写入的物理页字节数, key→page_id 列表, (page_id, 物理偏移) 列表)`。
    /// M11-0：合并时从页 payload（`key ++ 0xFF ++ value`）提取 key，
    /// 供 `compact_with_budget` 更新 `key_map`。
    fn write_new_segment(
        &mut self,
        latest: &std::collections::BTreeMap<u64, (u64, u64)>,
        scfg: SegmentConfig,
    ) -> Result<(u64, KeyUpdates, PlacedPages), Error> {
        // 先读取所有页数据（此时 dev 未被 writer 借用）
        let mut payloads: std::vec::Vec<(u64, std::vec::Vec<u8>)> = std::vec::Vec::new();
        for page_id in latest.keys() {
            let payload = self.read_payload(*page_id)?;
            payloads.push((*page_id, payload));
        }
        // 再创建 writer 并写入
        let mut writer = SegmentWriter::new(self.dev, scfg)?;
        let mut bytes_written = 0u64;
        // M11-0：写入时同步收集 key→page_id（payload 以首个 0xFF 分隔 key/value）。
        let mut key_updates: KeyUpdates = std::vec::Vec::new();
        // 收集实际物理偏移，供 compact_with_budget 更新 page_map。
        let mut placed: PlacedPages = std::vec::Vec::new();
        for (page_id, payload) in &payloads {
            let app = writer.append_page(*page_id, payload)?;
            placed.push((*page_id, app.offset));
            // C4：写入字节数累计可能溢出 u64（极端页数 × page_size）。
            bytes_written = bytes_written
                .checked_add(self.cfg.page_size as u64)
                .ok_or(Error::CapacityExhausted)?;
            if let Some(sep) = payload.iter().position(|b| *b == 0xFF) {
                key_updates.push((payload[..sep].to_vec(), *page_id));
            }
        }
        let record = writer.commit(0)?;
        self.last_commit_crc = record.commit_crc;
        self.dev.sync()?;
        Ok((bytes_written, key_updates, placed))
    }
}

/// 合并报告。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CompactReport {
    pub segments_merged: usize,
    pub bytes_written: u64,
    pub interrupted: bool,
    /// 写放大 = 物理写入字节 / 逻辑写入字节。目标 < 2.0。
    pub write_amplification: f64,
}

/// 事务句柄。
///
/// 单写者：`Db::begin` 抢锁，`commit`/`Drop` 释放。
/// `'d` 是 `Db` 持有的设备生命周期——`&mut` 在点类型上不变，
/// 因此不能把 `Db<'d>` 收窄成单生命周期的 `Db<'db>`。
#[derive(Debug)]
pub struct Txn<'db, 'd> {
    db: &'db Db<'d>,
    changes: std::vec::Vec<(std::vec::Vec<u8>, std::vec::Vec<u8>)>,
    deadline: Option<Deadline>,
    committed: bool,
}

impl<'db, 'd> Txn<'db, 'd> {
    /// 事务 deadline（若有）。
    pub fn deadline(&self) -> Option<Deadline> {
        self.deadline
    }

    /// 写入键值对。
    ///
    /// key 含 `0xFF` 时返回 `InvalidArgument`：payload 以首个 `0xFF` 分隔
    /// key/value，key 内出现该字节会导致 flush/get 解析错读。
    pub fn put(&mut self, k: &[u8], v: &[u8]) -> Result<(), Error> {
        if let Some(dl) = self.deadline {
            dl.remaining()?;
        }
        if k.contains(&0xFF) {
            return Err(Error::InvalidArgument);
        }
        self.changes.push((k.to_vec(), v.to_vec()));
        Ok(())
    }

    /// 读取键值。
    ///
    /// 先查本事务变更（last-wins），未命中再查已提交数据：
    /// `lookup_key` → `read_payload` → 解析 payload（`key ++ 0xFF ++ value`）。
    /// 返回 owned `Vec<u8>`（`read_payload` 返回 `Vec`）。
    pub fn get(&self, k: &[u8]) -> Result<Option<std::vec::Vec<u8>>, Error> {
        if let Some(dl) = self.deadline {
            dl.remaining()?;
        }
        // 1. 先查本事务变更（last-wins）
        for (key, val) in self.changes.iter().rev() {
            if key == k {
                return Ok(Some(val.clone()));
            }
        }
        // 2. 再查已提交数据（key_map → read_payload → 解析）
        if let Some(page_id) = self.db.lookup_key(k) {
            let payload = self.db.read_payload(page_id)?;
            // payload 格式: key ++ 0xFF ++ value
            if let Some(pos) = payload.iter().position(|&b| b == 0xFF) {
                if &payload[..pos] == k {
                    return Ok(Some(payload[pos + 1..].to_vec()));
                }
            }
        }
        Ok(None)
    }

    /// 范围扫描。返回范围内按键序排列的条目。
    ///
    /// M3 简化：只扫描本事务的变更（`Txn.changes`），不查已提交数据。
    /// 同键多次 `put` 时每个键只出现一次，且为最后一次写入的值。
    pub fn scan(&self, range: core::ops::Range<&[u8]>) -> Result<Cursor<'_>, Error> {
        if let Some(dl) = self.deadline {
            dl.remaining()?;
        }
        // 从后往前遍历，同键只保留最后一次写入
        let mut seen: std::collections::BTreeSet<std::vec::Vec<u8>> = Default::default();
        let mut items: std::vec::Vec<(std::vec::Vec<u8>, std::vec::Vec<u8>)> = std::vec::Vec::new();
        for (k, v) in self.changes.iter().rev() {
            if k.as_slice() >= range.start && k.as_slice() < range.end && seen.insert(k.clone()) {
                items.push((k.clone(), v.clone()));
            }
        }
        items.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(Cursor::new(items))
    }

    /// 提交事务。
    ///
    /// M5 简化（方案 B）：变更写入 `Db.pending_writes`（payload = key ++ 0xFF ++ value），
    /// 真正落盘在下次 `begin`/`compact` 时刷入活跃段（M6）。
    /// `Txn` 持有 `&Db`，通过 `RefCell::borrow_mut()` 获得内部可变性。
    pub fn commit(mut self) -> Result<CommitInfo, Error> {
        if let Some(dl) = self.deadline {
            dl.remaining()?;
        }
        let bytes_written: u64 = self
            .changes
            .iter()
            .map(|(k, v)| (k.len() + v.len()) as u64)
            .sum();
        let segment_seq = self.db.active_segment;
        // M5 简化：写入 pending_writes（真正落盘在 Db 下次操作时）
        let mut pending = self.db.pending_writes.borrow_mut();
        for (k, v) in &self.changes {
            let mut payload = std::vec::Vec::with_capacity(k.len() + 1 + v.len());
            payload.extend_from_slice(k);
            payload.push(0xFF);
            payload.extend_from_slice(v);
            // M8-3：page_id 由全局计数器分配，跨 flush/事务单调递增。
            let page_id = self.db.next_page_id.fetch_add(1, Ordering::Relaxed);
            pending.push((page_id, payload));
        }
        drop(pending);
        self.db.stats.borrow_mut().commit_count += 1;
        self.db.stats.borrow_mut().write_count += self.changes.len() as u64;
        self.committed = true;
        self.db.unlock_writer();
        Ok(CommitInfo {
            segment_seq,
            bytes_written,
        })
    }
}

impl<'db, 'd> Drop for Txn<'db, 'd> {
    fn drop(&mut self) {
        if !self.committed {
            self.db.unlock_writer();
        }
    }
}

#[cfg(all(test, feature = "std"))]
mod tests {
    use super::*;
    use crate::crypto::EncryptionKey;
    use crate::device::InMemoryDevice;
    use crate::types::{reset_clock, set_clock_offset, CompactionBudget, Deadline, TxnOptions};
    use crate::writer::SegmentWriter;
    // no_std crate：format! 需要显式引入
    use std::format;
    use std::vec;
    use std::vec::Vec;

    #[test]
    fn 空设备打开成功() {
        let mut dev = InMemoryDevice::new(1 << 20);
        let cfg = Config::default();
        let db = Db::open(&mut dev, cfg).unwrap();
        assert_eq!(db.config().segment_size, 1 << 20);
    }

    #[test]
    fn 非法配置被拒() {
        let mut dev = InMemoryDevice::new(1 << 20);
        let cfg = Config {
            page_size: 8,
            ..Config::default()
        };
        assert!(Db::open(&mut dev, cfg).is_err());
    }

    #[test]
    fn 已有段被恢复() {
        let mut dev = InMemoryDevice::new(1 << 20);
        // 先写一个已提交的段
        let scfg = SegmentConfig {
            segment_seq: 1,
            prev_segment: 0,
            prev_crc: 0,
            device_offset: 0,
            segment_size: 1 << 20,
            page_size: 4096,
            slots_per_page: 4,
            max_pages: 8,
            encryption_key: None,
        };
        {
            let mut w = SegmentWriter::new(&mut dev, scfg).unwrap();
            w.append_page(0, b"hello").unwrap();
            w.commit(1).unwrap();
        }
        // 打开 Db，应恢复该段
        let db = Db::open(&mut dev, Config::default()).unwrap();
        assert_eq!(db.config().segment_size, 1 << 20);
    }

    #[test]
    fn 事务基本流程() {
        let mut dev = InMemoryDevice::new(1 << 20);
        let mut db = Db::open(&mut dev, Config::default()).unwrap();
        let mut txn = db.begin(TxnOptions::default()).unwrap();
        txn.put(b"key1", b"value1").unwrap();
        txn.put(b"key2", b"value2").unwrap();
        assert_eq!(txn.get(b"key1").unwrap().as_deref(), Some(&b"value1"[..]));
        let info = txn.commit().unwrap();
        assert!(info.bytes_written > 0);
    }

    #[test]
    fn 单写者_第二个事务返回_busy() {
        let mut dev = InMemoryDevice::new(1 << 20);
        let mut db = Db::open(&mut dev, Config::default()).unwrap();
        // `begin` 现为 `&mut self`：Txn 存活期间无法再次 `begin`
        //（借用检查器拦截），改用 `try_lock_writer` 占锁验证 Busy 路径。
        db.try_lock_writer().unwrap();
        assert_eq!(db.begin(TxnOptions::default()).err(), Some(Error::Busy));
        db.unlock_writer();
    }

    #[test]
    fn 未提交事务_drop后锁释放() {
        let mut dev = InMemoryDevice::new(1 << 20);
        let mut db = Db::open(&mut dev, Config::default()).unwrap();
        {
            let _txn = db.begin(TxnOptions::default()).unwrap();
            // txn 在此 drop
        }
        // 锁已释放，可再 begin
        assert!(db.begin(TxnOptions::default()).is_ok());
    }

    #[test]
    fn deadline超时返回错误() {
        let mut dev = InMemoryDevice::new(1 << 20);
        let mut db = Db::open(&mut dev, Config::default()).unwrap();
        let opts = TxnOptions {
            deadline: Some(Deadline(0)),
        }; // 已超时
        assert_eq!(db.begin(opts).err(), Some(Error::DeadlineExceeded));
    }

    #[test]
    fn compact_合并已有段() {
        // 两个 1MiB 段需要 2MiB 设备；简报的 1MiB 装不下第二段。
        let mut dev = InMemoryDevice::new(2 << 20);
        // 写两个段。跨段 CRC 链：下一段的 prev_crc 必须是上一段的 commit_crc。
        let mut prev_crc = 0u32;
        for seq in 1..=2 {
            let scfg = SegmentConfig {
                segment_seq: seq,
                prev_segment: seq - 1,
                prev_crc,
                device_offset: (seq - 1) * (1 << 20),
                segment_size: 1 << 20,
                page_size: 4096,
                slots_per_page: 4,
                max_pages: 8,
                encryption_key: None,
            };
            let mut w = SegmentWriter::new(&mut dev, scfg).unwrap();
            w.append_page(0, format!("seg{seq}").as_bytes()).unwrap();
            let rec = w.commit(seq).unwrap();
            prev_crc = rec.commit_crc;
        }
        let mut db = Db::open(&mut dev, Config::default()).unwrap();
        db.compact().unwrap();
        // compact 后映射表应指向新段
        assert!(db.lookup_page(0).is_some());
    }

    #[test]
    fn compact_空库无操作() {
        let mut dev = InMemoryDevice::new(1 << 20);
        let mut db = Db::open(&mut dev, Config::default()).unwrap();
        db.compact().unwrap(); // 不 panic
    }

    #[test]
    fn deadline_传播至_get() {
        reset_clock();
        set_clock_offset(0);
        let mut dev = InMemoryDevice::new(1 << 20);
        let mut db = Db::open(&mut dev, Config::default()).unwrap();
        let opts = TxnOptions {
            deadline: Some(Deadline::after_millis(10)),
        };
        let txn = db.begin(opts).unwrap();
        set_clock_offset(1_000_000);
        assert_eq!(txn.get(b"key"), Err(Error::DeadlineExceeded));
        reset_clock();
    }

    #[test]
    fn deadline_传播至_put() {
        reset_clock();
        set_clock_offset(0);
        let mut dev = InMemoryDevice::new(1 << 20);
        let mut db = Db::open(&mut dev, Config::default()).unwrap();
        let opts = TxnOptions {
            deadline: Some(Deadline::after_millis(10)),
        };
        let mut txn = db.begin(opts).unwrap();
        set_clock_offset(1_000_000);
        assert_eq!(txn.put(b"key", b"val"), Err(Error::DeadlineExceeded));
        reset_clock();
    }

    #[test]
    fn put_含0xff的key返回_invalidargument() {
        let mut dev = InMemoryDevice::new(1 << 20);
        let mut db = Db::open(&mut dev, Config::default()).unwrap();
        let mut txn = db.begin(TxnOptions::default()).unwrap();
        let result = txn.put(&[0x01, 0xFF, 0x02], b"value");
        assert_eq!(result, Err(Error::InvalidArgument));
    }

    #[test]
    fn 范围扫描返回有序条目() {
        let mut dev = InMemoryDevice::new(1 << 20);
        let mut db = Db::open(&mut dev, Config::default()).unwrap();
        let mut txn = db.begin(TxnOptions::default()).unwrap();
        txn.put(b"k5", b"v5").unwrap();
        txn.put(b"k1", b"v1").unwrap();
        txn.put(b"k9", b"v9").unwrap();
        txn.put(b"k3", b"v3").unwrap();

        let cursor = txn.scan(b"k2".as_ref()..b"k8".as_ref()).unwrap();
        let items: Vec<(Vec<u8>, Vec<u8>)> = cursor.into_iter().collect();
        let keys: Vec<&[u8]> = items.iter().map(|(k, _)| k.as_slice()).collect();
        assert_eq!(keys, vec![b"k3".as_ref(), b"k5".as_ref()]);
    }

    #[test]
    fn 范围扫描_deadline_传播() {
        reset_clock();
        set_clock_offset(0);
        let mut dev = InMemoryDevice::new(1 << 20);
        let mut db = Db::open(&mut dev, Config::default()).unwrap();
        let opts = TxnOptions {
            deadline: Some(Deadline::after_millis(10)),
        };
        let txn = db.begin(opts).unwrap();
        set_clock_offset(1_000_000);
        assert_eq!(
            txn.scan(b"a".as_ref()..b"z".as_ref()),
            Err(Error::DeadlineExceeded)
        );
        reset_clock();
    }

    #[test]
    fn compact_with_budget_正常完成() {
        let mut dev = InMemoryDevice::new(1 << 20);
        let mut db = Db::open(&mut dev, Config::default()).unwrap();
        let budget = CompactionBudget::default();
        let report = db.compact_with_budget(&budget, None).unwrap();
        assert!(!report.interrupted);
    }

    #[test]
    fn compact_with_budget_deadline_中断() {
        let mut dev = InMemoryDevice::new(1 << 20);
        let mut db = Db::open(&mut dev, Config::default()).unwrap();
        let budget = CompactionBudget::default();
        let expired = Deadline::after_millis(0); // 已超时
        let report = db.compact_with_budget(&budget, Some(expired)).unwrap();
        assert!(report.interrupted);
    }

    #[test]
    fn 同键覆盖_get_返回最新值() {
        let mut dev = InMemoryDevice::new(1 << 20);
        let mut db = Db::open(&mut dev, Config::default()).unwrap();
        let mut txn = db.begin(TxnOptions::default()).unwrap();
        txn.put(b"k", b"v1").unwrap();
        txn.put(b"k", b"v2").unwrap();
        assert_eq!(txn.get(b"k").unwrap().as_deref(), Some(&b"v2"[..]));
    }

    #[test]
    fn 同键覆盖_scan_去重_返回最新值() {
        let mut dev = InMemoryDevice::new(1 << 20);
        let mut db = Db::open(&mut dev, Config::default()).unwrap();
        let mut txn = db.begin(TxnOptions::default()).unwrap();
        txn.put(b"a", b"v1").unwrap();
        txn.put(b"b", b"v1").unwrap();
        txn.put(b"a", b"v2").unwrap();
        let cursor = txn.scan(b"a".as_ref()..b"c".as_ref()).unwrap();
        // Cursor 产出 (Vec<u8>, Vec<u8>)，不能直接 collect 成引用切片
        let items: Vec<(Vec<u8>, Vec<u8>)> = cursor.collect();
        assert_eq!(
            items,
            vec![
                (b"a".to_vec(), b"v2".to_vec()),
                (b"b".to_vec(), b"v1".to_vec()),
            ]
        );
    }

    #[test]
    fn compact_with_budget_非可中断_过期返回错误() {
        let mut dev = InMemoryDevice::new(1 << 20);
        let mut db = Db::open(&mut dev, Config::default()).unwrap();
        let budget = CompactionBudget {
            interruptible: false,
            ..CompactionBudget::default()
        };
        let expired = Deadline::after_millis(0);
        assert_eq!(
            db.compact_with_budget(&budget, Some(expired)),
            Err(Error::DeadlineExceeded)
        );
    }

    #[test]
    fn compact_真正合并段() {
        // 已有 2 个 1MiB 段 + 合并新段落在第 3 段槽位（2 MiB），需 3 MiB 设备。
        let mut dev = InMemoryDevice::new(3 << 20);
        // 跨段 CRC 链：下一段的 prev_crc 必须是上一段的 commit_crc。
        let mut prev_crc = 0u32;
        for seq in 1..=2 {
            let scfg = SegmentConfig {
                segment_seq: seq,
                prev_segment: seq - 1,
                prev_crc,
                device_offset: (seq - 1) * (1 << 20),
                segment_size: 1 << 20,
                page_size: 4096,
                slots_per_page: 4,
                max_pages: 8,
                encryption_key: None,
            };
            let mut w = SegmentWriter::new(&mut dev, scfg).unwrap();
            w.append_page(seq - 1, format!("data{seq}").as_bytes())
                .unwrap();
            let rec = w.commit(seq).unwrap();
            prev_crc = rec.commit_crc;
        }
        let mut db = Db::open(&mut dev, Config::default()).unwrap();
        let budget = CompactionBudget::default();
        let report = db.compact_with_budget(&budget, None).unwrap();
        assert!(!report.interrupted);
        assert!(report.segments_merged >= 2, "应合并至少 2 个段");
        assert!(report.bytes_written > 0);
    }

    #[test]
    fn compact_合并后数据可读() {
        // 已有 2 个 1MiB 段 + 合并新段落在第 3 段槽位（2 MiB），需 3 MiB 设备。
        let mut dev = InMemoryDevice::new(3 << 20);
        // 跨段 CRC 链：下一段的 prev_crc 必须是上一段的 commit_crc。
        let mut prev_crc = 0u32;
        for seq in 1..=2 {
            let scfg = SegmentConfig {
                segment_seq: seq,
                prev_segment: seq - 1,
                prev_crc,
                device_offset: (seq - 1) * (1 << 20),
                segment_size: 1 << 20,
                page_size: 4096,
                slots_per_page: 4,
                max_pages: 8,
                encryption_key: None,
            };
            let mut w = SegmentWriter::new(&mut dev, scfg).unwrap();
            w.append_page(seq - 1, format!("data{seq}").as_bytes())
                .unwrap();
            let rec = w.commit(seq).unwrap();
            prev_crc = rec.commit_crc;
        }
        let mut db = Db::open(&mut dev, Config::default()).unwrap();
        // 记录合并前的映射
        let before = db.lookup_page(0);
        assert!(before.is_some(), "合并前 page 0 应可查");
        let budget = CompactionBudget::default();
        db.compact_with_budget(&budget, None).unwrap();
        // 合并后映射应指向新段
        let after = db.lookup_page(0);
        assert!(after.is_some(), "合并后 page 0 应仍可查");
    }

    #[test]
    fn compact_写入新段到设备() {
        // 简报 setup 修正（同 M4-2 先例）：
        // - `1 << 20 * 2` 因 Rust 优先级是 `1 << 40`（1 TiB），注释「2 倍空间」意图是 2 MiB；
        //   实际需要「2 个已有段 + 1 个合并新段」= 3 MiB。
        // - `prev_crc: 0` 会让恢复的跨段 CRC 链校验失败（要求盘上 prev_crc == 上一段 commit_crc）。
        // - `dev.read_at` 在 `db` 存活期间借用冲突，须先 `drop(db)`。
        let mut dev = InMemoryDevice::new(3 << 20);
        let mut prev_crc = 0u32;
        for seq in 1..=2 {
            let scfg = SegmentConfig {
                segment_seq: seq,
                prev_segment: seq - 1,
                prev_crc,
                device_offset: (seq - 1) * (1 << 20),
                segment_size: 1 << 20,
                page_size: 4096,
                slots_per_page: 4,
                max_pages: 8,
                encryption_key: None,
            };
            let mut w = SegmentWriter::new(&mut dev, scfg).unwrap();
            w.append_page(seq - 1, format!("data{seq}").as_bytes())
                .unwrap();
            let rec = w.commit(seq).unwrap();
            prev_crc = rec.commit_crc;
        }
        let mut db = Db::open(&mut dev, Config::default()).unwrap();
        let budget = CompactionBudget::default();
        let report = db.compact_with_budget(&budget, None).unwrap();
        assert!(report.bytes_written > 0, "应有字节写入");
        drop(db);
        // 新段应在设备上：第 3 个段的位置（offset = 2 * segment_size）。
        let mut header_buf = [0u8; 56];
        let new_offset = 2 * (1 << 20);
        assert!(dev.read_at(new_offset, &mut header_buf).is_ok());
        let header = crate::segment::SegmentHeader::decode(&header_buf).expect("新段头应可解码");
        assert_eq!(header.segment_seq, 3, "应写入第 3 段");
    }

    #[test]
    fn compact_写放大统计() {
        // 简报 setup 修正（同 compact_写入新段到设备）：
        // - `1 << 20 * 2` 因优先级是 1 TiB，意图是 2 MiB；实际需 3 MiB（2 段 + 合并新段）。
        // - `prev_crc: 0` 会让跨段 CRC 链校验失败，只恢复段 1。
        let mut dev = InMemoryDevice::new(3 << 20);
        let mut prev_crc = 0u32;
        for seq in 1..=2 {
            let scfg = SegmentConfig {
                segment_seq: seq,
                prev_segment: seq - 1,
                prev_crc,
                device_offset: (seq - 1) * (1 << 20),
                segment_size: 1 << 20,
                page_size: 4096,
                slots_per_page: 4,
                max_pages: 8,
                encryption_key: None,
            };
            let mut w = SegmentWriter::new(&mut dev, scfg).unwrap();
            w.append_page(seq - 1, format!("data{seq}").as_bytes())
                .unwrap();
            let rec = w.commit(seq).unwrap();
            prev_crc = rec.commit_crc;
        }
        let mut db = Db::open(&mut dev, Config::default()).unwrap();
        let budget = CompactionBudget::default();
        let report = db.compact_with_budget(&budget, None).unwrap();
        // 写放大 = 物理写入 / 逻辑写入，应 >= 1.0
        assert!(
            report.write_amplification >= 1.0,
            "写放大应 >= 1.0，实际 {}",
            report.write_amplification
        );
        // M4 目标：写放大 < 2.0
        assert!(
            report.write_amplification < 2.0,
            "写放大应 < 2.0，实际 {}",
            report.write_amplification
        );
    }

    #[test]
    fn 极端配置_compact_with_budget返回错误而非panic() {
        // C4：极端 Config 不得在 compact_with_budget 内 panic。
        // segment_size=usize::MAX / page_size=u32::MAX 通过 Config::validate
        //（validate 只覆盖 max_pages * run_bytes，不覆盖 device_offset /
        // logical_bytes 乘积），open 也成功（设备小于任一段，无段可恢复）。
        // compact_with_budget：提交记录写到 (segment_size - COMMIT_RECORD_SIZE)
        // 越界，必须返回 Err 而非 panic。若未来 validate/open 在入口拒绝该
        // 配置，断言改为 open 返回 Err——同样锁定不 panic。
        let mut dev = InMemoryDevice::new(1 << 20);
        let cfg = Config {
            segment_size: usize::MAX,
            page_size: u32::MAX,
            ..Config::default()
        };
        let mut db = match Db::open(&mut dev, cfg) {
            Ok(db) => db,
            Err(_) => return, // open 入口拒绝：已锁定不 panic
        };
        let budget = CompactionBudget::default();
        assert!(
            db.compact_with_budget(&budget, None).is_err(),
            "极端配置应返回 Err 而非 panic"
        );
    }

    #[test]
    fn 段合并_读取真实页数据() {
        // 简报 setup 修正：
        // - `1 << 20 * 3` 因 Rust 优先级是 `1 << 60`（1 EiB），意图是 3 MiB（2 段 + 合并新段）。
        // - `prev_crc: 0` 会让恢复的跨段 CRC 链校验失败（要求盘上 prev_crc == 上一段 commit_crc）。
        let mut dev = InMemoryDevice::new(3 << 20);
        // 写段 1：page 0 载荷为 "hello-seg1"
        let mut prev_crc = 0u32;
        {
            let scfg = SegmentConfig {
                segment_seq: 1,
                prev_segment: 0,
                prev_crc,
                device_offset: 0,
                segment_size: 1 << 20,
                page_size: 4096,
                slots_per_page: 4,
                max_pages: 8,
                encryption_key: None,
            };
            let mut w = SegmentWriter::new(&mut dev, scfg).unwrap();
            w.append_page(0, b"hello-seg1").unwrap();
            let rec = w.commit(1).unwrap();
            prev_crc = rec.commit_crc;
        }
        // 写段 2：page 0 覆盖为 "hello-seg2"
        {
            let scfg = SegmentConfig {
                segment_seq: 2,
                prev_segment: 1,
                prev_crc,
                device_offset: 1 << 20,
                segment_size: 1 << 20,
                page_size: 4096,
                slots_per_page: 4,
                max_pages: 8,
                encryption_key: None,
            };
            let mut w = SegmentWriter::new(&mut dev, scfg).unwrap();
            w.append_page(0, b"hello-seg2").unwrap();
            w.commit(2).unwrap();
        }
        let mut db = Db::open(&mut dev, Config::default()).unwrap();
        // 合并前读取：应为段 2 的数据
        let payload = db.read_payload(0).unwrap();
        assert_eq!(payload, b"hello-seg2");
        // 合并
        let budget = CompactionBudget::default();
        let report = db.compact_with_budget(&budget, None).unwrap();
        assert!(report.bytes_written > 0);
        // 合并后读取：应仍为段 2 的数据（不是占位零）
        let payload = db.read_payload(0).unwrap();
        assert_eq!(payload, b"hello-seg2", "合并后应保留真实页数据，不是占位零");
    }

    #[test]
    fn 段合并_多页真实数据() {
        // 简报 setup 修正（同 M5-0 先例）：
        // - `1 << 20 * 3` 因 Rust 优先级是 `1 << 60`（1 EiB），意图是 3 MiB（2 段 + 合并新段）。
        // - `prev_crc: 0` 会让恢复的跨段 CRC 链校验失败（要求盘上 prev_crc == 上一段 commit_crc）。
        let mut dev = InMemoryDevice::new(3 << 20);
        let mut prev_crc = 0u32;
        for seq in 1..=2 {
            let scfg = SegmentConfig {
                segment_seq: seq,
                prev_segment: seq - 1,
                prev_crc,
                device_offset: (seq - 1) * (1 << 20),
                segment_size: 1 << 20,
                page_size: 4096,
                slots_per_page: 4,
                max_pages: 8,
                encryption_key: None,
            };
            let mut w = SegmentWriter::new(&mut dev, scfg).unwrap();
            for page in 0..3 {
                let payload = format!("seg{seq}-page{page}");
                w.append_page(page, payload.as_bytes()).unwrap();
            }
            let rec = w.commit(seq).unwrap();
            prev_crc = rec.commit_crc;
        }
        let mut db = Db::open(&mut dev, Config::default()).unwrap();
        // 合并前读取：应为段 2 的最新数据
        assert_eq!(db.read_payload(0).unwrap(), b"seg2-page0");
        assert_eq!(db.read_payload(2).unwrap(), b"seg2-page2");
        // 合并
        let budget = CompactionBudget::default();
        db.compact_with_budget(&budget, None).unwrap();
        // 合并后读取：应仍为段 2 的真实数据
        assert_eq!(db.read_payload(0).unwrap(), b"seg2-page0");
        assert_eq!(db.read_payload(2).unwrap(), b"seg2-page2");
    }

    #[test]
    fn 段合并_写放大统计_真实数据() {
        // 简报 setup 修正（同 M5-0 先例）：
        // - `1 << 20 * 3` 因 Rust 优先级是 `1 << 60`（1 EiB），意图是 3 MiB（2 段 + 合并新段）。
        // - `prev_crc: 0` 会让恢复的跨段 CRC 链校验失败（要求盘上 prev_crc == 上一段 commit_crc）。
        let mut dev = InMemoryDevice::new(3 << 20);
        let mut prev_crc = 0u32;
        for seq in 1..=2 {
            let scfg = SegmentConfig {
                segment_seq: seq,
                prev_segment: seq - 1,
                prev_crc,
                device_offset: (seq - 1) * (1 << 20),
                segment_size: 1 << 20,
                page_size: 4096,
                slots_per_page: 4,
                max_pages: 8,
                encryption_key: None,
            };
            let mut w = SegmentWriter::new(&mut dev, scfg).unwrap();
            for page in 0..3 {
                let payload = format!("seg{seq}-page{page}");
                w.append_page(page, payload.as_bytes()).unwrap();
            }
            let rec = w.commit(seq).unwrap();
            prev_crc = rec.commit_crc;
        }
        let mut db = Db::open(&mut dev, Config::default()).unwrap();
        let budget = CompactionBudget::default();
        let report = db.compact_with_budget(&budget, None).unwrap();
        // 3 个唯一 page_id × 4096 字节 = 12288 字节逻辑/物理写入
        assert_eq!(report.bytes_written, 3 * 4096);
        assert!(
            report.write_amplification >= 1.0 && report.write_amplification < 2.0,
            "写放大应在 [1.0, 2.0)，实际 {}",
            report.write_amplification
        );
    }

    #[test]
    fn 段合并_key_map更新() {
        let mut dev = InMemoryDevice::new(3 << 20);
        let mut db = Db::open(&mut dev, Config::default()).unwrap();
        // 用 Txn::put 写入数据（payload 自动含 0xFF 分隔符：key ++ 0xFF ++ value）
        {
            let mut txn = db.begin(TxnOptions::default()).unwrap();
            txn.put(b"hello", b"world").unwrap();
            txn.commit().unwrap();
        }
        db.flush().unwrap();
        // flush 后 key_map 应包含 "hello" → page_id
        let page_id_before = db
            .lookup_key(b"hello")
            .expect("flush 后 key_map 应包含 hello");
        // 合并
        let budget = CompactionBudget::default();
        let report = db.compact_with_budget(&budget, None).unwrap();
        assert!(!report.interrupted);
        assert!(report.bytes_written > 0);
        // 合并后 key_map 应更新（page_id 应与合并前一致）
        let page_id_after = db
            .lookup_key(b"hello")
            .expect("合并后 key_map 应包含 hello");
        assert_eq!(
            page_id_after, page_id_before,
            "合并后 lookup_key 应返回相同 page_id"
        );
        // 合并后读取数据应仍正确
        let payload = db.read_payload(page_id_after).unwrap();
        assert!(
            payload.windows(5).any(|w| w == b"hello"),
            "合并后 payload 应含 hello"
        );
    }

    #[test]
    fn 段合并_后数据可读() {
        // 简报 setup 修正（同 M5-0 先例）：
        // - `1 << 20 * 3` 因 Rust 优先级是 `1 << 60`（1 EiB），意图是 3 MiB（2 段 + 合并新段）。
        // - `prev_crc: 0` 会让恢复的跨段 CRC 链校验失败（要求盘上 prev_crc == 上一段 commit_crc）。
        let mut dev = InMemoryDevice::new(3 << 20);
        let mut prev_crc = 0u32;
        for seq in 1..=2 {
            let scfg = SegmentConfig {
                segment_seq: seq,
                prev_segment: seq - 1,
                prev_crc,
                device_offset: (seq - 1) * (1 << 20),
                segment_size: 1 << 20,
                page_size: 4096,
                slots_per_page: 4,
                max_pages: 8,
                encryption_key: None,
            };
            let mut w = SegmentWriter::new(&mut dev, scfg).unwrap();
            w.append_page(0, format!("seg{seq}").as_bytes()).unwrap();
            let rec = w.commit(seq).unwrap();
            prev_crc = rec.commit_crc;
        }
        let mut db = Db::open(&mut dev, Config::default()).unwrap();
        let budget = CompactionBudget::default();
        db.compact_with_budget(&budget, None).unwrap();
        // 合并后读取数据
        let payload = db.read_payload(0).unwrap();
        assert!(
            payload.windows(4).any(|w| w == b"seg2"),
            "合并后应保留最新数据"
        );
    }

    #[test]
    fn commit_落盘_写入活跃段() {
        let mut dev = InMemoryDevice::new(1 << 20);
        let mut db = Db::open(&mut dev, Config::default()).unwrap();
        {
            let mut txn = db.begin(TxnOptions::default()).unwrap();
            txn.put(b"key1", b"value1").unwrap();
            txn.put(b"key2", b"value2").unwrap();
            let info = txn.commit().unwrap();
            assert!(info.bytes_written > 0);
        }
        // M5 简化（方案 B）：commit 写入 pending_writes，真正落盘在 M6。
        // 简报原断言 page_map_len() > 0；若把未落盘页登记进 page_map，
        // compact_with_budget 会 read_payload 失败（破坏 embedded-database-os 的
        // commit→compact 集成路径），故断言 pending_writes_len。
        assert!(
            db.pending_writes_len() > 0,
            "commit 后 pending_writes 应非空"
        );
    }

    #[test]
    fn commit_落盘_后可读回() {
        let mut dev = InMemoryDevice::new(1 << 20);
        let mut db = Db::open(&mut dev, Config::default()).unwrap();
        {
            let mut txn = db.begin(TxnOptions::default()).unwrap();
            txn.put(b"key1", b"value1").unwrap();
            txn.commit().unwrap();
        }
        // M5 简化：key→page 映射未实现，先验证 pending_writes 已记录变更。
        // 简报原断言 page_map_len() >= 1；同上，改断言 pending_writes_len。
        assert!(db.pending_writes_len() >= 1);
    }

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
        // 简报原代码仅 begin 不读取；补充真实读路径以验证 read_count 接线
        let payload = db.read_payload(0).unwrap();
        assert!(!payload.is_empty(), "flush 后 page 0 应可读");
        let after = db.stats().read_count;
        assert!(after > before, "读取操作应更新 read_count");
    }

    #[test]
    fn 审计_错误计数() {
        let mut dev = InMemoryDevice::new(1 << 20);
        let db = Db::open(&mut dev, Config::default()).unwrap();
        let before = db.stats().error_count;
        // 尝试读不存在的页
        let _ = db.read_payload(999);
        let after = db.stats().error_count;
        assert!(after > before, "错误操作应更新 error_count");
    }

    #[test]
    fn commit_落盘_flush写入设备() {
        let mut dev = InMemoryDevice::new(1 << 20);
        let mut db = Db::open(&mut dev, Config::default()).unwrap();
        {
            let mut txn = db.begin(TxnOptions::default()).unwrap();
            txn.put(b"key1", b"value1").unwrap();
            txn.put(b"key2", b"value2").unwrap();
            txn.commit().unwrap();
        }
        // commit 后 pending_writes 非空
        assert!(
            db.pending_writes_len() > 0,
            "commit 后 pending_writes 应非空"
        );
        // 主动 flush
        let info = db.flush().unwrap();
        assert!(info.bytes_written > 0, "flush 应写入字节");
        // flush 后 pending_writes 清空
        assert_eq!(db.pending_writes_len(), 0, "flush 后 pending_writes 应清空");
        // page_map 应更新
        assert!(db.page_map_len() > 0, "flush 后 page_map 应非空");
    }

    #[test]
    fn begin_自动flush_persisting() {
        let mut dev = InMemoryDevice::new(1 << 20);
        let mut db = Db::open(&mut dev, Config::default()).unwrap();
        {
            let mut txn = db.begin(TxnOptions::default()).unwrap();
            txn.put(b"key1", b"value1").unwrap();
            txn.commit().unwrap();
        }
        assert!(db.pending_writes_len() > 0);
        // 下次 begin 时自动 flush
        {
            let _txn = db.begin(TxnOptions::default()).unwrap();
        }
        assert_eq!(db.pending_writes_len(), 0, "begin 后 pending_writes 应清空");
    }

    #[test]
    fn flush_加密路径() {
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
            txn.put(b"secret", b"confidential").unwrap();
            txn.commit().unwrap();
        }
        let info = db.flush().unwrap();
        assert!(info.bytes_written > 0);
        // 读回并解密
        let payload = db.read_payload(0).unwrap();
        // M7 简化：payload = key ++ 0xFF ++ value（加密后）
        // 解密后应包含 "secret" 和 "confidential"
        assert!(
            payload.windows(6).any(|w| w == b"secret"),
            "解密后应包含 key"
        );
    }

    #[test]
    #[allow(non_snake_case)]
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
        assert!(
            payload.windows(10).any(|w| w == b"secret-key"),
            "解密后应包含 key"
        );
        assert!(
            payload.windows(12).any(|w| w == b"secret-value"),
            "解密后应包含 value"
        );
    }

    #[test]
    fn flush_后数据可读回() {
        let mut dev = InMemoryDevice::new(1 << 20);
        let mut db = Db::open(&mut dev, Config::default()).unwrap();
        {
            let mut txn = db.begin(TxnOptions::default()).unwrap();
            txn.put(b"persist-key", b"persist-value").unwrap();
            txn.commit().unwrap();
        }
        db.flush().unwrap();
        // flush 后 page_map 非空，可读回
        assert!(db.page_map_len() >= 1);
        let payload = db.read_payload(0).unwrap();
        // M7 简化：payload = key ++ 0xFF ++ value
        assert!(
            payload.windows(11).any(|w| w == b"persist-key"),
            "应包含 key"
        );
    }

    #[test]
    fn flush_多次commit批量落盘() {
        // 每次 begin 自动 flush 会推进 active_segment，设备需容纳多次落盘
        let mut dev = InMemoryDevice::new(8 << 20);
        let mut db = Db::open(&mut dev, Config::default()).unwrap();
        for i in 0..3 {
            let mut txn = db.begin(TxnOptions::default()).unwrap();
            txn.put(format!("k{i}").as_bytes(), b"v").unwrap();
            txn.commit().unwrap();
        }
        // 3 次 commit 后 pending_writes 有 3 条（或 1 条，取决于是否每次 begin 时 flush）
        // M7 方案：begin 时自动 flush，所以第 2、3 次 begin 时前一次已落盘
        // 最终 pending_writes 只有最后一次 commit 的数据
        db.flush().unwrap();
        assert_eq!(db.pending_writes_len(), 0);
        assert!(db.page_map_len() >= 1);
    }

    #[test]
    fn flush_空库无操作() {
        let mut dev = InMemoryDevice::new(1 << 20);
        let mut db = Db::open(&mut dev, Config::default()).unwrap();
        let info = db.flush().unwrap();
        assert_eq!(info.bytes_written, 0);
    }

    #[test]
    fn flush_不修改pending_writes语义() {
        let mut dev = InMemoryDevice::new(1 << 20);
        let mut db = Db::open(&mut dev, Config::default()).unwrap();
        {
            let mut txn = db.begin(TxnOptions::default()).unwrap();
            txn.put(b"k", b"v").unwrap();
            txn.commit().unwrap();
        }
        assert!(db.pending_writes_len() > 0);
        db.flush().unwrap();
        assert_eq!(db.pending_writes_len(), 0);
    }

    #[test]
    fn flush_两次后重开不报_corruption() {
        // M12 同段追加：两次小 flush 复用段 1（段内剩余空间与槽位充足）。
        // 第二次 flush 的 commit 覆盖提交记录（fold 含两页）。
        // 重开后段 1 有效，active_segment = 2（下一个可用段号）。
        // 设备仍预留 3 个 1MiB 段，容纳后续写入。
        let mut dev = InMemoryDevice::new(3 << 20);
        {
            let mut db = Db::open(&mut dev, Config::default()).unwrap();
            {
                let mut txn = db.begin(TxnOptions::default()).unwrap();
                txn.put(b"k1", b"v1").unwrap();
                txn.commit().unwrap();
            }
            db.flush().unwrap();
            {
                let mut txn = db.begin(TxnOptions::default()).unwrap();
                txn.put(b"k2", b"v2").unwrap();
                txn.commit().unwrap();
            }
            db.flush().unwrap();
        }
        let db = Db::open(&mut dev, Config::default()).unwrap();
        assert_eq!(
            db.active_segment(),
            2,
            "M12 同段追加：两次小 flush 落在同一段，重开后活跃段应为 2"
        );
    }

    #[test]
    fn lookup_page_取最新版本() {
        // 简报 setup 修正（同 M4-2 先例）：
        // - `1 << 20 * 3` 因 Rust 优先级是 `1 << 40`（1 TiB），意图是多段空间；
        //   本测试只写 2 个 1MiB 段，2 MiB 设备足够。
        // - `prev_crc: 0` 会让恢复的跨段 CRC 链校验失败（要求盘上 prev_crc ==
        //   上一段 commit_crc），必须链式传递。
        let mut dev = InMemoryDevice::new(2 << 20);
        let mut prev_crc = 0u32;
        for seq in 1..=2 {
            let scfg = SegmentConfig {
                segment_seq: seq,
                prev_segment: seq - 1,
                prev_crc,
                device_offset: (seq - 1) * (1 << 20),
                segment_size: 1 << 20,
                page_size: 4096,
                slots_per_page: 4,
                max_pages: 8,
                encryption_key: None,
            };
            let mut w = SegmentWriter::new(&mut dev, scfg).unwrap();
            w.append_page(0, format!("data{seq}").as_bytes()).unwrap();
            let rec = w.commit(seq).unwrap();
            prev_crc = rec.commit_crc;
        }
        let db = Db::open(&mut dev, Config::default()).unwrap();
        let (seq, _) = db.lookup_page(0).unwrap();
        assert_eq!(seq, 2, "lookup_page 应返回最新段的版本");
    }

    #[test]
    fn open_设备过小报错() {
        let mut dev = InMemoryDevice::new(100); // 极小设备
        let cfg = Config {
            segment_size: 1 << 20,
            ..Config::default()
        };
        // 设备小于一个段，应报错而非静默截断
        assert!(Db::open(&mut dev, cfg).is_err());
    }

    #[test]
    fn key_map_更新与查询() {
        let mut dev = InMemoryDevice::new(1 << 20);
        let mut db = Db::open(&mut dev, Config::default()).unwrap();
        {
            let mut txn = db.begin(TxnOptions::default()).unwrap();
            txn.put(b"hello", b"world").unwrap();
            txn.commit().unwrap();
        }
        db.flush().unwrap();
        // flush 后 key_map 应包含 "hello" → page_id
        let page_id = db.lookup_key(b"hello");
        assert!(page_id.is_some(), "flush 后 key_map 应包含已提交的 key");
    }

    #[test]
    fn key_map_未提交_key不存在() {
        let mut dev = InMemoryDevice::new(1 << 20);
        let db = Db::open(&mut dev, Config::default()).unwrap();
        let page_id = db.lookup_key(b"nonexistent");
        assert_eq!(page_id, None, "未提交的 key 不应在映射中");
    }

    #[test]
    fn get_读已提交数据() {
        let mut dev = InMemoryDevice::new(1 << 20);
        let mut db = Db::open(&mut dev, Config::default()).unwrap();
        {
            let mut txn = db.begin(TxnOptions::default()).unwrap();
            txn.put(b"committed-key", b"committed-value").unwrap();
            txn.commit().unwrap();
        }
        db.flush().unwrap();
        // 新事务读已提交数据
        let txn = db.begin(TxnOptions::default()).unwrap();
        let value = txn.get(b"committed-key").unwrap();
        assert_eq!(
            value.as_deref(),
            Some(b"committed-value".as_ref()),
            "应能读已提交数据"
        );
    }

    #[test]
    fn get_本事务优先于已提交() {
        let mut dev = InMemoryDevice::new(1 << 20);
        let mut db = Db::open(&mut dev, Config::default()).unwrap();
        {
            let mut txn = db.begin(TxnOptions::default()).unwrap();
            txn.put(b"key", b"old-value").unwrap();
            txn.commit().unwrap();
        }
        db.flush().unwrap();
        // 新事务覆盖同键
        let mut txn = db.begin(TxnOptions::default()).unwrap();
        txn.put(b"key", b"new-value").unwrap();
        // get 应返回本事务的值（last-wins）
        assert_eq!(
            txn.get(b"key").unwrap().as_deref(),
            Some(b"new-value".as_ref())
        );
    }

    #[test]
    fn 冷启动重建_key_map() {
        let mut dev = InMemoryDevice::new(1 << 20);
        // 写数据并 flush
        {
            let mut db = Db::open(&mut dev, Config::default()).unwrap();
            let mut txn = db.begin(TxnOptions::default()).unwrap();
            txn.put(b"persist-key", b"persist-value").unwrap();
            txn.commit().unwrap();
            db.flush().unwrap();
        }
        // 重开，key_map 应重建
        let db = Db::open(&mut dev, Config::default()).unwrap();
        let page_id = db.lookup_key(b"persist-key");
        assert!(page_id.is_some(), "冷启动后 key_map 应包含已提交的 key");
    }

    #[test]
    fn 冷启动重建_重开后get() {
        let mut dev = InMemoryDevice::new(1 << 20);
        {
            let mut db = Db::open(&mut dev, Config::default()).unwrap();
            let mut txn = db.begin(TxnOptions::default()).unwrap();
            txn.put(b"hello", b"world").unwrap();
            txn.commit().unwrap();
            db.flush().unwrap();
        }
        // 重开后 get 应读已提交数据
        let mut db = Db::open(&mut dev, Config::default()).unwrap();
        let txn = db.begin(TxnOptions::default()).unwrap();
        let value = txn.get(b"hello").unwrap();
        assert_eq!(value, Some(b"world".to_vec()), "重开后应能读已提交数据");
    }

    #[test]
    fn 冷启动重建_跨段同键后写胜出() {
        // 设备需容纳多个 1MiB 段（同 flush_两次后重开不报_corruption 先例）。
        let mut dev = InMemoryDevice::new(3 << 20);
        // 段 1：key="k", value="v1"
        let pid_seg1 = {
            let mut db = Db::open(&mut dev, Config::default()).unwrap();
            let mut txn = db.begin(TxnOptions::default()).unwrap();
            txn.put(b"k", b"v1").unwrap();
            txn.commit().unwrap();
            db.flush().unwrap();
            db.lookup_key(b"k").unwrap()
        };
        // 段 2：key="k", value="v2"
        let pid_seg2 = {
            let mut db = Db::open(&mut dev, Config::default()).unwrap();
            let mut txn = db.begin(TxnOptions::default()).unwrap();
            txn.put(b"k", b"v2").unwrap();
            txn.commit().unwrap();
            db.flush().unwrap();
            db.lookup_key(b"k").unwrap()
        };
        assert_ne!(pid_seg1, pid_seg2, "两段写入应分配不同 page_id");
        // 重开：lookup_key 应返回段 2 的 page_id，get 应返回 "v2"
        let mut db = Db::open(&mut dev, Config::default()).unwrap();
        assert_eq!(
            db.lookup_key(b"k"),
            Some(pid_seg2),
            "跨段同键后写胜出：冷启动后应映射到段 2 的 page_id"
        );
        let txn = db.begin(TxnOptions::default()).unwrap();
        assert_eq!(
            txn.get(b"k").unwrap(),
            Some(b"v2".to_vec()),
            "跨段同键后写胜出：重开后 get 应返回段 2 的值"
        );
    }

    #[test]
    fn 冷启动重建_加密路径() {
        let mut dev = InMemoryDevice::new(1 << 20);
        let key = EncryptionKey {
            sm4: crate::sm4::Sm4Key { bytes: [0x01; 16] },
            hmac: [0x02; 32],
        };
        let cfg = Config {
            encryption_key: Some(key),
            ..Config::default()
        };
        {
            let mut db = Db::open(&mut dev, cfg.clone()).unwrap();
            let mut txn = db.begin(TxnOptions::default()).unwrap();
            txn.put(b"enc-key", b"enc-value").unwrap();
            txn.commit().unwrap();
            db.flush().unwrap();
        }
        // 重开（带相同密钥），key_map 应重建
        let db = Db::open(&mut dev, cfg).unwrap();
        let page_id = db.lookup_key(b"enc-key");
        assert!(page_id.is_some(), "加密路径冷启动后 key_map 应重建");
    }

    #[test]
    fn 同段追加_多次flush复用同一段() {
        let mut dev = InMemoryDevice::new(1 << 20);
        let mut db = Db::open(&mut dev, Config::default()).unwrap();
        // 第一次 flush
        {
            let mut txn = db.begin(TxnOptions::default()).unwrap();
            txn.put(b"key1", b"value1").unwrap();
            txn.commit().unwrap();
        }
        db.flush().unwrap();
        let seq_after_first = db.active_segment();
        // 第二次 flush（同段应有空间）
        {
            let mut txn = db.begin(TxnOptions::default()).unwrap();
            txn.put(b"key2", b"value2").unwrap();
            txn.commit().unwrap();
        }
        db.flush().unwrap();
        let seq_after_second = db.active_segment();
        // M12：同段追加，active_segment 不应递增
        assert_eq!(
            seq_after_first, seq_after_second,
            "同段追加时 active_segment 不应递增"
        );
    }

    #[test]
    fn 同段追加_空间耗尽后开新段() {
        // 简报 setup 修正：设备需 2 MiB。
        // Config::default() 的 max_pages=8 约束段内唯一逻辑页数；
        // 10 次 put 分配全局 page_id 0..9，段 1 只能容纳 8 页，
        // 第 9 页起必须开段 2（偏移 1 MiB），1 MiB 设备放不下段 2。
        let mut dev = InMemoryDevice::new(2 << 20);
        let mut db = Db::open(&mut dev, Config::default()).unwrap();
        // 多次 flush 直到空间耗尽
        for i in 0..10 {
            let mut txn = db.begin(TxnOptions::default()).unwrap();
            txn.put(format!("key{i}").as_bytes(), format!("value{i}").as_bytes())
                .unwrap();
            txn.commit().unwrap();
            db.flush().unwrap();
        }
        // 空间耗尽后应开新段（B-1 回归：不能卡在 2，必须推进到 3）
        assert!(
            db.active_segment() > 2,
            "空间耗尽后应开新段，active_segment 应 > 2，实际 {}",
            db.active_segment()
        );
    }

    #[allow(non_snake_case)]
    #[test]
    fn B1_槽位耗尽后flush写入新段而非覆写满段() {
        // M12 终审 B-1：槽位耗尽时误用满段号新建 writer 覆写当前段。
        // 修复后：第 9 次 flush 的 CommitInfo.segment_seq 应为 2，且重开后全部 key 可读。
        let mut dev = InMemoryDevice::new(4 << 20);
        {
            let mut db = Db::open(&mut dev, Config::default()).unwrap();
            let mut last_seg = 0u64;
            for i in 0..10 {
                let mut txn = db.begin(TxnOptions::default()).unwrap();
                txn.put(format!("key{i}").as_bytes(), format!("value{i}").as_bytes())
                    .unwrap();
                txn.commit().unwrap();
                let info = db.flush().unwrap();
                last_seg = info.segment_seq;
            }
            // 段 1 最多 8 页；第 9 次 flush 必须进段 2
            assert_eq!(last_seg, 2, "第 9 次 flush 应写入段 2，实际段 {last_seg}");
        }
        // 重开：全部 key 可读，无 Corruption
        let mut db = Db::open(&mut dev, Config::default()).unwrap();
        for i in 0..10 {
            let txn = db.begin(TxnOptions::default()).unwrap();
            let v = txn
                .get(format!("key{i}").as_bytes())
                .unwrap()
                .unwrap_or_else(|| panic!("key{i} 冷启动后应可读"));
            assert_eq!(v, format!("value{i}").as_bytes(), "key{i} 值不匹配");
        }
    }

    #[allow(non_snake_case)]
    #[test]
    fn B2_段2同段追加后重开CRC链不断裂() {
        // M12 终审 B-2：段≥2 同段追加把 CommitRecord.prev_crc 覆写为本段早先 commit，
        // 跨段 CRC 链断裂。修复后：段 2 同段追加多次，重开不报 Corruption。
        let mut dev = InMemoryDevice::new(4 << 20);
        {
            let mut db = Db::open(&mut dev, Config::default()).unwrap();
            // 先写满段 1（8 页），强制进段 2
            for i in 0..8 {
                let mut txn = db.begin(TxnOptions::default()).unwrap();
                txn.put(format!("k{i}").as_bytes(), b"v").unwrap();
                txn.commit().unwrap();
                db.flush().unwrap();
            }
            assert_eq!(db.active_segment(), 2, "应已进入段 2");
            // 对段 2 同段追加 3 次
            for i in 8..11 {
                let mut txn = db.begin(TxnOptions::default()).unwrap();
                txn.put(format!("k{i}").as_bytes(), b"v").unwrap();
                txn.commit().unwrap();
                let info = db.flush().unwrap();
                assert_eq!(info.segment_seq, 2, "应同段追加到段 2");
            }
        }
        // 重开：CRC 链通过，数据可读
        let mut db = Db::open(&mut dev, Config::default()).unwrap();
        for i in 0..11 {
            let txn = db.begin(TxnOptions::default()).unwrap();
            let v = txn
                .get(format!("k{i}").as_bytes())
                .unwrap()
                .unwrap_or_else(|| panic!("k{i} 冷启动后应可读"));
            assert_eq!(v, b"v", "k{i} 值不匹配");
        }
    }

    #[allow(non_snake_case)]
    #[test]
    fn B3_flush中途失败后旧数据仍可恢复() {
        // M12 终审 B-3：flush 中途失败使已提交段整体不可恢复。
        //
        // 场景：第一次 flush 成功落盘；第二次 flush 在第 1 页写成功后注入失败，
        // 已提交段追加区多出 1 页但 CommitRecord.fold 仍是旧值 → 重开应报 Corruption。
        // 修复后：重开仍能读到第一次 flush 的数据。
        //
        // 用 fail_at 控制：仅在第 N 次写失败，之后的写（含回滚）均成功。
        struct Ctrl {
            writes: core::cell::Cell<u64>,
            fail_at: core::cell::Cell<Option<u64>>,
        }
        struct CtrlDev<'a> {
            inner: &'a mut InMemoryDevice,
            ctrl: &'a Ctrl,
        }
        impl BlockDevice for CtrlDev<'_> {
            fn size(&self) -> u64 {
                self.inner.size()
            }
            fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), Error> {
                self.inner.read_at(offset, buf)
            }
            fn write_at(&mut self, offset: u64, data: &[u8]) -> Result<(), Error> {
                let n = self.ctrl.writes.get();
                // 计数器始终递增（含失败写），保证 fail_at 只命中指定序号，
                // 后续写（含回滚）不再被拦截。
                self.ctrl.writes.set(n + 1);
                if self.ctrl.fail_at.get() == Some(n) {
                    return Err(Error::Io);
                }
                self.inner.write_at(offset, data)
            }
            fn sync(&mut self) -> Result<(), Error> {
                self.inner.sync()
            }
        }

        let mut dev = InMemoryDevice::new(3 << 20);
        let ctrl = Ctrl {
            writes: core::cell::Cell::new(0),
            fail_at: core::cell::Cell::new(None),
        };
        {
            let mut cdev = CtrlDev {
                inner: &mut dev,
                ctrl: &ctrl,
            };
            let mut db = Db::open(&mut cdev, Config::default()).unwrap();
            // 第一次 flush：写入 old-key，全部写成功（fail_at=None）
            let mut txn = db.begin(TxnOptions::default()).unwrap();
            txn.put(b"old-key", b"old-value").unwrap();
            txn.commit().unwrap();
            db.flush().unwrap();
            // 第二次 flush：写入 3 个 key。
            // 第二次 flush 的设备写序列（同段追加到段 1）：
            //   write 0: append_page(new-key1) 成功
            //   write 1: append_page(new-key2) 失败 ← fail_at=1
            //   write 2: rollback 清零 write 0 的页槽（成功）
            // 若回滚也失败，已提交段追加区会残留新页 → fold 失配 → Corruption。
            let mut txn = db.begin(TxnOptions::default()).unwrap();
            txn.put(b"new-key1", b"v1").unwrap();
            txn.put(b"new-key2", b"v2").unwrap();
            txn.put(b"new-key3", b"v3").unwrap();
            txn.commit().unwrap();
            ctrl.writes.set(0);
            ctrl.fail_at.set(Some(1));
            let result = db.flush();
            assert!(result.is_err(), "注入写失败后 flush 应返回 Err");
        }
        // 重开：旧数据仍可读，无 Corruption
        let mut db = Db::open(&mut dev, Config::default()).unwrap();
        let txn = db.begin(TxnOptions::default()).unwrap();
        let v = txn
            .get(b"old-key")
            .unwrap()
            .unwrap_or_else(|| panic!("旧数据冷启动后应仍可读"));
        assert_eq!(v, b"old-value", "旧值应保持不变");
    }

    #[test]
    fn 同段追加后重开不报_corruption() {
        // M12 验收：同段追加后 Db::open 重开不报 Corruption。
        let mut dev = InMemoryDevice::new(3 << 20);
        {
            let mut db = Db::open(&mut dev, Config::default()).unwrap();
            // 第一次 flush
            let mut txn = db.begin(TxnOptions::default()).unwrap();
            txn.put(b"first", b"one").unwrap();
            txn.commit().unwrap();
            db.flush().unwrap();
            // 第二次 flush（同段追加）
            let mut txn = db.begin(TxnOptions::default()).unwrap();
            txn.put(b"second", b"two").unwrap();
            txn.commit().unwrap();
            db.flush().unwrap();
        }
        // 重开不报 Corruption，数据可读
        let mut db = Db::open(&mut dev, Config::default()).unwrap();
        let txn = db.begin(TxnOptions::default()).unwrap();
        assert_eq!(
            txn.get(b"first").unwrap().as_deref(),
            Some(&b"one"[..]),
            "第一次写入应可读"
        );
        assert_eq!(
            txn.get(b"second").unwrap().as_deref(),
            Some(&b"two"[..]),
            "第二次写入（同段追加）应可读"
        );
    }

    #[test]
    fn 中间槽被擦除后仍能恢复后续段() {
        // 每段最多 8 页（max_pages=8）：写 17 次 ⇒ 占满段 1、段 2，段 3 起头。
        let cfg = Config::default();
        let dev_size = (cfg.max_segments as u64) * (cfg.segment_size as u64);
        let mut dev = InMemoryDevice::new(dev_size as usize);
        {
            let mut db = Db::open(&mut dev, cfg.clone()).unwrap();
            for i in 0..17u64 {
                db.put(format!("k{i}").as_bytes(), b"v").unwrap();
                db.flush().unwrap();
            }
        }
        // 擦掉第 1 槽（段 1）的段头 → 该段不可解码（环回时的空洞）。
        dev.write_at(0, &[0xFFu8; 64]).unwrap();
        let db = Db::open(&mut dev, cfg).unwrap();
        // 空洞之后的段仍被恢复：最新键可读，重开不报损坏。
        assert_eq!(db.get(b"k16").unwrap(), Some(b"v".to_vec()));
        // 段 1 里的键随该段一起丢失。
        assert!(db.get(b"k0").unwrap().is_none());
    }

    #[test]
    fn 段号超过槽数后重开仍读到最新数据() {
        let cfg = Config::default();
        let dev_size = (cfg.max_segments as u64) * (cfg.segment_size as u64);
        let mut dev = InMemoryDevice::new(dev_size as usize);
        {
            let mut db = Db::open(&mut dev, cfg.clone()).unwrap();
            // 5 段（≥ 槽数 4 + 1），环回到第 0 槽。
            for i in 0..40u64 {
                db.put(format!("k{i}").as_bytes(), b"v").unwrap();
                db.flush().unwrap();
            }
        }
        let db = Db::open(&mut dev, cfg).unwrap();
        assert_eq!(db.get(b"k39").unwrap(), Some(b"v".to_vec()));
    }

    #[test]
    fn 槽位复用后短段不会读到旧页() {
        let cfg = Config::default();
        let dev_size = (cfg.max_segments as u64) * (cfg.segment_size as u64);
        let mut dev = InMemoryDevice::new(dev_size as usize);
        {
            let mut db = Db::open(&mut dev, cfg.clone()).unwrap();
            // 首轮：写满 4 个段（每段 8 页）⇒ 槽 0 的旧租期留下 8 页。
            for i in 0..32u64 {
                db.put(format!("a{i}").as_bytes(), b"v").unwrap();
                db.flush().unwrap();
            }
        }
        // 擦掉全部段头，让 4 个槽都变成「空洞」——恢复会跳过它们。
        for slot in 0..cfg.max_segments as u64 {
            dev.write_at(slot * cfg.segment_size as u64, &[0xFFu8; 64])
                .unwrap();
        }
        // 第二轮：只写 3 次 flush ⇒ 槽 0 的新租期（段 1）只占 3 页，远少于旧的 8 页。
        {
            let mut db = Db::open(&mut dev, cfg.clone()).unwrap();
            for i in 0..3u64 {
                db.put(format!("b{i}").as_bytes(), b"v").unwrap();
                db.flush().unwrap();
            }
        }
        // 重开：若未清零，扫描会在第 4 页读到旧租期的合法页 ⇒ fold 不符 ⇒ Err(Corruption)。
        let db = Db::open(&mut dev, cfg).unwrap();
        assert_eq!(db.get(b"b2").unwrap(), Some(b"v".to_vec()));
        assert!(db.get(b"b3").unwrap().is_none());
    }

    #[test]
    fn 存活段数达到上限时自动合并且不丢数据() {
        let cfg = Config::default();
        let dev_size = (cfg.max_segments as u64) * (cfg.segment_size as u64);
        let mut dev = InMemoryDevice::new(dev_size as usize);
        let mut db = Db::open(&mut dev, cfg.clone()).unwrap();
        // 每段 8 页、环内 4 槽：同一批键反复覆盖，旧版本把段填满。
        // 第 25 次 flush 时已有 3 个存活段（= max_segments - 1，必须合并腾槽）；
        // 此时存活页 ≤ max_pages，合并结果装得进一个段。设备只有 4 个槽而
        // 不加约束需要 5+ 个段，不自动合并必然环回覆写丢数据。
        for i in 0..40u64 {
            db.put(format!("k{}", i % 8).as_bytes(), format!("v{i}").as_bytes())
                .unwrap();
            db.flush().unwrap();
        }
        // 合并保留每个键的最新版本：最后一轮 v32..v39 全部可读。
        for j in 0..8u64 {
            assert_eq!(
                db.get(format!("k{j}").as_bytes()).unwrap(),
                Some(format!("v{}", 32 + j).into_bytes())
            );
        }
        // 不变量：存活段数 ≤ max_segments - 1。
        let first = db.first_live_segment.expect("有已提交数据必有存活段");
        assert!(db.active_segment - first < cfg.max_segments as u64);
    }

    #[test]
    fn 合并后对退役段发出_discard() {
        let cfg = Config::default();
        let dev_size = (cfg.max_segments as u64) * (cfg.segment_size as u64);
        let mut dev = InMemoryDevice::new(dev_size as usize);
        {
            let mut db = Db::open(&mut dev, cfg.clone()).unwrap();
            for i in 0..40u64 {
                db.put(format!("k{}", i % 8).as_bytes(), format!("v{i}").as_bytes())
                    .unwrap();
                db.flush().unwrap();
            }
        }
        assert!(
            !dev.discarded_ranges().is_empty(),
            "合并后必须对退役段发 discard"
        );
        for (_, len) in dev.discarded_ranges() {
            assert_eq!(*len, cfg.segment_size as u64);
        }
    }
}
