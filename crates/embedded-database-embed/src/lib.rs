#![no_std]
#![forbid(unsafe_code)]

//! embedded-database 公开嵌入 API（`Db` / `Txn` 门面）。
//!
//! # 定位
//!
//! 本 crate 是 `embedded-database-core` 存储引擎面向外部使用者的公开面：调用者只需依赖
//! `embedded-database-embed`，不必接触 core 的内部类型与观测接口。实现是**显式转发**
//! （不 `pub use` core 的 `Db`/`Txn`），因此 core 的内部演进不会自动改变本 crate
//! 的公开面。
//!
//! # 特性
//!
//! - `std`：启用完整门面（`Db` / `Txn`），依赖 `embedded-database-core/std`。
//! - 默认（no_std）：仅导出 `Placeholder`，保证裸机目标可编译（C1）。
//!
//! # 稳定面承诺
//!
#![cfg_attr(
    feature = "std",
    doc = "稳定面 = [`Db`] / [`Txn`] 及其方法签名，以及本 crate 再导出的类型"
)]
#![cfg_attr(
    feature = "std",
    doc = "（[`BlockDevice`]、[`Config`]、[`TxnOptions`]、[`Deadline`]、[`Error`] 等）。"
)]
#![cfg_attr(
    not(feature = "std"),
    doc = "稳定面 = `Db` / `Txn` 及其方法签名，以及本 crate 再导出的类型"
)]
#![cfg_attr(
    not(feature = "std"),
    doc = "（`BlockDevice`、`Config`、`TxnOptions`、`Deadline`、`Error` 等）。"
)]
//! 对稳定面的不兼容变更必须提升 minor/major 版本并在提交信息中说明
//! （仓库当前无 `CHANGELOG.md`；在引入 CHANGELOG 前以版本号 + 提交信息承载）。
//! `Placeholder` 与内部模块 `imp` 不在承诺范围内。
//!
//! # 明确不做
//!
//! `delete`/墓碑、范围迭代（`Cursor` 不外泄）、C/FFI ABI、真正的 no_std 可用
//! （core 的 `Db`/`Txn` 仍受 `std` 门控）、core 的内部观测面
//! （`page_map_entries`/`read_payload`/`try_lock_writer` 等）与 M2 占位实现
//! （`Db::compact`）。

// 测试与 std 路径需要 std（Vec、format! 等）。
#[cfg(any(test, feature = "std"))]
extern crate std;

/// std 路径：完整门面实现。
#[cfg(any(test, feature = "std"))]
mod imp {
    use core::fmt;
    use std::vec::Vec;

    // 参数/返回类型与错误：直接复用 core，不引入 embed 自有类型。
    // EncryptionKey 是 Config.encryption_key 的字段类型，必须一并导出，
    // 否则调用者无法构造启用加密的配置。Sm4Key 是 EncryptionKey.sm4 的
    // 字段类型，core 只给了结构体定义（无 Default、无构造函数），
    // 不导出则调用者连 EncryptionKey 都无法拼出来，故随 EncryptionKey 一并导出。
    pub use embedded_database_core::{
        BlockDevice, CommitInfo, CompactReport, CompactionBudget, Config, DbStats, Deadline,
        EncryptionKey, Error, InMemoryDevice, IntegrityLevel, Result, Sm4Key, TxnOptions,
    };

