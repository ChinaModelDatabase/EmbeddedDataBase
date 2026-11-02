//! 段格式定义与零拷贝编解码。
//!
//! # 布局不变量（改动本文件前必读）
//!
//! 1. **零填充**。`zerocopy` 的 `IntoBytes` 派生要求结构体**没有任何填充字节**
//!    （含尾部填充）。这不是洁癖：CRC 覆盖的是字节区间，编译器插入的任何隐式空洞
//!    都是「不参与校验的磁盘字节」，也意味着 `size_of` 会被 ABI 对齐悄悄改变。
//!    因此每个结构体都被排成**字段宽度之和 == 尺寸常量**，且所有 `u64` 落在
//!    8 字节对齐的偏移上。编译器强制这条——写错即报 `has N byte(s) of padding`。
//!    推论：只要首字段是 `u64`，零填充尺寸就**必须是 8 的倍数**（align 被抬到 8，
//!    尺寸必须是它的整数倍）。这正是 `PAGE_HEADER_SIZE` 取 24 而非 28 的原因。
//! 2. **CRC 覆盖除自身外的全部字节**。四个结构体的校验和字段（`header_crc` /
//!    `page_crc` / `commit_crc` / `run_crc`）都固定占据各自末尾 4 字节，CRC 取
//!    其之前的前缀，使翻转任意一个字节都能被检出；四个 `逐位翻转全部被检出`
//!    测试逐字节锁死这个不变量。
//!    ⚠️ 反面教材：CRC 范围写成 `[..N]`（含自身）会形成自引用——`new()` 时算出的
//!    值与 `decode()` 时读到的值必然不等，写入后立即校验失败。
//! 3. **校验和字段一旦被改动就要重算**。`_pad` / `flags` 这类「看起来只是元数据」
//!    的字节同样落在 CRC 覆盖范围内：`PageRun::freeze()` 改了 `flags` 就必须
//!    重算 `run_crc`，否则一条刚写下的记录会被自己的 `decode()` 拒掉。
//! 4. **`Unalign` 不是可选的**。这些结构体对齐为 8，但从设备读出的 `&[u8]`
//!    起点通常不对齐；`FromBytes::read_from_bytes` 会因对齐不满足直接返回 `Err`，
//!    把合法数据误判为损坏。`Unalign::<T>` 把对齐要求降到 1，是这里唯一正确的读法。
//! 5. **`decode` 的错误分两档，第一档四个解码器必须完全一致**：
//!    - **缓冲区不足 / 读不出结构体** → `Error::InvalidArgument`。切片长度不对是
//!      **调用方的编程错误**，与磁盘上的数据无关；报成损坏会让下游（Task 7 崩溃恢复）
//!      分不清「我的代码有 bug」（应立刻暴露并修代码）与「磁盘坏了」（应停下上报）。
//!      四个 `decode` 在这一档**没有**任何差异。
//!    - **字段校验失败（魔数 / CRC / 非法取值）** → 才是真正的数据损坏，此时报
//!      `Corruption { segment_seq }` 或 `CorruptedPage { page_id }`。**这一档的差异是
//!      有意为之**，不构成不一致：解码器手里只有它实际持有的定位信息。段头与提交记录
//!      在段上下文内被解析，拿得到段序列号，报 `Corruption { segment_seq }`；页头与槽表
//!      记录可能在冷启动时脱离段上下文被单独解析，段号无从取得，故报页号
//!      `CorruptedPage { page_id }`（见 `leading_page_id`）。绝不用 0 或任何哨兵值顶替——
//!      0 是合法段号/页号，哨兵会让「真的是 0 号」与「不知道」无法区分。
//!
//! 字段顺序即磁盘布局：调整顺序等于修改磁盘格式，Task 7/8 会依赖它。

