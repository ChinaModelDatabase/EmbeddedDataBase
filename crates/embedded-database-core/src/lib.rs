#![no_std]
#![forbid(unsafe_code)]

//! embedded-database 存储引擎内核。
//!
//! 设计约束（不得违反）：
//! - `no_std`：所有可变容量内存由调用方提供的 `&mut [u8]` 承载。
//! - 零 `unsafe`：全 crate `#![forbid(unsafe_code)]`。
//! - 确定性：操作有可测量的时延上界，容量固定，失败显式。

// ⚠️ 关键：`#![no_std]` 不会因为 `cfg(test)` 自动获得 std。
// `cargo test` 编译的仍是 no_std crate，因此测试里的 Vec/String/format! 必须
// 显式引入。`std` 特性是 M1 中少数几个"允许用 Vec"的场景，仅测试与 bench 使用。
#[cfg(any(test, feature = "std"))]
extern crate std;

// 以下模块在 M1 走 std 路径（内部用 Vec 组织可变长度列表）。
// 门控放在 `mod` 声明处而非逐个 item——门控一次，`no_std` 构建就整体剔除，
// 不会因为某个 item 漏加 cfg 而让 no_std 构建失败。
// M3 会把它们的 std 依赖替换为 arena 内的定长结构，届时移除这些门控。
mod arena;
#[cfg(feature = "std")]
mod arenapool;
mod btree;
mod crc;
mod crypto;
#[cfg(feature = "std")]
mod cursor;
#[cfg(feature = "std")]
mod db;
mod device;
mod hashtable;
mod hmac;
mod pagemap;
mod segment;
mod sm3;
mod sm4;
mod types;

#[cfg(feature = "std")]
mod memtable;
#[cfg(feature = "std")]
mod recovery;
#[cfg(feature = "std")]
mod slot;
#[cfg(feature = "std")]
mod writer;

pub use arena::Arena;
#[cfg(feature = "std")]
pub use arenapool::ArenaPool;
pub use btree::{BTree, LEAF_CAPACITY, ROOT_CAPACITY};
#[cfg(feature = "std")]
pub use btree::{BTreeEntry, BTreeScan};
pub use crc::{fold_crcs, hash_bytes, Crc32c, CrcChain};
pub use crypto::EncryptionKey;
#[cfg(feature = "std")]
pub use crypto::{decrypt_page, encrypt_page};
#[cfg(feature = "std")]
pub use cursor::Cursor;
#[cfg(feature = "std")]
pub use db::{CompactReport, Db, Txn};
pub use device::BlockDevice;
#[cfg(feature = "std")]
pub use device::{InMemoryDevice, WriteStats};
pub use hashtable::{FixedTable, ScanEntry, MAX_PROBES};
pub use hmac::hmac_sm3;
#[cfg(feature = "std")]
pub use memtable::Memtable;
pub use pagemap::PageMap;
#[cfg(feature = "std")]
pub use recovery::{recover, RecoveryReport};
pub use segment::{
    CommitRecord, PageHeader, PageRun, SegmentHeader, COMMIT_MAGIC, COMMIT_RECORD_SIZE,
    PAGE_HEADER_SIZE, PAGE_RUN_ENTRY_SIZE, SEGMENT_HEADER_SIZE, SEGMENT_MAGIC, SLOT_FLAG_FROZEN,
};
#[cfg(feature = "std")]
pub use slot::{SlotAllocation, SlotAllocator};
pub use sm3::sm3_hash;
pub use sm4::{derive_iv, sm4_decrypt_block, sm4_encrypt_block, Sm4Key};
#[cfg(feature = "std")]
pub use sm4::{sm4_cbc_decrypt, sm4_cbc_encrypt};
#[cfg(feature = "std")]
pub use types::DbStats;
pub use types::{
    now_micros, reset_clock, set_clock_offset, CommitInfo, CompactionBudget, Config, Deadline,
    IntegrityLevel, TxnOptions,
};
#[cfg(feature = "std")]
pub use writer::{AppendedPage, SegmentConfig, SegmentWriter};

/// 库级统一错误类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// 写者锁被占用。调用方应重试——embedded-database 永不等待（spec §7.1）。
    Busy,
    /// 超过调用方指定的 Deadline。embedded-database 永不阻塞（spec §5.4）。
    DeadlineExceeded,
    /// 固定容量结构已满。**不可恢复**，需提高配置容量后重试。
    CapacityExhausted,
    /// 存储配额超限。
    QuotaExceeded,
    /// 键不存在。
    KeyNotFound,
    /// 数据损坏。`segment_seq` 为出问题的段序列号。
    ///
    /// 只在解码方**确实持有段上下文**时使用（`SegmentHeader::decode` /
    /// `CommitRecord::decode`）。段序号 0 是合法值，不能拿它当「未知」的哨兵——
    /// 那会让「段 0 损坏」与「来源不明」无法区分。
    Corruption { segment_seq: u64 },
    /// 单个页或槽表条目损坏，且损坏位置不在段上下文内（冷启动扫描单个页时）。
    CorruptedPage { page_id: u64 },
    /// 段中不存在给定 page_id。
    PageNotFound { page_id: u64 },
    /// 底层设备 I/O 失败。
    Io,
    /// 缓冲区越界或长度不合法。
    InvalidArgument,
    /// 页的 k 个槽已用尽且已冻结。
    SlotFrozen { page_id: u64 },
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Error::Busy => f.write_str("busy: writer lock held"),
            Error::DeadlineExceeded => f.write_str("deadline exceeded"),
            Error::CapacityExhausted => {
                f.write_str("capacity exhausted: fixed-capacity structure is full")
            }
            Error::QuotaExceeded => f.write_str("quota exceeded"),
            Error::KeyNotFound => f.write_str("key not found"),
            Error::Corruption { segment_seq } => write!(f, "corruption in segment {segment_seq}"),
            Error::CorruptedPage { page_id } => write!(f, "page {page_id} corrupted"),
            Error::PageNotFound { page_id } => write!(f, "page {page_id} not found"),
            Error::Io => f.write_str("io error"),
            Error::InvalidArgument => f.write_str("invalid argument"),
            Error::SlotFrozen { page_id } => write!(f, "page {page_id} has no free slot"),
        }
    }
}

/// 库的 Result 别名。
pub type Result<T> = core::result::Result<T, Error>;
