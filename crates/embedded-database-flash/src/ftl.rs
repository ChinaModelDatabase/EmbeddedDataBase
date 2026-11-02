//! 擦除块级 FTL：把 `RawFlash` 包装成 core 的 `BlockDevice`。

use alloc::vec::Vec;

use embedded_database_core::{BlockDevice, Error, Result};

use crate::raw::{FlashError, RawFlash};

/// 块头魔数。
pub const HEADER_MAGIC: [u8; 4] = *b"WHFT";
/// 块头版本。
pub const HEADER_VERSION: u8 = 1;
/// 块头字节数（占块内第 0 页的前 32 字节）。
pub const HEADER_SIZE: usize = 32;
/// 块头 `state` 字段：`1` 表示该块持有有效的逻辑块数据。
const STATE_DATA: u8 = 1;
/// L2P 中"未映射"的哨兵值。
const UNMAPPED: u32 = u32::MAX;

/// FTL 可调参数。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FtlConfig {
    /// 预留空间百分比（OP）。1..=100。逻辑容量 = `block_count × (100 - op) / 100`。
    pub op_percent: u8,
    /// 静态磨损均衡间隔：每完成这么多次擦除，尝试搬移一个冷块。0 = 关闭。
    pub static_level_interval: u64,
    /// 空闲块最大擦除计数与最小擦除计数之差超过它就触发静态均衡。0 = 关闭。
    pub wear_gap_threshold: u32,
}

impl Default for FtlConfig {
    fn default() -> Self {
        FtlConfig {
            op_percent: 20,
            static_level_interval: 64,
            wear_gap_threshold: 2,
        }
    }
}

/// FTL 运行统计。`write_amplification = physical_program_bytes / logical_write_bytes`
/// （逻辑写字节为 0 时为 0.0）。
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct FtlStats {
    /// 上层请求写入的逻辑字节数。
    pub logical_write_bytes: u64,
    /// 实际编程到介质的字节数（含搬迁与块头）。
    pub physical_program_bytes: u64,
    /// 写放大。
    pub write_amplification: f64,
    /// 擦除操作次数。
    pub erase_ops: u64,
    /// 空闲块中的最小擦除计数。
    pub erase_count_min: u32,
    /// 空闲块中的最大擦除计数。
    pub erase_count_max: u32,
    /// 块搬迁次数。
    pub block_moves: u64,
    /// 坏块数。
    pub bad_blocks: u32,
    /// 空闲块数。
    pub free_blocks: u32,
    /// 待回收（Dirty）块数。
    pub retired_blocks: u32,
}

/// 单个物理块的运行时状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BlockState {
    /// 已擦除、可分配。
    Free,
    /// 持有某个逻辑块的有效数据。
    Data,
    /// 曾持有数据但已被新版本取代（或块头不可信），擦除后即可重用。
    Dirty,
    /// 介质故障，永久退役。
    Bad,
}

/// RAM 中的每物理块元数据。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Phys {
    state: BlockState,
    logical: u32,
    alloc_seq: u64,
    erase_count: u32,
}

/// 落盘块头（32 字节，CRC 覆盖前 28 字节）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Header {
    logical: u32,
    alloc_seq: u64,
    erase_count: u32,
}

/// 把块头编码成 32 字节。
fn encode_header(h: Header) -> [u8; HEADER_SIZE] {
    let mut buf = [0u8; HEADER_SIZE];
    buf[0..4].copy_from_slice(&HEADER_MAGIC);
    buf[4] = HEADER_VERSION;
    buf[5] = STATE_DATA;
    buf[8..12].copy_from_slice(&h.logical.to_le_bytes());
    buf[12..20].copy_from_slice(&h.alloc_seq.to_le_bytes());
    buf[20..24].copy_from_slice(&h.erase_count.to_le_bytes());
    let crc = embedded_database_core::hash_bytes(&buf[..28]);
    buf[28..32].copy_from_slice(&crc.to_le_bytes());
    buf
}

