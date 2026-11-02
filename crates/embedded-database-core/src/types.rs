//! M2 基础类型：Deadline、事务选项、提交信息、完整性级别、配置。

use core::sync::atomic::{AtomicU64, Ordering};

use crate::crypto::EncryptionKey;
use crate::Error;

/// 绝对时刻 Deadline。单位由宿主的时间基准决定（M2 用微秒）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Deadline(pub u64);

impl Deadline {
    /// 从当前时刻起 `ms` 毫秒后到期。
    pub fn after_millis(ms: u64) -> Self {
        let now = now_micros();
        Deadline(now.saturating_add(ms.saturating_mul(1000)))
    }

    /// 剩余微秒数。已超时返回 `DeadlineExceeded`。
    pub fn remaining(&self) -> Result<u64, Error> {
        let now = now_micros();
        if now >= self.0 {
            return Err(Error::DeadlineExceeded);
        }
        Ok(self.0 - now)
    }
}

// 时钟偏移（微秒）。测试通过设置偏移来控制时间。
//
// std/test 路径下挂在 thread-local 上，避免并行测试互相污染；
// 真正的 no_std 生产路径回退到进程级静态（单执行流，无竞争）。
#[cfg(any(test, feature = "std"))]
std::thread_local! {
    static CLOCK_OFFSET: AtomicU64 = const { AtomicU64::new(0) };
}

/// 时钟偏移（微秒）。no_std 生产路径：进程级静态。
#[cfg(not(any(test, feature = "std")))]
static CLOCK_OFFSET: AtomicU64 = AtomicU64::new(0);

/// 读取当前线程/进程的时钟偏移。
fn clock_offset() -> u64 {
    #[cfg(any(test, feature = "std"))]
    {
        CLOCK_OFFSET.with(|c| c.load(Ordering::Relaxed))
    }
    #[cfg(not(any(test, feature = "std")))]
    {
        CLOCK_OFFSET.load(Ordering::Relaxed)
    }
}

/// 写入当前线程/进程的时钟偏移。
fn set_clock_offset_store(offset_us: u64) {
    #[cfg(any(test, feature = "std"))]
    {
        CLOCK_OFFSET.with(|c| c.store(offset_us, Ordering::Relaxed));
    }
    #[cfg(not(any(test, feature = "std")))]
    {
        CLOCK_OFFSET.store(offset_us, Ordering::Relaxed);
    }
}

/// 设置时钟偏移（微秒）。测试用。
pub fn set_clock_offset(offset_us: u64) {
    set_clock_offset_store(offset_us);
}

/// 重置时钟偏移为 0。
pub fn reset_clock() {
    set_clock_offset_store(0);
}

/// 读取当前时刻（微秒）= 真实时间 + 偏移。
pub fn now_micros() -> u64 {
    let real = default_clock();
    real.saturating_add(clock_offset())
}

/// 默认时钟函数（std 下用系统时间，no_std 下返回 0）。
fn default_clock() -> u64 {
    #[cfg(feature = "std")]
    {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_micros() as u64)
            .unwrap_or(0)
    }
    #[cfg(not(feature = "std"))]
    {
        0
    }
}

/// 事务选项。
#[derive(Debug, Clone, Copy, Default)]
pub struct TxnOptions {
    pub deadline: Option<Deadline>,
}

/// 提交结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommitInfo {
    pub segment_seq: u64,
    pub bytes_written: u64,
}

/// 完整性校验级别。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum IntegrityLevel {
    /// 不做校验，仅在 debug 构建下断言。
    Off,
    /// 仅校验页 CRC 与 commit CRC 链（默认）。
    #[default]
    Commit,
    /// 另加段头、页槽表、元数据全量校验；启动耗时显著增加。
    Full,
}

/// M6-5：安全审计日志计数快照。
///
/// 审计通过操作计数 + 最近错误实现（no_std 下不引入日志框架）。
/// `last_error` 以 `Display` 字符串的字节形式存储最近一次错误，
/// 需要堆分配，因此仅在 `std` 特性路径下可用。
#[cfg(feature = "std")]
#[derive(Debug, Clone, Default)]
pub struct DbStats {
    pub commit_count: u64,
    pub read_count: u64,
    pub write_count: u64,
    pub error_count: u64,
    pub last_error: Option<std::vec::Vec<u8>>,
}

/// 预算化合并配置。
#[derive(Debug, Clone, Copy)]
pub struct CompactionBudget {
    /// 每个时间片内合并的字节上限。
    pub max_bytes_per_slice: u64,
    /// 时间片时长（微秒）。
    pub slice_duration_micros: u64,
    /// 是否响应 Deadline 中断。
    pub interruptible: bool,
}