    /// 嵌入式数据库句柄（公开门面，包装 `embedded_database_core::Db`）。
    ///
    /// # 示例
    ///
    /// ```
    /// use embedded_database_embed::{Config, Db, InMemoryDevice};
    ///
    /// let mut dev = InMemoryDevice::new(4 << 20);
    /// let mut db = Db::open(&mut dev, Config::default()).unwrap();
    /// db.put(b"key", b"value").unwrap();
    /// assert_eq!(db.get(b"key").unwrap(), Some(b"value".to_vec()));
    /// ```
    pub struct Db<'d> {
        inner: embedded_database_core::Db<'d>,
    }

    impl<'d> fmt::Debug for Db<'d> {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_struct("Db").finish_non_exhaustive()
        }
    }

    impl<'d> Db<'d> {
        /// 打开数据库。冷启动时扫描已有段并重建映射表。
        pub fn open(dev: &'d mut dyn BlockDevice, cfg: Config) -> Result<Db<'d>> {
            Ok(Db {
                inner: embedded_database_core::Db::open(dev, cfg)?,
            })
        }

        /// 当前配置（打开时确定，运行期不变）。
        pub fn config(&self) -> &Config {
            self.inner.config()
        }

        /// 下一个写入将使用的段号。
        pub fn active_segment(&self) -> u64 {
            self.inner.active_segment()
        }

        /// 计数快照（commit/read/write/error 计数与最近一次错误）。
        pub fn stats(&self) -> DbStats {
            self.inner.stats()
        }

        /// 便捷写入：`begin` → `put` → `commit` → `flush`。
        ///
        /// 返回 `flush` 的提交结果（真正落盘的段号与字节数）。
        /// key 含 `0xFF` 时返回 `Err(Error::InvalidArgument)`：payload 以首个
        /// `0xFF` 分隔 key/value。批量写入请自行 `begin` + 多次 `Txn::put` +
        /// `flush`——本方法每次调用都完整走一遍提交与刷盘（写放大）。
        pub fn put(&mut self, k: &[u8], v: &[u8]) -> Result<CommitInfo> {
            let mut txn = self.begin(TxnOptions::default())?;
            txn.put(k, v)?;
            txn.commit()?;
            self.flush()
        }

        /// 读取键值。未命中返回 `Ok(None)`。
        ///
        /// 只读**已 flush** 的提交（`Txn::commit` 仅写入 `pending_writes`）；
        /// `Db::put` 自带 flush，故「put 后可 get」。
        /// **不带 deadline**：需要超时语义的读请用
        /// `begin(TxnOptions { deadline })` 再调 `Txn::get`。
        ///
        /// 实现说明：`payload` 解析（`key ++ 0xFF ++ value`）与 core `Txn::get`、
        /// `embedded-database-kv` 同构——core 已冻结，且 `&self` 读路径不能取写者锁，
        /// 故就地复刻而非改 core 抽象。
        pub fn get(&self, k: &[u8]) -> Result<Option<Vec<u8>>> {
            if let Some(page_id) = self.inner.lookup_key(k) {
                let payload = self.inner.read_payload(page_id)?;
                // payload 格式：key ++ 0xFF ++ value
                if let Some(pos) = payload.iter().position(|&b| b == 0xFF) {
                    if &payload[..pos] == k {
                        return Ok(Some(payload[pos + 1..].to_vec()));
                    }
                }
            }
            Ok(None)
        }

        /// 把 `pending_writes` 落盘到活跃段（同段追加或开新段）。
        pub fn flush(&mut self) -> Result<CommitInfo> {
            self.inner.flush()
        }

        /// 预算化段合并。
        ///
        /// Deadline 契约（C7）：过期且 `budget.interruptible = true` → 返回
        /// `interrupted: true` 的报告；过期且 `interruptible = false` →
        /// `Err(Error::DeadlineExceeded)`；未过期 → 正常执行。
        pub fn compact_with_budget(
            &mut self,
            budget: &CompactionBudget,
            deadline: Option<Deadline>,
        ) -> Result<CompactReport> {
            self.inner.compact_with_budget(budget, deadline)
        }

        /// 开启事务。写者锁被占用时返回 `Err(Error::Busy)`（绝不等待）。
        pub fn begin(&mut self, opts: TxnOptions) -> Result<Txn<'_, 'd>> {
            Ok(Txn {
                inner: self.inner.begin(opts)?,
            })
        }
    }

    /// 事务句柄（公开门面，包装 `embedded_database_core::Txn`）。
    ///
    /// # 示例
    ///
    /// ```
    /// use embedded_database_embed::{Config, Db, InMemoryDevice, TxnOptions};
    ///
    /// let mut dev = InMemoryDevice::new(4 << 20);
    /// let mut db = Db::open(&mut dev, Config::default()).unwrap();
    /// let mut txn = db.begin(TxnOptions::default()).unwrap();
    /// txn.put(b"k1", b"v1").unwrap();
    /// txn.put(b"k2", b"v2").unwrap();
    /// txn.commit().unwrap();
    /// db.flush().unwrap(); // commit 只进 pending_writes，flush 才落盘
    /// assert_eq!(db.get(b"k1").unwrap(), Some(b"v1".to_vec()));
    /// ```
    ///
    /// 未 `commit` 即 drop：变更丢弃，写者锁由 core 的 `Txn::Drop` 释放
    /// （本类型不实现 `Drop`，以保住 `commit(self)` 的字段移动）。
    pub struct Txn<'db, 'd> {
        inner: embedded_database_core::Txn<'db, 'd>,
    }

    impl<'db, 'd> fmt::Debug for Txn<'db, 'd> {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_struct("Txn").finish_non_exhaustive()
        }
    }

    impl<'db, 'd> Txn<'db, 'd> {
        /// 事务 deadline（来自 `TxnOptions::deadline`）。
        pub fn deadline(&self) -> Option<Deadline> {
            self.inner.deadline()
        }

        /// 写入键值对。key 含 `0xFF` 或 deadline 已过期时返回错误。
        pub fn put(&mut self, k: &[u8], v: &[u8]) -> Result<()> {
            self.inner.put(k, v)
        }

        /// 读取键值：先查本事务变更（last-wins），再查已提交数据。
        pub fn get(&self, k: &[u8]) -> Result<Option<Vec<u8>>> {
            self.inner.get(k)
        }

        /// 提交事务（变更进入 `pending_writes`，需 `Db::flush` 才落盘）。
        pub fn commit(self) -> Result<CommitInfo> {
            self.inner.commit()
        }
    }
}