/// 解码块头。魔数/版本/状态/CRC 任一不符即视为"无有效块头"。
fn decode_header(buf: &[u8]) -> Option<Header> {
    if buf.len() < HEADER_SIZE {
        return None;
    }
    if buf[0..4] != HEADER_MAGIC || buf[4] != HEADER_VERSION || buf[5] != STATE_DATA {
        return None;
    }
    let expect = embedded_database_core::hash_bytes(&buf[..28]);
    let got = u32::from_le_bytes([buf[28], buf[29], buf[30], buf[31]]);
    if expect != got {
        return None;
    }
    Some(Header {
        logical: u32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]),
        alloc_seq: u64::from_le_bytes([
            buf[12], buf[13], buf[14], buf[15], buf[16], buf[17], buf[18], buf[19],
        ]),
        erase_count: u32::from_le_bytes([buf[20], buf[21], buf[22], buf[23]]),
    })
}

/// 擦除块级 FTL。
///
/// 对外是一个连续的逻辑块设备（逻辑块 = 逻辑页 1..15 = 3840 字节）。
/// 对内维护 `l2p` 映射与每物理块状态；提交点是块头的成功编程。
pub struct Ftl<R: RawFlash> {
    raw: R,
    cfg: FtlConfig,
    page_size: usize,
    #[allow(dead_code)]
    erase_block_size: usize,
    logical_block_size: usize,
    block_count: u32,
    logical_blocks: u32,
    l2p: Vec<u32>,
    phys: Vec<Phys>,
    next_alloc_seq: u64,
    /// 静态均衡的递归守卫（Task 3 加入）。
    leveling: bool,
    stats: FtlStats,
}

impl<R: RawFlash> Ftl<R> {
    /// 用给定参数构造并挂载 FTL。
    ///
    /// 几何校验失败、或 `op_percent` 非法时返回 [`Error::InvalidArgument`]。
    pub fn new(raw: R, cfg: FtlConfig) -> Result<Self> {
        let page_size = raw.page_size();
        let erase_block_size = raw.erase_block_size();
        let block_count = raw.block_count();
        if page_size < 64 || !page_size.is_power_of_two() {
            return Err(Error::InvalidArgument);
        }
        if erase_block_size % page_size != 0 || erase_block_size / page_size < 2 {
            return Err(Error::InvalidArgument);
        }
        if cfg.op_percent == 0 || cfg.op_percent > 100 {
            return Err(Error::InvalidArgument);
        }
        let logical_blocks =
            (u64::from(block_count) * u64::from(100 - cfg.op_percent) / 100) as u32;
        if block_count < 4 || logical_blocks < 2 {
            return Err(Error::InvalidArgument);
        }
        let logical_block_size = erase_block_size - page_size;
        if u64::from(logical_blocks)
            .checked_mul(logical_block_size as u64)
            .is_none()
        {
            return Err(Error::InvalidArgument);
        }
        let mut ftl = Ftl {
            raw,
            cfg,
            page_size,
            erase_block_size,
            logical_block_size,
            block_count,
            logical_blocks,
            l2p: alloc::vec![UNMAPPED; logical_blocks as usize],
            phys: alloc::vec![
                Phys { state: BlockState::Free, logical: UNMAPPED, alloc_seq: 0, erase_count: 0 };
                block_count as usize
            ],
            next_alloc_seq: 1,
            leveling: false,
            stats: FtlStats::default(),
        };
        ftl.mount()?;
        ftl.refresh_stats();
        Ok(ftl)
    }

    /// 取出底层设备（供测试做 L2P 重建/掉电复验）。
    pub fn into_raw(self) -> R {
        self.raw
    }

    /// 逻辑容量（字节）。
    pub fn logical_size(&self) -> u64 {
        u64::from(self.logical_blocks) * self.logical_block_size as u64
    }

