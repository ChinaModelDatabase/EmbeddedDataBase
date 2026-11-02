//! Memtable：读路径全内存，段文件仅冷启动扫描时使用。
//!
//! 不持有自己的缓冲区——借用调用方的 arena（见任务说明的"所有权决策"）。

use crate::arena::Arena;
use crate::hashtable::{FixedTable, ScanEntry};
use crate::Error;
use std::vec::Vec;

/// 内存工作区。所有键值字节在 arena 中，表结构在 arena 中。
#[derive(Debug)]
pub struct Memtable<'a, 'b> {
    table: FixedTable<'a, 'b>,
    next_seq: u32,
    dirty: bool,
}

impl<'a, 'b> Memtable<'a, 'b> {
    /// 在 arena 中建立 Memtable，容量 `slot_count` 固定。
    pub fn new(arena: &'a mut Arena<'b>, slot_count: usize) -> Result<Self, Error> {
        Ok(Memtable {
            table: FixedTable::new(arena, slot_count)?,
            next_seq: 0,
            dirty: false,
        })
    }

    pub fn len(&self) -> usize {
        self.table.len()
    }

    pub fn is_empty(&self) -> bool {
        self.table.is_empty()
    }

    pub fn capacity(&self) -> usize {
        self.table.capacity()
    }

    /// 自上次 commit 后是否有写入。
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// 写入键值对。同键覆盖。
    ///
    /// `next_seq` 仅在插入成功后推进——失败的 put 不消耗序号。
    pub fn put(&mut self, key: &[u8], value: &[u8]) -> Result<(), Error> {
        let seq = self
            .next_seq
            .checked_add(1)
            .ok_or(Error::CapacityExhausted)?;
        self.table.insert(key, value, seq)?;
        self.next_seq = seq;
        self.dirty = true;
        Ok(())
    }

    /// 查键。未命中返回 `None`。
    pub fn get(&self, key: &[u8]) -> Result<Option<&[u8]>, Error> {
        self.table.get(key)
    }

    /// 从 `start_seq` 起取至多 `limit` 个条目，按写入序。
    pub fn scan(&self, start_seq: u32, limit: usize) -> Result<Vec<ScanEntry<'_>>, Error> {
        let mut out = Vec::new();
        let mut cursor = start_seq;
        while out.len() < limit {
            match self.table.scan(cursor)? {
                Some(e) => {
                    cursor = e.seq;
                    out.push(e);
                }
                None => break,
            }
        }
        Ok(out)
    }

    /// 逻辑清空：重置表结构与序号游标，置脏标记。
    ///
    /// M1 **不回收** arena 空间——bump 分配器无法归还中间块。
    /// 空间回收属于 M2 的 arena 段复用。
    pub fn clear(&mut self) -> Result<(), Error> {
        self.table.reset()?;
        self.next_seq = 0;
        self.dirty = true;
        Ok(())
    }
}

#[cfg(all(test, feature = "std"))]
mod tests {
    use super::*;
    use std::vec;

    fn with_mt<F: FnOnce(&mut Memtable)>(buf_len: usize, slots: usize, f: F) {
        let mut buf = vec![0u8; buf_len];
        let mut arena = Arena::new(&mut buf);
        let mut mt = Memtable::new(&mut arena, slots).unwrap();
        f(&mut mt);
    }

    #[test]
    fn 写入后可读回() {
        with_mt(1 << 16, 128, |mt| {
            mt.put(b"a", b"1").unwrap();
            mt.put(b"b", b"2").unwrap();
            assert_eq!(mt.get(b"a").unwrap(), Some(&b"1"[..]));
            assert_eq!(mt.len(), 2);
            assert!(mt.is_dirty());
        });
    }

    #[test]
    fn 未命中返回空() {
        with_mt(1 << 16, 128, |mt| {
            assert_eq!(mt.get(b"nope").unwrap(), None);
            assert!(mt.is_empty());
            assert!(!mt.is_dirty());
        });
    }

    #[test]
    fn 同键覆盖() {
        with_mt(1 << 16, 128, |mt| {
            mt.put(b"k", b"v1").unwrap();
            mt.put(b"k", b"v1").unwrap();
            assert_eq!(mt.len(), 1);
        });
    }

    #[test]
    fn 扫描按写入序且游标推进() {
        with_mt(1 << 16, 128, |mt| {
            for k in [b"x", b"y", b"z"] {
                mt.put(k, b"v").unwrap();
            }
            let all = mt.scan(0, 10).unwrap();
            assert_eq!(all.len(), 3);
            assert_eq!(all[0].key, b"x");
            assert_eq!(all[2].key, b"z");
            // limit 生效
            let two = mt.scan(0, 2).unwrap();
            assert_eq!(two.len(), 2);
            // 从游标续扫
            let rest = mt.scan(two[1].seq, 10).unwrap();
            assert_eq!(rest.len(), 1);
            assert_eq!(rest[0].key, b"z");
        });
    }

    #[test]
    fn 清空后逻辑为空且仍脏() {
        with_mt(1 << 16, 128, |mt| {
            mt.put(b"a", b"1").unwrap();
            mt.clear().unwrap();
            assert_eq!(mt.get(b"a").unwrap(), None);
            assert_eq!(mt.len(), 0);
            assert!(mt.is_dirty(), "清空本身也是待提交的变更");
        });
    }

    #[test]
    fn 清空后可重新写入() {
        with_mt(1 << 16, 128, |mt| {
            mt.put(b"a", b"1").unwrap();
            mt.clear().unwrap();
            mt.put(b"b", b"2").unwrap();
            assert_eq!(mt.get(b"b").unwrap(), Some(&b"2"[..]));
            assert_eq!(mt.len(), 1);
        });
    }

    #[test]
    fn 容量耗尽显式失败() {
        with_mt(1 << 16, 2, |mt| {
            mt.put(b"a", b"1").unwrap();
            mt.put(b"b", b"1").unwrap();
            assert_eq!(mt.put(b"c", b"1"), Err(Error::CapacityExhausted));
        });
    }
}