#[cfg(any(test, feature = "std"))]
pub use imp::*;

/// no_std 路径占位类型。完整门面需要 `std` 特性。
#[cfg(not(any(test, feature = "std")))]
#[derive(Debug)]
pub struct Placeholder;

#[cfg(test)]
mod tests {
    use super::*;
    use std::format;
    use std::path::PathBuf;
    use embedded_database_os::FileDevice;

    #[test]
    fn 门面_基本读写往返() {
        let mut dev = InMemoryDevice::new(4 << 20);
        let mut db = Db::open(&mut dev, Config::default()).unwrap();
        let info = db.put(b"key1", b"value1").unwrap();
        assert!(info.segment_seq >= 1);
        assert_eq!(db.get(b"key1").unwrap(), Some(b"value1".to_vec()));
    }

    #[test]
    fn 门面_未写入键返回_none() {
        let mut dev = InMemoryDevice::new(4 << 20);
        let mut db = Db::open(&mut dev, Config::default()).unwrap();
        db.put(b"present", b"v").unwrap();
        assert_eq!(db.get(b"missing").unwrap(), None);
    }

    #[test]
    fn 门面_put_含0xff的key被拒() {
        let mut dev = InMemoryDevice::new(4 << 20);
        let mut db = Db::open(&mut dev, Config::default()).unwrap();
        assert_eq!(
            db.put(&[0x01, 0xFF, 0x02], b"value"),
            Err(Error::InvalidArgument)
        );
    }

    #[test]
    fn 门面_提交后flush前不可见_flush后可见() {
        let mut dev = InMemoryDevice::new(4 << 20);
        let mut db = Db::open(&mut dev, Config::default()).unwrap();
        let mut txn = db.begin(TxnOptions::default()).unwrap();
        txn.put(b"k", b"v").unwrap();
        txn.commit().unwrap();
        // commit 只进 pending_writes，未 flush 前 key_map 里没有该键
        assert_eq!(db.get(b"k").unwrap(), None);
        db.flush().unwrap();
        assert_eq!(db.get(b"k").unwrap(), Some(b"v".to_vec()));
    }

    #[test]
    fn 门面_未提交事务drop后变更丢弃且锁释放() {
        let mut dev = InMemoryDevice::new(4 << 20);
        let mut db = Db::open(&mut dev, Config::default()).unwrap();
        {
            let mut txn = db.begin(TxnOptions::default()).unwrap();
            txn.put(b"k", b"v").unwrap();
            // 不 commit，直接 drop
        }
        // 能再次 begin ⇒ core 的 Txn::Drop 已释放写者锁
        let txn2 = db.begin(TxnOptions::default()).unwrap();
        drop(txn2);
        assert_eq!(db.get(b"k").unwrap(), None);
    }