    /// 挂载：扫描全部物理块的块头，重建 L2P。
    fn mount(&mut self) -> Result<()> {
        let mut buf = [0u8; HEADER_SIZE];
        let mut max_ec = 0u32;
        let mut found: Vec<(u32, Header)> = Vec::new();
        for block in 0..self.block_count {
            match self.raw.read(block, 0, &mut buf) {
                Ok(()) => {}
                Err(FlashError::BadBlock { .. }) | Err(FlashError::Read { .. }) => {
                    self.phys[block as usize].state = BlockState::Bad;
                    continue;
                }
                Err(_) => return Err(Error::Io),
            }
            if let Some(h) = decode_header(&buf) {
                if h.erase_count > max_ec {
                    max_ec = h.erase_count;
                }
                found.push((block, h));
            }
        }

        // 同一逻辑块多个候选：alloc_seq 最大者胜，其余标 Dirty。
        for (block, h) in found {
            let idx = block as usize;
            if h.logical == UNMAPPED || h.logical >= self.logical_blocks {
                self.phys[idx] = Phys {
                    state: BlockState::Dirty,
                    logical: h.logical,
                    alloc_seq: h.alloc_seq,
                    erase_count: h.erase_count,
                };
                continue;
            }
            let li = h.logical as usize;
            let cur = self.l2p[li];
            let replace = cur == UNMAPPED || self.phys[cur as usize].alloc_seq < h.alloc_seq;
            if replace {
                if cur != UNMAPPED {
                    let c = cur as usize;
                    self.phys[c] = Phys {
                        state: BlockState::Dirty,
                        ..self.phys[c]
                    };
                }
                self.l2p[li] = block;
                self.phys[idx] = Phys {
                    state: BlockState::Data,
                    logical: h.logical,
                    alloc_seq: h.alloc_seq,
                    erase_count: h.erase_count,
                };
                if h.alloc_seq >= self.next_alloc_seq {
                    self.next_alloc_seq = h.alloc_seq.saturating_add(1);
                }
            } else {
                self.phys[idx] = Phys {
                    state: BlockState::Dirty,
                    logical: h.logical,
                    alloc_seq: h.alloc_seq,
                    erase_count: h.erase_count,
                };
            }
        }

        // 块头不可信的空闲块：擦除计数不可知，保守取已知最大值。
        for p in self.phys.iter_mut() {
            if p.state == BlockState::Free {
                p.erase_count = p.erase_count.max(max_ec);
            }
        }
        Ok(())
    }

    /// 选一个空闲块；没有空闲则先回收一个 Dirty 块。都不行则 `CapacityExhausted`。
    fn alloc_block(&mut self, logical: u32) -> Result<u32> {
        let block = match self.pick_free() {
            Some(b) => b,
            None => {
                self.reclaim_one_dirty()?;
                self.pick_free().ok_or(Error::CapacityExhausted)?
            }
        };
        let idx = block as usize;
        let ec = self.phys[idx].erase_count;
        self.phys[idx] = Phys {
            state: BlockState::Data,
            logical,
            alloc_seq: self.next_alloc_seq,
            erase_count: ec,
        };
        self.next_alloc_seq = self.next_alloc_seq.saturating_add(1);
        Ok(block)
    }

    /// 空闲块中擦除计数最小者。
    fn pick_free(&self) -> Option<u32> {
        let mut best: Option<u32> = None;
        for (i, p) in self.phys.iter().enumerate() {
            if p.state != BlockState::Free {
                continue;
            }
            match best {
                None => best = Some(i as u32),
                Some(b) if p.erase_count < self.phys[b as usize].erase_count => {
                    best = Some(i as u32)
                }
                _ => {}
            }
        }
        best
    }

    /// 回收一个 Dirty 块（擦除后变 Free）。没有 Dirty 块则空操作。
    fn reclaim_one_dirty(&mut self) -> Result<()> {
        let victim = self
            .phys
            .iter()
            .position(|p| p.state == BlockState::Dirty)
            .map(|i| i as u32);
        if let Some(v) = victim {
            self.erase_block(v)?;
        }
        Ok(())
    }

    /// 擦除一块；介质故障则永久标记为坏块（不算操作失败）。
    fn erase_block(&mut self, block: u32) -> Result<()> {
        match self.raw.erase(block) {
            Ok(()) => {
                let idx = block as usize;
                self.phys[idx] = Phys {
                    state: BlockState::Free,
                    logical: UNMAPPED,
                    alloc_seq: 0,
                    erase_count: self.phys[idx].erase_count.saturating_add(1),
                };
                self.stats.erase_ops += 1;
                self.refresh_stats();
                // 静态均衡：失败不影响本次擦除（块已变 Free）。
                let _ = self.maybe_static_level();
                Ok(())
            }
            Err(FlashError::BadBlock { .. }) | Err(FlashError::Erase { .. }) => {
                self.phys[block as usize].state = BlockState::Bad;
                Ok(())
            }
            Err(_) => Err(Error::Io),
        }
    }