// 磁盘格式的字节序契约
//
// 四个格式结构体的 encode/decode 走 zerocopy 的 IntoBytes/Unalign，
// 而 `IntoBytes::as_bytes` 返回的是**内存字节**，即主机字节序。
// 因此本格式在小端目标上是稳定的 LE 布局，但在大端目标上会变成 BE 布局，
// 写出的段将无法被其他目标读取。
//
// 模块内的 `leading_page_id` 用的是显式 LE（u64::from_le_bytes），
// 在大端目标上会与 IntoBytes 的主机字节序不一致——同一份字节解出不同的 page_id。
//
// M1 的三个目标（loongarch64-*、aarch64-*、x86_64-*）全为小端，故此处直接
// 拦截大端构建：与其静默产出跨架构不兼容的数据，不如编译期失败。
// 若将来确需支持大端，正确做法是把四个结构的 encode/decode 改写为显式的
// to_le_bytes/from_le_bytes，并把下面的 compile_error 一并移除。
#[cfg(target_endian = "big")]
compile_error!(
    "embedded-database 的磁盘格式目前依赖主机小端字节序（zerocopy IntoBytes 为内存转录）。\
     在大端目标上,encode 会写出 BE 布局,与模块内显式 LE 的 leading_page_id 也不一致。\
     需先将四个格式结构的 encode/decode 改写为显式小端序列化,并移除本 compile_error。"
);

use crate::crc::Crc32c;
use crate::Error;
use zerocopy::{FromBytes, Immutable, IntoBytes, Unalign};

pub const SEGMENT_MAGIC: [u8; 8] = *b"WHSEG\0\0\0";
pub const COMMIT_MAGIC: [u8; 8] = *b"WHCMT\0\0\0";

pub const SEGMENT_HEADER_SIZE: usize = 56;
pub const PAGE_HEADER_SIZE: usize = 24;
pub const COMMIT_RECORD_SIZE: usize = 40;
pub const PAGE_RUN_ENTRY_SIZE: usize = 24;

/// PageRun.flags：页的 k 个槽已用尽，后续更新必须走溢出段。
pub const SLOT_FLAG_FROZEN: u16 = 0x0001;

/// `PageHeader` 与 `PageRun` 的 `page_id` 都固定落在 `[0, 8)`，小端。
///
/// 这两个结构体是在**解析单个页头 / 单条槽表记录**时解码的，调用点并不持有段上下文，
/// 因此无法报出真正的段序列号；过去它们拿 `segment_seq: 0` 顶替，但 0 是**合法**段号，
/// 于是「段 0 损坏」和「来源不明」在下游（Task 7 崩溃恢复）无法区分。改报
/// `CorruptedPage { page_id }` 后，页号是唯一可靠的定位信息。
///
/// 注意页号不需要 CRC 校验通过才可信：它是页头/槽表记录的**首个**字段，位于任何
/// 结构性前置校验（魔数、CRC）之前，所以校验失败时这 8 字节通常仍原样可读。
///
/// 长度不足 8 字节时**不返回页号**，而是按同一口径报 `InvalidArgument`——「凑不满 8 字节」
/// 与「凑不满 24 字节」是同一种调用方错误，没有理由为前者捏造一个填 0 的哨兵页号。
fn leading_page_id(buf: &[u8]) -> Result<u64, Error> {
    // `first_chunk::<8>()` 是唯一的下界检查，返回的定长视图长度必然为 8，因此不存在越界。
    let head: &[u8; 8] = buf.first_chunk::<8>().ok_or(Error::InvalidArgument)?;
    Ok(u64::from_le_bytes(*head))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, FromBytes, IntoBytes, Immutable)]
#[repr(C)]
pub struct SegmentHeader {
    pub magic: [u8; 8],
    pub format_major: u32,
    pub format_minor: u32,
    pub segment_seq: u64,
    pub prev_segment: u64,
    pub prev_crc: u32,
    pub page_size: u32,
    /// 槽区中 PageRun 表的起始偏移，相对段起点。
    pub slot_table_off: u64,
    pub slot_count: u16,
    pub _pad: u16,
    /// 覆盖 [0, 52)，即除 header_crc 自身以外的全部字节。
    pub header_crc: u32,
}

impl SegmentHeader {
    pub fn new(
        segment_seq: u64,
        prev_segment: u64,
        prev_crc: u32,
        page_size: u32,
        slot_count: u16,
        slot_table_off: u64,
    ) -> Self {
        let mut h = SegmentHeader {
            magic: SEGMENT_MAGIC,
            format_major: 1,
            format_minor: 0,
            segment_seq,
            prev_segment,
            prev_crc,
            page_size,
            slot_table_off,
            slot_count,
            _pad: 0,
            header_crc: 0,
        };
        h.header_crc = h.compute_crc();
        h
    }

