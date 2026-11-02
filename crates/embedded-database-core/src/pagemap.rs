//! page_id → (segment_seq, offset) 映射表。基于 FixedTable 实现。

use crate::arena::Arena;
use crate::hashtable::FixedTable;
use crate::Error;

/// 映射表。键 = page_id (8B)，值 = segment_seq (8B) ++ offset (8B)。
#[derive(Debug)]
pub struct PageMap<'a, 'b> {
    table: FixedTable<'a, 'b>,
}

impl<'a, 'b> PageMap<'a, 'b> {
    pub fn new(arena: &'a mut Arena<'b>, capacity: usize) -> Result<Self, Error> {
        Ok(PageMap {
            table: FixedTable::new(arena, capacity)?,
        })
    }

    pub fn len(&self) -> usize {
        self.table.len()
    }

    pub fn is_empty(&self) -> bool {
        self.table.is_empty()
    }

    /// 插入或更新映射。
    pub fn insert(&mut self, page_id: u64, segment_seq: u64, offset: u64) -> Result<(), Error> {
        let key = page_id.to_le_bytes();
        let mut val = [0u8; 16];
        val[..8].copy_from_slice(&segment_seq.to_le_bytes());
        val[8..].copy_from_slice(&offset.to_le_bytes());
        self.table.insert(&key, &val, 0)
    }

    /// 查询映射。未找到返回 `None`。
    pub fn lookup(&self, page_id: u64) -> Result<Option<(u64, u64)>, Error> {
        let key = page_id.to_le_bytes();
        let Some(val) = self.table.get(&key)? else {
            return Ok(None);
        };
        if val.len() < 16 {
            return Err(Error::CorruptedPage { page_id });
        }
        let seq = u64::from_le_bytes(
            val[..8]
                .try_into()
                .map_err(|_| Error::CorruptedPage { page_id })?,
        );
        let off = u64::from_le_bytes(
            val[8..16]
                .try_into()
                .map_err(|_| Error::CorruptedPage { page_id })?,
        );
        Ok(Some((seq, off)))
    }

    /// 移除映射。M1 的 FixedTable 无删除接口，此处用 reset 语义简化：
    /// M2 阶段 compaction 后重建映射表而非逐条删除。
    pub fn remove(&mut self, _page_id: u64) -> Result<(), Error> {
        // M1 FixedTable 不支持单条删除。compaction 时重建整表。
        Ok(())
    }
}

#[cfg(all(test, feature = "std"))]
mod tests {
    use super::*;
    use std::vec;

    fn with_map<F: FnOnce(&mut PageMap)>(capacity: usize, buf_len: usize, f: F) {
        let mut buf = vec![0u8; buf_len];
        let mut arena = Arena::new(&mut buf);
        let mut map = PageMap::new(&mut arena, capacity).unwrap();
        f(&mut map);
    }

    #[test]
    fn 插入后可查询() {
        with_map(16, 4096, |m| {
            m.insert(1, 10, 100).unwrap();
            assert_eq!(m.lookup(1).unwrap(), Some((10, 100)));
            assert_eq!(m.len(), 1);
        });
    }

    #[test]
    fn 未插入返回空() {
        with_map(16, 4096, |m| {
            assert_eq!(m.lookup(99).unwrap(), None);
        });
    }

    #[test]
    fn 同键覆盖() {
        with_map(16, 4096, |m| {
            m.insert(1, 10, 100).unwrap();
            m.insert(1, 20, 200).unwrap();
            assert_eq!(m.lookup(1).unwrap(), Some((20, 200)));
            assert_eq!(m.len(), 1);
        });
    }

    // M2 简化：M1 FixedTable 无单条删除接口，`remove` 是空操作（恒返回 `Ok(())`）。
    // compaction 时重建整表，而非逐条删除。此测试仅验证空操作语义。
    #[test]
    fn 移除为空操作() {
        with_map(16, 4096, |m| {
            m.insert(1, 10, 100).unwrap();
            assert_eq!(m.remove(1), Ok(()));
        });
    }

    #[test]
    fn 容量耗尽返回容量不足() {
        with_map(1, 4096, |m| {
            m.insert(1, 10, 100).unwrap();
            assert_eq!(m.insert(2, 10, 100), Err(Error::CapacityExhausted));
        });
    }
}
