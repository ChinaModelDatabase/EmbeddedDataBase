#![no_std]
#![forbid(unsafe_code)]

//! embedded-database KV 访问接口。
//!
//! # 特性
//!
//! - `std`：启用完整 [`KvDb`] 实现（依赖 `embedded-database-core/std` 导出的 `Db`）。
//! - 默认（no_std）：仅导出 [`Placeholder`] 占位类型，保证裸机目标可编译（C1）。

// 测试在 host 上编译，需要 std 路径的 KvDb 实现。
#[cfg(any(test, feature = "std"))]
extern crate std;

/// std 路径：完整 KV 实现。
#[cfg(any(test, feature = "std"))]
mod imp {
    use embedded_database_core::{BlockDevice, Config, Db, Error, Result};

    /// 类型化 KV 数据库。包装 `embedded_database_core::Db`。
    pub struct KvDb<'d> {
        db: Db<'d>,
    }

    impl<'d> core::fmt::Debug for KvDb<'d> {
        fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
            f.debug_struct("KvDb").finish_non_exhaustive()
        }
    }

    impl<'d> KvDb<'d> {
        /// 打开 KV 数据库。
        pub fn open(dev: &'d mut dyn BlockDevice, cfg: Config) -> Result<KvDb<'d>> {
            Ok(KvDb {
                db: Db::open(dev, cfg)?,
            })
        }

        /// 写入键值对。
        ///
        /// key 含 `0xFF` 时返回 `InvalidArgument`：payload 以首个 `0xFF`
        /// 分隔 key/value，key 内出现该字节会导致 get 解析错读。
        pub fn put(&mut self, k: &[u8], v: &[u8]) -> Result<()> {
            if k.contains(&0xFF) {
                return Err(Error::InvalidArgument);
            }
            let mut txn = self.db.begin(Default::default())?;
            txn.put(k, v)?;
            txn.commit()?;
            self.db.flush()?;
            Ok(())
        }

        /// 批量写入。累积多条变更到 pending_writes，一次 flush 落盘。
        pub fn put_batch(&mut self, items: &[(&[u8], &[u8])]) -> Result<()> {
            for (k, _) in items {
                if k.contains(&0xFF) {
                    return Err(Error::InvalidArgument);
                }
            }
            let mut txn = self.db.begin(Default::default())?;
            for (k, v) in items {
                txn.put(k, v)?;
            }
            txn.commit()?;
            self.db.flush()?;
            Ok(())
        }

        /// 读取键值。未命中返回 `None`。
        ///
        /// M9 简化：直接调用 `Db::lookup_key` + `read_payload`，不经过 `Txn`
        /// （`begin` 需要 `&mut self`，与 `get(&self)` 冲突）。
        pub fn get(&self, k: &[u8]) -> Result<Option<std::vec::Vec<u8>>> {
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

        /// 落盘。
        pub fn flush(&mut self) -> Result<()> {
            self.db.flush()?;
            Ok(())
        }

        /// 范围扫描：返回所有 key 以 `prefix` 开头的 (key, value) 对。
        pub fn scan(
            &self,
            prefix: &str,
        ) -> Result<std::vec::Vec<(std::string::String, std::vec::Vec<u8>)>> {
            let mut results = std::vec::Vec::new();
            // M19 简化：从 Db 的 page_map 线性扫描
            for (page_id, _, _) in self.db.page_map_entries() {
                if let Ok(payload) = self.db.read_payload(*page_id) {
                    if let Some(pos) = payload.iter().position(|&b| b == 0xFF) {
                        let key =
                            std::string::String::from_utf8_lossy(&payload[..pos]).into_owned();
                        if key.starts_with(prefix) {
                            let value = payload[pos + 1..].to_vec();
                            results.push((key, value));
                        }
                    }
                }
            }
            Ok(results)
        }
    }
}

/// no_std 路径占位类型。完整 KV 实现需要 `std` 特性。
#[cfg(not(any(test, feature = "std")))]
pub struct Placeholder;

#[cfg(any(test, feature = "std"))]
pub use imp::*;

#[cfg(test)]
mod tests {
    use embedded_database_core::{Config, Error, InMemoryDevice};

    #[test]
    fn kvdb_基本读写() {
        let mut dev = InMemoryDevice::new(1 << 20);
        let mut kv = super::KvDb::open(&mut dev, Config::default()).unwrap();
        kv.put(b"key1", b"value1").unwrap();
        kv.flush().unwrap();
        let value = kv.get(b"key1").unwrap();
        assert_eq!(value, Some(b"value1".to_vec()));
    }

    #[test]
    fn kvdb_未写入_key返回_none() {
        let mut dev = InMemoryDevice::new(1 << 20);
        let kv = super::KvDb::open(&mut dev, Config::default()).unwrap();
        let value = kv.get(b"missing").unwrap();
        assert_eq!(value, None);
    }

    #[test]
    fn kvdb_put_含0xff的key返回_invalidargument() {
        let mut dev = InMemoryDevice::new(1 << 20);
        let mut kv = super::KvDb::open(&mut dev, Config::default()).unwrap();
        let result = kv.put(&[0x01, 0xFF, 0x02], b"value");
        assert_eq!(result, Err(Error::InvalidArgument));
    }

    #[test]
    fn kvdb_范围扫描() {
        let mut dev = InMemoryDevice::new(1 << 20);
        let mut kv = super::KvDb::open(&mut dev, Config::default()).unwrap();
        kv.put(b"users:alice", b"name=alice").unwrap();
        kv.put(b"users:bob", b"name=bob").unwrap();
        kv.put(b"orders:1", b"item=x").unwrap();
        kv.flush().unwrap();
        let results = kv.scan("users:").unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].0, "users:alice");
        assert_eq!(results[1].0, "users:bob");
    }

    #[test]
    fn kvdb_范围扫描_空前缀() {
        let mut dev = InMemoryDevice::new(1 << 20);
        let mut kv = super::KvDb::open(&mut dev, Config::default()).unwrap();
        kv.put(b"key1", b"v1").unwrap();
        kv.put(b"key2", b"v2").unwrap();
        kv.flush().unwrap();
        let results = kv.scan("").unwrap();
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn kvdb_put_batch_批量写入() {
        let mut dev = InMemoryDevice::new(1 << 20);
        let mut kv = super::KvDb::open(&mut dev, Config::default()).unwrap();
        let items = [
            (b"k1".as_ref(), b"v1".as_ref()),
            (b"k2".as_ref(), b"v2".as_ref()),
            (b"k3".as_ref(), b"v3".as_ref()),
        ];
        kv.put_batch(&items).unwrap();
        kv.flush().unwrap();
        assert_eq!(kv.get(b"k1").unwrap(), Some(b"v1".to_vec()));
        assert_eq!(kv.get(b"k2").unwrap(), Some(b"v2".to_vec()));
        assert_eq!(kv.get(b"k3").unwrap(), Some(b"v3".to_vec()));
    }

    #[test]
    fn kvdb_put_batch_含0xff被拒() {
        let mut dev = InMemoryDevice::new(1 << 20);
        let mut kv = super::KvDb::open(&mut dev, Config::default()).unwrap();
        let items = [
            (b"ok".as_ref(), b"v".as_ref()),
            ([0x01, 0xFF, 0x02].as_ref(), b"v".as_ref()),
        ];
        assert!(kv.put_batch(&items).is_err());
    }

    #[test]
    fn kvdb_范围扫描_无匹配() {
        let mut dev = InMemoryDevice::new(1 << 20);
        let mut kv = super::KvDb::open(&mut dev, Config::default()).unwrap();
        kv.put(b"key1", b"v1").unwrap();
        kv.flush().unwrap();
        let results = kv.scan("missing:").unwrap();
        assert!(results.is_empty());
    }
}