    fn compute_crc(&self) -> u32 {
        // header_crc 自身固定占据 [52, 56)，故只覆盖它之前的 [0, 52)——
        // 恰好是除自身以外的全部字节，不留任何未校验的尾巴。
        // ⚠️ 不能写成 [..SEGMENT_HEADER_SIZE]（或任何含末尾 4 字节的范围）：
        // 那会把 header_crc 自身算进去形成自引用，new() 时算出的值与 decode
        // 时读到的值必然不等。
        let bytes = <Self as IntoBytes>::as_bytes(self);
        let mut c = Crc32c::new();
        c.update(&bytes[..52]);
        c.finish()
    }

    pub fn encode(&self) -> [u8; SEGMENT_HEADER_SIZE] {
        let mut out = [0u8; SEGMENT_HEADER_SIZE];
        out.copy_from_slice(<Self as IntoBytes>::as_bytes(self));
        out
    }

    pub fn decode(buf: &[u8]) -> Result<SegmentHeader, Error> {
        let bytes = buf
            .get(..SEGMENT_HEADER_SIZE)
            .ok_or(Error::InvalidArgument)?;
        let h = Unalign::<Self>::read_from_bytes(bytes)
            .map(|u| u.get())
            .map_err(|_| Error::InvalidArgument)?;
        if h.magic != SEGMENT_MAGIC || h.compute_crc() != h.header_crc {
            return Err(Error::Corruption {
                segment_seq: h.segment_seq,
            });
        }
        if h.page_size == 0 || h.slot_count == 0 {
            return Err(Error::Corruption {
                segment_seq: h.segment_seq,
            });
        }
        Ok(h)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, FromBytes, IntoBytes, Immutable)]
#[repr(C)]
pub struct PageHeader {
    pub page_id: u64,
    pub version: u32,
    pub payload_len: u32,
    /// 预留字节，恒为 0。**它不能删**：删掉后结构体只剩 20 字节，而首字段 `u64`
    /// 把 align 抬到 8，编译器随即补 4 字节尾部填充，`IntoBytes` 派生直接编译失败。
    /// 这也是 `PAGE_HEADER_SIZE` 只能是 24（8 的倍数）而不能是 28 的原因。
    /// `page_crc` 住的正是 `_pad` 之后那 4 字节预留空间——若将来决定不要页级
    /// 校验，要删的是 `page_crc`，而不是把 `_pad` 加宽。
    pub _pad: u32,
    /// 覆盖 [0, 20)，即除 page_crc 自身以外的全部字节。
    pub page_crc: u32,
}

impl PageHeader {
    pub fn new(page_id: u64, version: u32, payload_len: u32) -> Self {
        let mut h = PageHeader {
            page_id,
            version,
            payload_len,
            _pad: 0,
            page_crc: 0,
        };
        h.page_crc = h.compute_crc();
        h
    }

    fn compute_crc(&self) -> u32 {
        // page_crc 自身固定占据 [20, 24)，故只覆盖它之前的 [0, 20)——
        // 恰好是除自身以外的全部字节，连 _pad 也在校验范围内。
        let bytes = <Self as IntoBytes>::as_bytes(self);
        let mut c = Crc32c::new();
        c.update(&bytes[..20]);
        c.finish()
    }

    pub fn encode(&self) -> [u8; PAGE_HEADER_SIZE] {
        let mut out = [0u8; PAGE_HEADER_SIZE];
        out.copy_from_slice(<Self as IntoBytes>::as_bytes(self));
        out
    }

    pub fn decode(buf: &[u8]) -> Result<PageHeader, Error> {
        // 缓冲区不足 = 调用方传错了切片长度，是**编程错误**而非数据损坏，
        // 故报 InvalidArgument（与 SegmentHeader::decode / CommitRecord::decode 同口径）。
        let bytes = buf.get(..PAGE_HEADER_SIZE).ok_or(Error::InvalidArgument)?;
        // 页头没有段上下文，只能报页号（见 `leading_page_id`）：长度已确认，
        // 取前 8 字节得到真实页号，好让下面的校验失败分支带上它。
        let page_id = leading_page_id(bytes)?;
        let h = <Unalign<Self> as FromBytes>::read_from_bytes(bytes)
            .map(|u| u.get())
            .map_err(|_| Error::InvalidArgument)?;
        // 只有真正读到结构体之后才有资格谈「损坏」，此时页号已可信。
        if h.compute_crc() != h.page_crc {
            return Err(Error::CorruptedPage { page_id });
        }
        Ok(h)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, FromBytes, IntoBytes, Immutable)]
