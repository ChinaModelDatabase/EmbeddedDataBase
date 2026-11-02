//! 裸闪存抽象：页/擦除块几何与 NOR 1→0 编程语义。
//!
//! 这一层只描述"介质能做什么"，不含任何地址映射；映射由 `ftl` 负责。

/// 裸闪存操作错误。
///
/// 每种错误都携带出错的物理块号，便于 FTL 把块标坏或触发搬迁。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlashError {
    /// 块号或块内偏移/长度越界。
    OutOfRange,
    /// 编程范围跨越页边界。真实 NOR 的页编程不可跨页。
    Unaligned,
    /// 目标块已被实现标记为坏块。
    BadBlock { block: u32 },
    /// 编程失败：目标位置已有 `0` 但本次要写 `1`（违反写前擦除），
    /// 或介质/注入的硬错误。实现必须**整体拒绝**，不得部分写入。
    Program { block: u32 },
    /// 擦除失败（介质故障或注入）。
    Erase { block: u32 },
    /// 读取失败（介质故障或注入）。
    Read { block: u32 },
    /// 刷缓冲失败。
    Sync,
}

/// `RawFlash` 操作的返回类型。
pub type FlashResult<T> = core::result::Result<T, FlashError>;

/// 裸闪存设备。
///
/// 实现必须满足以下语义（FTL 依赖它们）：
/// - `read`/`program` 的 `offset` 是**块内字节偏移**，任何访问不得跨块。
/// - `program` 的写入范围不得跨越页边界，否则返回 [`FlashError::Unaligned`]。
/// - `program` 遵守 NOR 语义：只允许 `1 → 0`；任何 `0 → 1` 需求**整体拒绝**
///   并返回 [`FlashError::Program`]（不产生部分写入）。
/// - `erase` 把整块置为 `0xFF`，且幂等。
/// - 坏块由实现自行记录：`read`/`program`/`erase` 命中坏块一律返回
///   [`FlashError::BadBlock`]，FTL 据此把块退役。
pub trait RawFlash {
    /// 页大小（字节）。
    fn page_size(&self) -> usize;
    /// 擦除块大小（字节），必须是 `page_size` 的整数倍。
    fn erase_block_size(&self) -> usize;
    /// 物理擦除块总数。
    fn block_count(&self) -> u32;
    /// 从 `block` 的块内偏移 `offset` 读 `buf.len()` 字节。
    fn read(&self, block: u32, offset: usize, buf: &mut [u8]) -> FlashResult<()>;
    /// 把 `data` 编程到块内 `offset`（1→0 语义，不得跨页）。
    fn program(&mut self, block: u32, offset: usize, data: &[u8]) -> FlashResult<()>;
    /// 擦除整块（置 0xFF）。
    fn erase(&mut self, block: u32) -> FlashResult<()>;
    /// 落盘/刷缓冲。内存实现可为空操作。
    fn sync(&mut self) -> FlashResult<()>;
}