    /// 把一个逻辑块载荷（长度必须是 `logical_block_size`）写进指定物理块。
    ///
    /// 按页拆分编程（`RawFlash::program` 不允许跨页）。**不**提交块头——
    /// 调用方在数据落盘后再调 `commit_header`。
    fn program_payload(&mut self, phys: u32, buf: &[u8]) -> Result<()> {
        for (i, chunk) in buf.chunks(self.page_size).enumerate() {
            self.raw
                .program(phys, self.page_size + i * self.page_size, chunk)
                .map_err(|_| Error::Io)?;
            self.stats.physical_program_bytes += chunk.len() as u64;
        }
        Ok(())
    }

    /// 单次逻辑块内写：必要时分配或搬迁，最后统一提交块头。
    ///
    /// 循环上限 4 次只为"换掉中途变坏的块"，不是重试语义：坏块每出现一次就
    /// 退役一个物理块，空闲池有限，因此必须有界，避免无限递归。
    fn write_chunk(&mut self, logical: u32, within: usize, data: &[u8]) -> Result<()> {
        for _attempt in 0..4 {
            let mut phys = self.l2p[logical as usize];
            let mut fresh = false;
            if phys == UNMAPPED {
                phys = self.alloc_block(logical)?;
                self.l2p[logical as usize] = phys;
                fresh = true;
            }
            let off = self.page_size + within;
            match self.raw.program(phys, off, data) {
                Ok(()) => {
                    self.stats.logical_write_bytes += data.len() as u64;
                    self.stats.physical_program_bytes += data.len() as u64;
                    if fresh {
                        self.commit_header(phys)?;
                    }
                    return Ok(());
                }
                // 0→1 冲突或硬错误：整块搬迁（读旧 + 合并本次写 + 写新块）。
                Err(FlashError::Program { .. }) => return self.relocate(logical, within, data),
                // 块坏了：标坏、解除映射，下一轮换一块重写本次数据。
                Err(FlashError::BadBlock { .. }) => {
                    self.phys[phys as usize].state = BlockState::Bad;
                    if self.l2p[logical as usize] == phys {
                        self.l2p[logical as usize] = UNMAPPED;
                    }
                    continue;
                }
                Err(_) => return Err(Error::Io),
            }
        }
        Err(Error::Io)
    }

    /// 整块搬迁：读出旧载荷，套上本次写，写进新块并提交。
    ///
    /// 这是 FTL 唯一的写放大来源：同一个逻辑块第二次写落在同一个物理块时，
    /// NOR 的 0→1 限制会拒绝，于是必须换块重写整块。
    fn relocate(&mut self, logical: u32, within: usize, data: &[u8]) -> Result<()> {
        let end = within
            .checked_add(data.len())
            .ok_or(Error::InvalidArgument)?;
        if end > self.logical_block_size {
            return Err(Error::InvalidArgument);
        }
        let old = self.l2p[logical as usize];
        let mut buf = alloc::vec![0xFFu8; self.logical_block_size];
        if old != UNMAPPED && self.phys[old as usize].state == BlockState::Data {
            for (i, chunk) in buf.chunks_mut(self.page_size).enumerate() {
                let off = self.page_size + i * self.page_size;
                // 旧块读失败（坏块/硬错误）时保留 0xFF：数据已不可救，
                // 但调用方仍会拿到本次写入的新数据。
                if self.raw.read(old, off, chunk).is_err() {
                    chunk.fill(0xFF);
                }
            }
        }
        buf[within..end].copy_from_slice(data);

        let np = self.alloc_block(logical)?;
        self.program_payload(np, &buf)?;
        self.l2p[logical as usize] = np;
        self.commit_header(np)?;
        if old != UNMAPPED {
            self.phys[old as usize] = Phys {
                state: BlockState::Dirty,
                ..self.phys[old as usize]
            };
        }
        self.stats.logical_write_bytes += data.len() as u64;
        self.stats.block_moves += 1;
        Ok(())
    }