#[repr(C)]
pub struct CommitRecord {
    pub commit_magic: [u8; 8],
    pub segment_seq: u64,
    pub timestamp: u64,
    pub prev_crc: u32,
    pub page_crc_fold: u32,
    pub _pad: u32,
    /// 覆盖 [0, 36)，即除 commit_crc 自身以外的全部字节。
    pub commit_crc: u32,
}

impl CommitRecord {
    pub fn new(segment_seq: u64, prev_crc: u32, page_crc_fold: u32, timestamp: u64) -> Self {
        let mut r = CommitRecord {
            commit_magic: COMMIT_MAGIC,
            segment_seq,
            timestamp,
            prev_crc,
            page_crc_fold,
            _pad: 0,
            commit_crc: 0,
        };
        r.commit_crc = r.compute_crc();
        r
    }

    fn compute_crc(&self) -> u32 {
        let bytes = <Self as IntoBytes>::as_bytes(self);
        let mut c = Crc32c::new();
        c.update(&bytes[..36]);
        c.finish()
    }

    pub fn encode(&self) -> [u8; COMMIT_RECORD_SIZE] {
        let mut out = [0u8; COMMIT_RECORD_SIZE];
        out.copy_from_slice(<Self as IntoBytes>::as_bytes(self));
        out
    }

    pub fn decode(buf: &[u8]) -> Result<CommitRecord, Error> {
        let bytes = buf
            .get(..COMMIT_RECORD_SIZE)
            .ok_or(Error::InvalidArgument)?;
        let r = Unalign::<Self>::read_from_bytes(bytes)
            .map(|u| u.get())
            .map_err(|_| Error::InvalidArgument)?;
        if r.commit_magic != COMMIT_MAGIC || r.compute_crc() != r.commit_crc {
            return Err(Error::Corruption {
                segment_seq: r.segment_seq,
            });
        }
        Ok(r)
    }
}

/// 段内一个逻辑页的槽位组描述。
///
/// 注意这里**没有** `_pad`：8 + 8 + 2 + 2 + 4 = 24 正好等于尺寸常量，
/// 再加一个 `u32` 会到 32——虽然仍是 8 的倍数，但会多出本可用来放校验和的空间，
/// 而 `run_crc` 正是当初把它从 16 加宽到 24 的理由。
#[derive(Debug, Clone, Copy, PartialEq, Eq, FromBytes, IntoBytes, Immutable)]
#[repr(C)]
pub struct PageRun {
    pub page_id: u64,
    /// 槽区中该页首个槽的绝对偏移。宽度是 `u64` 而非 `u32`：段大小按 u64 计，
    /// 用 u32 存会在 4 GiB 以上的段上截断。
    pub run_offset: u64,
    pub next_slot: u16,
    pub flags: u16,
    /// 覆盖 [0, 20)，即除 run_crc 自身以外的全部字节（含 `next_slot` 与 `flags`）。
    pub run_crc: u32,
}

impl PageRun {
    pub fn new(page_id: u64, run_offset: u64, next_slot: u16, flags: u16) -> Self {
        let mut r = PageRun {
            page_id,
            run_offset,
            next_slot,
            flags,
            run_crc: 0,
        };
        r.run_crc = r.compute_crc();
        r
    }

    fn compute_crc(&self) -> u32 {
        // run_crc 自身固定占据 [20, 24)，故只覆盖它之前的 [0, 20)。
        let bytes = <Self as IntoBytes>::as_bytes(self);
        let mut c = Crc32c::new();
        c.update(&bytes[..20]);
        c.finish()
    }

    pub fn encode(&self) -> [u8; PAGE_RUN_ENTRY_SIZE] {
        let mut out = [0u8; PAGE_RUN_ENTRY_SIZE];
        out.copy_from_slice(<Self as IntoBytes>::as_bytes(self));
        out
    }

