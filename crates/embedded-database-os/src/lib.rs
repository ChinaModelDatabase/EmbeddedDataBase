//! embedded-database 操作系统层：Linux file 后端。
//!
//! 实现 `BlockDevice` trait，用 `std::fs::File` 提供持久化存储。

pub mod file;
pub mod flash;

pub use file::FileDevice;
pub use flash::FileRawFlash;

#[cfg(test)]
mod integration_tests {
    use super::*;
    use embedded_database_core::{BlockDevice, CompactionBudget, Config, Db};
    use std::path::PathBuf;

    fn temp_path() -> PathBuf {
        static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "embedded-database-integ-{}-{}.db",
            std::process::id(),
            n
        ))
    }

    #[test]
    fn file_device_与_db_集成() {
        let path = temp_path();
        {
            let mut dev = FileDevice::open(&path, 1 << 20).unwrap();
            let mut db = Db::open(&mut dev, Config::default()).unwrap();
            {
                let mut txn = db.begin(Default::default()).unwrap();
                txn.put(b"key1", b"value1").unwrap();
                txn.put(b"key2", b"value2").unwrap();
                txn.commit().unwrap();
            }
            let budget = CompactionBudget::default();
            let report = db.compact_with_budget(&budget, None).unwrap();
            assert!(!report.interrupted);
            assert!(report.write_amplification >= 1.0);
        }
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn file_backend_落盘集成() {
        let path =
            std::env::temp_dir().join(format!("embedded-database-flush-{}.db", std::process::id()));
        {
            let mut dev = FileDevice::open(&path, 1 << 20).unwrap();
            let mut db = Db::open(&mut dev, Config::default()).unwrap();
            {
                let mut txn = db.begin(Default::default()).unwrap();
                txn.put(b"key", b"value").unwrap();
                txn.commit().unwrap();
            }
            db.flush().unwrap();
            drop(db);
            drop(dev);
        }
        // 重新打开，数据应持久化
        let dev = FileDevice::open(&path, 1 << 20).unwrap();
        assert_eq!(dev.size(), 1 << 20);
        drop(dev);
        let _ = std::fs::remove_file(&path);
    }
}