    /// 提交：把块头写进块的页 0。
    fn commit_header(&mut self, block: u32) -> Result<()> {
        let p = self.phys[block as usize];
        let h = Header {
            logical: p.logical,
            alloc_seq: p.alloc_seq,
            erase_count: p.erase_count,
        };
        let buf = encode_header(h);
        self.raw.program(block, 0, &buf).map_err(|_| Error::Io)?;
        self.stats.physical_program_bytes += HEADER_SIZE as u64;
        Ok(())
    }

    /// 刷新 FTL 统计。
    fn refresh_stats(&mut self) {
        let mut free = 0u32;
        let mut bad = 0u32;
        let mut retired = 0u32;
        let mut min = u32::MAX;
        let mut max = 0u32;
        for p in self.phys.iter() {
            match p.state {
                BlockState::Free => {
                    free += 1;
                    min = min.min(p.erase_count);
                    max = max.max(p.erase_count);
                }
                BlockState::Bad => bad += 1,
                BlockState::Dirty => retired += 1,
                BlockState::Data => {}
            }
        }
        if free == 0 {
            min = 0;
        }
        self.stats.free_blocks = free;
        self.stats.bad_blocks = bad;
        self.stats.retired_blocks = retired;
        self.stats.erase_count_min = min;
        self.stats.erase_count_max = max;
        let logical = self.stats.logical_write_bytes;
        let physical = self.stats.physical_program_bytes;
        self.stats.write_amplification = if logical == 0 {
            0.0
        } else {
            physical as f64 / logical as f64
        };
    }

    /// 返回统计计数器快照：这些数值是上次完成的写/擦除/discard 操作后的状态。
    pub fn stats(&self) -> FtlStats {
        self.stats
    }

    /// 静态磨损均衡：把擦除计数最低的数据块搬到新块，平衡擦除分布。
    ///
    /// 触发条件（任一成立）：累计擦除次数是 `static_level_interval` 的整数倍，
    /// 或空闲块的擦除计数极差超过 `wear_gap_threshold`。两个参数为 0 时该项关闭。
    fn maybe_static_level(&mut self) -> Result<()> {
        // 递归护栏：搬迁可能触发擦除，擦除又会调用本函数。
        if self.leveling {
            return Ok(());
        }
        let by_interval = self.cfg.static_level_interval != 0
            && self.stats.erase_ops % self.cfg.static_level_interval == 0;
        let by_gap = self.cfg.wear_gap_threshold != 0
            && self
                .stats
                .erase_count_max
                .saturating_sub(self.stats.erase_count_min)
                > self.cfg.wear_gap_threshold;
        if !(by_interval || by_gap) {
            return Ok(());
        }
        // 不占用最后一个空闲块：本次擦除可能发生在回收路径上，调用方还要取块。
        if self.stats.free_blocks < 2 {
            return Ok(());
        }
        let cold = self
            .phys
            .iter()
            .enumerate()
            .filter(|(_, p)| p.state == BlockState::Data)
            .min_by_key(|(_, p)| p.erase_count)
            .map(|(i, _)| i as u32);
        let Some(cold) = cold else {
            return Ok(());
        };
        let logical = self.phys[cold as usize].logical;
        if logical == UNMAPPED {
            return Ok(());
        }
        // 搬迁整块：读出全部载荷再写进新块（relocate_all 会置旧块为 Dirty）。
        self.leveling = true;
        let result = self.relocate_all(logical);
        self.leveling = false;
        result
    }