    pub fn decode(buf: &[u8]) -> Result<PageRun, Error> {
        // 同 PageHeader::decode：长度不对是调用方的错，报 InvalidArgument。
        let bytes = buf
            .get(..PAGE_RUN_ENTRY_SIZE)
            .ok_or(Error::InvalidArgument)?;
        // 槽表条目同样没有段上下文，报页号而非段序号（见 `leading_page_id`）。
        let page_id = leading_page_id(bytes)?;
        let r = <Unalign<Self> as FromBytes>::read_from_bytes(bytes)
            .map(|u| u.get())
            .map_err(|_| Error::InvalidArgument)?;
        if r.compute_crc() != r.run_crc {
            return Err(Error::CorruptedPage { page_id });
        }
        Ok(r)
    }

    pub fn is_frozen(&self) -> bool {
        self.flags & SLOT_FLAG_FROZEN != 0
    }

    pub fn freeze(&mut self) {
        self.flags |= SLOT_FLAG_FROZEN;
        // `flags` 落在 run_crc 的覆盖范围内，改了它就必须重算，
        // 否则这条刚被冻结的记录会被自己的 decode() 当成损坏数据拒掉。
        self.run_crc = self.compute_crc();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::mem::{align_of, size_of};

    #[test]
    fn 结构体大小符合磁盘格式() {
        assert_eq!(size_of::<SegmentHeader>(), SEGMENT_HEADER_SIZE);
        assert_eq!(size_of::<PageHeader>(), PAGE_HEADER_SIZE);
        assert_eq!(size_of::<CommitRecord>(), COMMIT_RECORD_SIZE);
        assert_eq!(size_of::<PageRun>(), PAGE_RUN_ENTRY_SIZE);
    }

    #[test]
    fn 结构体八字节对齐() {
        assert_eq!(align_of::<SegmentHeader>() % 8, 0);
        assert_eq!(align_of::<PageHeader>() % 8, 0);
        assert_eq!(align_of::<CommitRecord>() % 8, 0);
        assert_eq!(align_of::<PageRun>() % 8, 0);
    }

    #[test]
    fn 段头往返一致() {
        let h = SegmentHeader::new(7, 6, 0xDEAD_BEEF, 4096, 4, 4096);
        assert_eq!(SegmentHeader::decode(&h.encode()).unwrap(), h);
    }

    #[test]
    fn 段头_逐位翻转全部被检出() {
        let h = SegmentHeader::new(9, 8, 0x1234_5678, 4096, 4, 4096);
        let base = h.encode();
        for i in 0..SEGMENT_HEADER_SIZE {
            let mut b = base;
            b[i] ^= 0x01;
            assert!(SegmentHeader::decode(&b).is_err(), "翻转字节 {i} 未被检出");
        }
    }

    #[test]
    fn 提交记录_逐位翻转全部被检出() {
        let r = CommitRecord::new(42, 0x0102_0304, 0xAABB_CCDD, 1_700_000_000);
        let base = r.encode();
        for i in 0..COMMIT_RECORD_SIZE {
            let mut b = base;
            b[i] ^= 0x01;
            assert!(CommitRecord::decode(&b).is_err(), "翻转字节 {i} 未被检出");
        }
    }

    #[test]
    fn 提交记录往返一致() {
        let r = CommitRecord::new(42, 0x0102_0304, 0xAABB_CCDD, 1_700_000_000);
        assert_eq!(CommitRecord::decode(&r.encode()).unwrap(), r);
    }

    #[test]
    fn 提交记录_篡改被拒() {
        let r = CommitRecord::new(42, 0, 0xFFFF_FFFF, 0);
        let mut bytes = r.encode();
        bytes[28] ^= 0xFF;
        assert!(CommitRecord::decode(&bytes).is_err());
    }

    #[test]
    fn 提交记录_错魔数被拒() {
        let r = CommitRecord::new(42, 0, 0, 0);
        let mut bytes = r.encode();
        bytes[0] = b'Z';
        assert!(CommitRecord::decode(&bytes).is_err());
    }

    #[test]
    fn 段头_缓冲区不足被拒() {
        let h = SegmentHeader::new(1, 0, 0, 4096, 4, 4096);
        let bytes = h.encode();
        assert_eq!(
            SegmentHeader::decode(&bytes[..SEGMENT_HEADER_SIZE - 1]),
            Err(Error::InvalidArgument)
        );
    }

    #[test]
    fn 段头_非法页大小被拒() {
        let h = SegmentHeader::new(1, 0, 0, 0, 4, 4096);
        assert!(SegmentHeader::decode(&h.encode()).is_err());
    }

    #[test]
    fn 页头往返一致() {
        let p = PageHeader::new(12345, 3, 100);
        assert_eq!(PageHeader::decode(&p.encode()).unwrap(), p);
    }

    #[test]
    fn 页头_逐位翻转全部被检出() {
        let p = PageHeader::new(0x0123_4567_89AB_CDEF, 0x1122_3344, 100);
        let base = p.encode();
        let mut checked = 0usize;
        for i in 0..PAGE_HEADER_SIZE {
            let mut b = base;
            b[i] ^= 0x01;
            assert!(PageHeader::decode(&b).is_err(), "翻转字节 {i} 未被检出");
            checked += 1;
        }
        assert_eq!(checked, 24, "必须覆盖 PageHeader 全部 24 字节");
    }

    #[test]
    fn 页槽组往返与冻结() {
        let mut r = PageRun::new(7, 8192, 2, 0);
        assert!(!r.is_frozen());
        r.freeze();
        assert!(r.is_frozen());
        // flags 落在 run_crc 的覆盖范围内，所以 freeze 必须重算校验和，
        // 否则写回磁盘后 decode 会拒掉一条自己刚写的记录。
        let decoded = PageRun::decode(&r.encode()).unwrap();
        assert_eq!(decoded, r);
        assert!(decoded.is_frozen());
    }

    #[test]
    fn 页槽组_逐位翻转全部被检出() {
        let r = PageRun::new(
            0x0123_4567_89AB_CDEF,
            0x1122_3344_5566_7788,
            0x0302,
            SLOT_FLAG_FROZEN,
        );
        let base = r.encode();
        let mut checked = 0usize;
        for i in 0..PAGE_RUN_ENTRY_SIZE {
            let mut b = base;
            b[i] ^= 0x01;
            assert!(PageRun::decode(&b).is_err(), "翻转字节 {i} 未被检出");
            checked += 1;
        }
        assert_eq!(checked, 24, "必须覆盖 PageRun 全部 24 字节");
    }

    #[test]
    fn 页头_校验失败上报页号而非段0() {
        // page_id 固定在 [0, 8)，version 在 [8, 12)。翻转 version 的一个字节：
        // CRC 必然失配，而 page_id 完好无损——解码器此时手上**已经有**真实页号，
        // 若仍报「段 0 损坏」，就等于把它和「来源不明」混为一谈。
        // 返回的页号必须等于原始 page_id，而不是 0。
        let p = PageHeader::new(0x0123_4567_89AB_CDEF, 0x1122_3344, 100);
        let mut b = p.encode();
        b[8] ^= 0x01;
        assert_eq!(
            PageHeader::decode(&b),
            Err(Error::CorruptedPage {
                page_id: 0x0123_4567_89AB_CDEF
            })
        );
    }

    #[test]
    fn 页头_页号字节被篡改时上报篡改后的页号() {
        // 证明上报的页号确实**读自字节**而非硬编码：翻转 page_id 内的一个字节后，
        // 报出的必须是从字节解出的小端值（既非原始页号，也非 0）。
        let p = PageHeader::new(0x0123_4567_89AB_CDEF, 0x1122_3344, 100);
        let mut b = p.encode();
        b[0] ^= 0x01;
        assert_eq!(
            PageHeader::decode(&b),
            Err(Error::CorruptedPage {
                page_id: 0x0123_4567_89AB_CDEF ^ 0x01
            })
        );
    }

    #[test]
    fn 页槽组_校验失败上报页号而非段0() {
        // 与 PageHeader 同理：槽表条目同样没有段上下文。
        let r = PageRun::new(0x0123_4567_89AB_CDEF, 0x1122_3344_5566_7788, 3, 0);
        let mut b = r.encode();
        b[8] ^= 0x01;
        assert_eq!(
            PageRun::decode(&b),
            Err(Error::CorruptedPage {
                page_id: 0x0123_4567_89AB_CDEF
            })
        );
    }

    #[test]
    fn 页头_缓冲区不足报调用方错误而非损坏() {
        // 缓冲区长度不对是**调用方编程错误**（切片传短了），不是磁盘上的数据损坏。
        // 报成 CorruptedPage 会让下游崩溃恢复分不清「我的代码有 bug」和「磁盘坏了」：
        // 前者要立刻暴露并修代码，后者才停下上报。四个 decode 在这一档必须一致。
        let p = PageHeader::new(7, 0, 0);
        let bytes = p.encode();
        assert_eq!(
            PageHeader::decode(&bytes[..PAGE_HEADER_SIZE - 1]),
            Err(Error::InvalidArgument)
        );
        // 连 8 字节都凑不齐：页号无从谈起，但那不再是理由——
        // 仍然是长度不合法，仍然是调用方的错。
        assert_eq!(PageHeader::decode(&bytes[..3]), Err(Error::InvalidArgument));
        assert_eq!(PageHeader::decode(&[]), Err(Error::InvalidArgument));
    }

    #[test]
    fn 页槽组_缓冲区不足报调用方错误而非损坏() {
        // 与 PageHeader::decode 同一语义：槽表条目同样把「缓冲区不足」判为调用方错误。
        let r = PageRun::new(7, 8192, 2, 0);
        let bytes = r.encode();
        assert_eq!(
            PageRun::decode(&bytes[..PAGE_RUN_ENTRY_SIZE - 1]),
            Err(Error::InvalidArgument)
        );
        assert_eq!(PageRun::decode(&bytes[..3]), Err(Error::InvalidArgument));
        assert_eq!(PageRun::decode(&[]), Err(Error::InvalidArgument));
    }

    #[test]
    fn 四个解码器_缓冲区不足口径完全一致() {
        // 本文件的核心不变量（见模块文档第 5 条）：四个 decode 在「缓冲区不足」这一档
        // 必须报**同一个**错误 `InvalidArgument`。此前 PageHeader / PageRun 报
        // `CorruptedPage`，等于告诉下游「磁盘坏了」，而实际只是调用方传短了切片。
        //
        // 整段扫描 0..len 而不是只测空切片与 N-1：只测两端会在中间留下「只有 1 字节
        // 的洞」，而退化分支正是从这种缝里长出来的。
        let seg = SegmentHeader::new(1, 0, 0, 4096, 4, 4096).encode();
        let cmt = CommitRecord::new(42, 0, 0, 0).encode();
        let page = PageHeader::new(7, 1, 0).encode();
        let run = PageRun::new(7, 8192, 2, 0).encode();
        // 四个 decode 签名不同，把返回值统一抹平成 () 才能放进同一张表里逐档扫描。
        type Decode = fn(&[u8]) -> Result<(), Error>;
        let cases: [(&str, &[u8], Decode); 4] = [
            ("段头", &seg, |b| SegmentHeader::decode(b).map(|_| ())),
            ("提交记录", &cmt, |b| {
                CommitRecord::decode(b).map(|_| ())
            }),
            ("页头", &page, |b| PageHeader::decode(b).map(|_| ())),
            ("页槽组", &run, |b| PageRun::decode(b).map(|_| ())),
        ];

        let mut checked = 0usize;
        for (name, bytes, decode) in cases {
            for len in 0..bytes.len() {
                assert_eq!(
                    decode(&bytes[..len]),
                    Err(Error::InvalidArgument),
                    "{name} 的缓冲区只有 {len} 字节（需 {}），却没报调用方错误",
                    bytes.len()
                );
                checked += 1;
            }
            // 反面对照：长度够时必须解码成功，说明上面的 InvalidArgument
            // 确实来自长度检查，而不是被别的条件顺带触发。
            assert_eq!(decode(bytes), Ok(()), "{name} 的完整缓冲区本应解码成功");
        }
        assert_eq!(
            checked,
            SEGMENT_HEADER_SIZE + COMMIT_RECORD_SIZE + PAGE_HEADER_SIZE + PAGE_RUN_ENTRY_SIZE,
            "四个解码器的每一个短长度都必须被覆盖，不能有漏网的"
        );
    }
}