    #[test]
    fn 门面_compact_with_budget后数据仍可回读() {
        let mut dev = InMemoryDevice::new(8 << 20);
        let mut db = Db::open(&mut dev, Config::default()).unwrap();
        db.put(b"k1", b"v1").unwrap();
        db.put(b"k2", b"v2").unwrap();
        let report = db
            .compact_with_budget(&CompactionBudget::default(), None)
            .unwrap();
        assert!(!report.interrupted);
        assert_eq!(db.get(b"k1").unwrap(), Some(b"v1".to_vec()));
        assert_eq!(db.get(b"k2").unwrap(), Some(b"v2".to_vec()));
    }

    #[test]
    fn 门面_stats随操作增长() {
        let mut dev = InMemoryDevice::new(4 << 20);
        let mut db = Db::open(&mut dev, Config::default()).unwrap();
        assert_eq!(db.stats().commit_count, 0);
        assert_eq!(db.stats().write_count, 0);
        db.put(b"k", b"v").unwrap();
        assert_eq!(db.stats().commit_count, 1);
        assert_eq!(db.stats().write_count, 1);
    }

    #[test]
    fn 门面_过期deadline使put返回_deadlineexceeded() {
        // core 的 `begin` 会在抢锁前校验 deadline（`dl.remaining()?`，db.rs:622-624），
        // 已过期值无法开成事务。与 core 自身用例 `deadline_传播至_put` 同构：
        // 先用未过期 deadline 开事务，再推进时钟使其过期。
        use embedded_database_core::{reset_clock, set_clock_offset};
        reset_clock();
        let mut dev = InMemoryDevice::new(4 << 20);
        let mut db = Db::open(&mut dev, Config::default()).unwrap();
        let opts = TxnOptions {
            deadline: Some(Deadline::after_millis(10)),
        };
        let mut txn = db.begin(opts).unwrap();
        set_clock_offset(1_000_000);
        assert_eq!(txn.put(b"k", b"v"), Err(Error::DeadlineExceeded));
        reset_clock();
    }

    #[test]
    fn 门面_配置与活跃段可访问_加密配置仅凭再导出类型构造() {
        // Sm4Key 无 Default、无构造函数，但 `bytes` 字段为 pub：
        // 仅用 embedded_database_embed 再导出的类型即可拼出 EncryptionKey → 加密 Config。
        let cfg = Config {
            encryption_key: Some(EncryptionKey {
                sm4: Sm4Key { bytes: [0x42; 16] },
                hmac: [0x7F; 32],
            }),
            ..Config::default()
        };
        let mut dev = InMemoryDevice::new(4 << 20);
        let mut db = Db::open(&mut dev, cfg).unwrap();
        assert_eq!(db.config().page_size, Config::default().page_size);
        assert!(db.active_segment() >= 1);
        db.put(b"enc", b"v").unwrap();
        assert_eq!(db.get(b"enc").unwrap(), Some(b"v".to_vec()));
    }

    /// 每个用例独立临时文件，避免并行测试互相污染（与 os/file.rs 的 temp_path 同款）。
    fn 临时路径() -> PathBuf {
        static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "embedded-database-embed-test-{}-{}.db",
            std::process::id(),
            n
        ))
    }

    #[test]
    fn 门面_文件后端持久化重开后回读() {
        let path = 临时路径();
        // 设备容量 4 MiB = 默认 Config 的 max_segments(4) × segment_size(1 MiB)，
        // 保证段槽位不会越界，测的是持久化而不是边界。
        {
            let mut dev = FileDevice::open(&path, 4 << 20).unwrap();
            let mut db = Db::open(&mut dev, Config::default()).unwrap();
            db.put(b"persisted", b"value").unwrap();
            // db 在此作用域结束时先于 dev drop（逆序），借用关系满足
        }

        let mut dev = FileDevice::open(&path, 4 << 20).unwrap();
        let db = Db::open(&mut dev, Config::default()).unwrap();
        assert_eq!(db.get(b"persisted").unwrap(), Some(b"value".to_vec()));

        drop(db);
        drop(dev);
        let _ = std::fs::remove_file(&path);
    }
}