    /// 搬迁整个逻辑块到新物理块（不改变内容）。
    fn relocate_all(&mut self, logical: u32) -> Result<()> {
        let old = self.l2p[logical as usize];
        if old == UNMAPPED {
            return Ok(());
        }
        let mut buf = alloc::vec![0xFFu8; self.logical_block_size];
        for (i, chunk) in buf.chunks_mut(self.page_size).enumerate() {
            self.raw
                .read(old, self.page_size + i * self.page_size, chunk)
                .map_err(|_| Error::Io)?;
        }
        let np = self.alloc_block(logical)?;
        self.program_payload(np, &buf)?;
        self.l2p[logical as usize] = np;
        self.commit_header(np)?;
        self.phys[old as usize] = Phys {
            state: BlockState::Dirty,
            ..self.phys[old as usize]
        };
        self.stats.block_moves += 1;
        Ok(())
    }
}

impl<R: RawFlash> BlockDevice for Ftl<R> {
    fn size(&self) -> u64 {
        self.logical_size()
    }

    fn discard(&mut self, offset: u64, len: u64) -> Result<()> {
        let end = offset.checked_add(len).ok_or(Error::InvalidArgument)?;
        if end > self.logical_size() {
            return Err(Error::InvalidArgument);
        }
        let lsize = self.logical_block_size as u64;
        // 仅回收被完整覆盖的逻辑块；部分覆盖不动作、不报错。
        let first = offset.div_ceil(lsize);
        let last = end / lsize; // 开区间：不完整的尾块不计
        for lb in first..last {
            let li = lb as usize;
            let phys = self.l2p[li];
            if phys == UNMAPPED {
                continue;
            }
            self.l2p[li] = UNMAPPED;
            if self.phys[phys as usize].state == BlockState::Data {
                // 设计 §3.5：退役即擦除，擦除计数 +1 后回到空闲池。
                self.erase_block(phys)?;
            }
        }
        self.refresh_stats();
        Ok(())
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        let end = offset
            .checked_add(buf.len() as u64)
            .ok_or(Error::InvalidArgument)?;
        if end > self.logical_size() {
            return Err(Error::InvalidArgument);
        }
        let start = offset as usize;
        let lsize = self.logical_block_size;
        let mut done = 0usize;
        while done < buf.len() {
            let absolute = start + done;
            let logical = (absolute / lsize) as u32;
            let within = absolute % lsize;
            let take = (lsize - within).min(buf.len() - done);
            let phys = self.l2p[logical as usize];
            let dst = &mut buf[done..done + take];
            if phys == UNMAPPED || self.phys[phys as usize].state != BlockState::Data {
                dst.fill(0xFF);
            } else {
                self.raw
                    .read(phys, self.page_size + within, dst)
                    .map_err(|_| Error::Io)?;
            }
            done += take;
        }
        Ok(())
    }

    fn write_at(&mut self, offset: u64, buf: &[u8]) -> Result<()> {
        let end = offset
            .checked_add(buf.len() as u64)
            .ok_or(Error::InvalidArgument)?;
        if end > self.logical_size() {
            return Err(Error::InvalidArgument);
        }
        let start = offset as usize;
        let lsize = self.logical_block_size;
        let mut done = 0usize;
        while done < buf.len() {
            let absolute = start + done;
            let logical = (absolute / lsize) as u32;
            let within = absolute % lsize;
            // 切到"逻辑块尾"与"物理页尾"两者中更近的一个：`RawFlash::program`
            // 不允许跨页，块内页偏移 = `page_size + within`。
            let to_page_end = self.page_size - (within % self.page_size);
            let take = to_page_end.min(lsize - within).min(buf.len() - done);
            self.write_chunk(logical, within, &buf[done..done + take])?;
            done += take;
        }
        self.refresh_stats();
        Ok(())
    }