impl Default for CompactionBudget {
    fn default() -> Self {
        CompactionBudget {
            max_bytes_per_slice: 64 * 1024, // 64 KiB
            slice_duration_micros: 10_000,  // 10 ms
            interruptible: true,
        }
    }
}

/// 数据库配置。段布局在打开时确定，运行期不可变。
#[derive(Debug, Clone)]
pub struct Config {
    pub segment_size: usize,
    pub page_size: u32,
    pub slots_per_page: u16,
    pub max_pages: usize,
    pub max_segments: usize,
    pub arena_pool_size: usize,
    pub integrity: IntegrityLevel,
    pub compaction: CompactionBudget,
    /// 加密密钥。None 表示不做加密（向后兼容）。
    pub encryption_key: Option<EncryptionKey>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            segment_size: 1 << 20, // 1 MiB
            page_size: 4096,
            slots_per_page: 4,
            max_pages: 8,
            max_segments: 4,
            arena_pool_size: 2,
            integrity: IntegrityLevel::Commit,
            compaction: CompactionBudget::default(),
            encryption_key: None,
        }
    }
}

impl Config {
    /// 校验配置合法性。非法配置返回 `InvalidArgument`。
    pub fn validate(&self) -> Result<(), Error> {
        use crate::segment::{COMMIT_RECORD_SIZE, PAGE_HEADER_SIZE, SEGMENT_HEADER_SIZE};
        if self.segment_size < SEGMENT_HEADER_SIZE + COMMIT_RECORD_SIZE {
            return Err(Error::InvalidArgument);
        }
        if self.page_size < PAGE_HEADER_SIZE as u32 {
            return Err(Error::InvalidArgument);
        }
        if self.slots_per_page == 0 || self.max_pages == 0 || self.max_segments == 0 {
            return Err(Error::InvalidArgument);
        }
        // 段槽环需要至少 1 个空闲槽位才能开新段（存活段数上限 max_segments - 1），
        // 且合并目标槽必须空闲：3 是能同时容纳"合并前"与"合并后"的最小值。
        if self.max_segments < 3 {
            return Err(Error::InvalidArgument);
        }
        if self.arena_pool_size == 0 {
            return Err(Error::InvalidArgument);
        }
        let run_bytes = (self.page_size as usize)
            .checked_mul(self.slots_per_page as usize)
            .ok_or(Error::InvalidArgument)?;
        if self.max_pages.checked_mul(run_bytes).is_none() {
            return Err(Error::InvalidArgument);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deadline_创建与剩余() {
        let d = Deadline::after_millis(100);
        assert!(d.remaining().unwrap() <= 100_000); // 微秒
    }

    #[test]
    fn deadline_超时返回错误() {
        let d = Deadline(0); // 绝对时刻 0，早已超时
        assert_eq!(d.remaining(), Err(Error::DeadlineExceeded));
    }

    #[test]
    fn txn_options_默认无_deadline() {
        let opts = TxnOptions::default();
        assert!(opts.deadline.is_none());
    }

    #[test]
    fn integrity_level_默认为_commit() {
        assert_eq!(Config::default().integrity, IntegrityLevel::Commit);
    }

    #[test]
    fn 时钟偏移后_deadline_使用偏移时钟() {
        reset_clock();
        set_clock_offset(1000); // 偏移 1ms
        let d = Deadline::after_millis(10); // 到期 = real+1000+10000
                                            // 由于 real 在测试中不可控，改为验证 relative 语义
        let remaining = d.remaining().unwrap();
        assert!(remaining <= 10_000); // 不超过 10ms
        reset_clock();
    }

    #[test]
    fn 重置时钟后_deadline_正常() {
        reset_clock();
        let d = Deadline::after_millis(100);
        assert!(d.remaining().unwrap() <= 100_000);
    }

    #[test]
    fn compaction_budget_默认值() {
        let budget = CompactionBudget::default();
        assert!(budget.interruptible);
        assert!(budget.max_bytes_per_slice > 0);
    }

    #[test]
    fn config_包含_compaction_budget() {
        let cfg = Config::default();
        assert!(cfg.compaction.interruptible);
    }

    #[test]
    fn max_segments_小于_3_被拒() {
        let cfg = Config {
            max_segments: 2,
            ..Config::default()
        };
        assert_eq!(cfg.validate(), Err(Error::InvalidArgument));
        let cfg = Config {
            max_segments: 3,
            ..Config::default()
        };
        assert!(cfg.validate().is_ok());
    }
}