    fn sync(&mut self) -> Result<()> {
        self.raw.sync().map_err(|_| Error::Io)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::SimFlash;
    use embedded_database_core::BlockDevice;

    const PAGE: usize = 256;
    const BLOCK: usize = 4096;
    const LOGICAL_BLOCK: usize = BLOCK - PAGE; // 3840
                                               // 1024 个逻辑块 = 3840 KiB 逻辑容量（20% OP：1280 物理块）。
    const NBLOCKS: u32 = 1280;

    fn ftl() -> Ftl<SimFlash> {
        let raw = SimFlash::new(NBLOCKS, PAGE, BLOCK);
        Ftl::new(raw, FtlConfig::default()).unwrap()
    }

    #[test]
    fn 逻辑容量等于_物理块数乘_4_除_5_再乘_3840() {
        let f = ftl();
        assert_eq!(
            f.logical_size(),
            (NBLOCKS as u64 * 4 / 5) * LOGICAL_BLOCK as u64
        );
    }

    #[test]
    fn 单块写后读回一致() {
        let mut f = ftl();
        let data = alloc::vec![0xA5u8; LOGICAL_BLOCK];
        f.write_at(0, &data).unwrap();
        let mut got = alloc::vec![0u8; LOGICAL_BLOCK];
        f.read_at(0, &mut got).unwrap();
        assert_eq!(got, data);
    }

    #[test]
    fn 同一逻辑块分两次写_两段都能读回() {
        let mut f = ftl();
        let a = alloc::vec![0x11u8; 256];
        let b = alloc::vec![0x22u8; 256];
        f.write_at(0, &a).unwrap();
        f.write_at(256, &b).unwrap();
        let mut got = alloc::vec![0u8; 512];
        f.read_at(0, &mut got).unwrap();
        assert_eq!(&got[..256], &a[..]);
        assert_eq!(&got[256..], &b[..]);
    }

    #[test]
    fn 覆写同一逻辑块后读到新数据() {
        let mut f = ftl();
        f.write_at(0, &alloc::vec![0x11u8; 128]).unwrap();
        f.write_at(0, &alloc::vec![0x33u8; 128]).unwrap();
        let mut got = alloc::vec![0u8; 128];
        f.read_at(0, &mut got).unwrap();
        assert_eq!(got, alloc::vec![0x33u8; 128]);
    }

    #[test]
    fn 卸载重挂后数据仍在() {
        let mut f = ftl();
        f.write_at(LOGICAL_BLOCK as u64, &alloc::vec![0x77u8; 512])
            .unwrap();
        f.sync().unwrap();
        let raw = f.into_raw();
        let f2 = Ftl::new(raw, FtlConfig::default()).unwrap();
        let mut got = alloc::vec![0u8; 512];
        f2.read_at(LOGICAL_BLOCK as u64, &mut got).unwrap();
        assert_eq!(got, alloc::vec![0x77u8; 512]);
    }

    #[test]
    #[allow(non_snake_case)]
    fn 越界读写返回_InvalidArgument() {
        let mut f = ftl();
        let size = f.logical_size();
        assert_eq!(f.write_at(size, &[0u8; 1]), Err(Error::InvalidArgument));
        assert_eq!(f.read_at(size, &mut [0u8; 1]), Err(Error::InvalidArgument));
    }

    #[test]
    fn 未映射逻辑块读出全_ff() {
        let f = ftl();
        let mut got = alloc::vec![0u8; 16];
        f.read_at(0, &mut got).unwrap();
        assert_eq!(got, alloc::vec![0xFFu8; 16]);
    }

    #[test]
    fn discard_后整块变为可回收且数据读出全_ff() {
        let mut f = ftl();
        f.write_at(0, &alloc::vec![0x55u8; LOGICAL_BLOCK]).unwrap();
        f.discard(0, LOGICAL_BLOCK as u64).unwrap();
        let mut got = alloc::vec![0u8; LOGICAL_BLOCK];
        f.read_at(0, &mut got).unwrap();
        assert_eq!(got, alloc::vec![0xFFu8; LOGICAL_BLOCK]);
    }

    #[test]
    fn discard_部分覆盖不动数据() {
        let mut f = ftl();
        f.write_at(0, &alloc::vec![0x55u8; LOGICAL_BLOCK]).unwrap();
        f.discard(0, 100).unwrap();
        let mut got = alloc::vec![0u8; 16];
        f.read_at(0, &mut got).unwrap();
        assert_eq!(got, alloc::vec![0x55u8; 16]);
    }

    #[test]
    fn 空闲块耗尽时回收脏块而不是报容量不足() {
        // 5 个物理块、OP 20% ⇒ 逻辑 4 块。反复覆写制造脏块。
        let raw = SimFlash::new(5, PAGE, BLOCK);
        let mut f = Ftl::new(raw, FtlConfig::default()).unwrap();
        assert_eq!(f.logical_size(), 4 * LOGICAL_BLOCK as u64);
        for round in 0..20u8 {
            for lb in 0..4u64 {
                f.write_at(lb * LOGICAL_BLOCK as u64, &alloc::vec![round; 128])
                    .unwrap();
            }
        }
        let mut got = alloc::vec![0u8; 128];
        f.read_at(0, &mut got).unwrap();
        assert_eq!(got, alloc::vec![19u8; 128]);
        assert!(f.stats().erase_ops > 0);
    }

    #[test]
    fn 统计不变量_空闲加数据加脏加坏等于物理块数() {
        let mut f = ftl();
        for lb in 0..16u64 {
            f.write_at(lb * LOGICAL_BLOCK as u64, &alloc::vec![1u8; 64])
                .unwrap();
        }
        let s = f.stats();
        assert_eq!(
            s.free_blocks + s.retired_blocks + s.bad_blocks + 16,
            NBLOCKS
        );
    }

    #[test]
    fn 坏块被跳过且数据仍可读() {
        let raw = SimFlash::new(16, PAGE, BLOCK);
        let mut raw = raw;
        // 先把 0 号块用一次，制造一个"已经用过"的块，再标坏。
        raw.program(0, 0, &[0x00]).unwrap();
        raw.mark_bad(0);
        let mut f = Ftl::new(raw, FtlConfig::default()).unwrap();
        f.write_at(0, &alloc::vec![0x42u8; 3840]).unwrap();
        let mut got = alloc::vec![0u8; 3840];
        f.read_at(0, &mut got).unwrap();
        assert_eq!(got, alloc::vec![0x42u8; 3840]);
        assert_eq!(f.stats().bad_blocks, 1);
    }

    #[test]
    fn 覆写场景下写放大大于一() {
        let mut f = ftl();
        for i in 0..500u64 {
            f.write_at(0, &alloc::vec![(i & 0xFF) as u8; 64]).unwrap();
        }
        // 每次覆写落在同一物理块上、被 NOR 0→1 拒绝，于是触发整块搬迁。
        assert!(f.stats().write_amplification > 1.0);
        assert!(f.stats().block_moves > 0);
    }

    #[test]
    fn core_db_建立在_ftl_之上可写满多段并重开() {
        use embedded_database_core::{Config, Db};

        const SEG: usize = 3840 * 1024; // 3_932_160，逻辑块(3840)的整数倍
        let max_segments = 4u64;
        let logical = max_segments * SEG as u64;
        let logical_blocks = logical / 3840;
        // FTL 默认 OP 20% ⇒ 物理块数 = 逻辑块数 × 5/4。
        let phys = (logical_blocks * 5 / 4) as u32;

        let raw = SimFlash::new(phys, PAGE, BLOCK);
        let mut ftl = Ftl::new(raw, FtlConfig::default()).unwrap();
        assert_eq!(ftl.logical_size(), logical);

        let cfg = Config {
            segment_size: SEG,
            max_segments: max_segments as usize,
            // 计划勘误：默认 max_pages=8 时单段只装 8 页，60 个不同键
            // （60 页）在 4 槽环内会被环回覆写淘汰最旧段。15 是能让
            // 60 页恰好落满 4 个存活段的最小值（60 = 4 × 15），环内
            // 槽位写满且不触发覆写；段内预留区（max_pages ×
            // slots_per_page × page_size）+ 追加区仍远小于 SEG。
            max_pages: 15,
            ..Config::default()
        };

        {
            let mut db = Db::open(&mut ftl, cfg.clone()).unwrap();
            for i in 0..60u64 {
                db.put(alloc::format!("k{i}").as_bytes(), b"payload")
                    .unwrap();
                db.flush().unwrap();
            }
        }

        // 重开：段环 + 合并后所有键仍在。
        let db = Db::open(&mut ftl, cfg).unwrap();
        for i in 0..60u64 {
            assert_eq!(
                db.get(alloc::format!("k{i}").as_bytes()).unwrap(),
                Some(b"payload".to_vec()),
                "键 k{i} 丢失"
            );
        }
    }
}
