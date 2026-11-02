//! 固定容量开放寻址哈希表。不增长、不 rehash、无随机化。
//!
//! 容量在启动时固定，探测次数上限 `MAX_PROBES`。满则返回
//! `CapacityExhausted`——**绝不**扩容或退化（C7）。
//! 哈希函数固定（FNV-1a），保证同样的键在任何平台落在同样的桶。

use crate::arena::Arena;
use crate::crc::hash_bytes;
use crate::Error;

/// 单次查找/插入的最大线性探测步数。
pub const MAX_PROBES: usize = 8;

const SLOT_SIZE: usize = 24;
const ST_EMPTY: u16 = 0;
const ST_LIVE: u16 = 1;
const ST_TOMB: u16 = 2;

/// 固定容量哈希表。所有键值字节都存在 arena 中。
///
/// `'a` 是对 arena 的借用期，`'b` 是 arena 缓冲区的生命期。
/// 键值切片实际住在缓冲区里，但受限于 `&mut` 借用，对外返回的
/// 切片借用期绑定到 `&self`（见 `Arena::peek`）。
#[derive(Debug)]
pub struct FixedTable<'a, 'b> {
    arena: &'a mut Arena<'b>,
    slots_off: usize,
    slot_count: usize,
    len: usize,
}

impl<'a, 'b> FixedTable<'a, 'b> {
    /// 在 arena 中建立 `slot_count` 个槽的哈希表。
    ///
    /// 槽区一次性分配，之后永不增长。
    pub fn new(arena: &'a mut Arena<'b>, slot_count: usize) -> Result<Self, Error> {
        if slot_count == 0 {
            return Err(Error::InvalidArgument);
        }
        let slots_bytes = slot_count
            .checked_mul(SLOT_SIZE)
            .ok_or(Error::InvalidArgument)?;
        let slots_off = arena.alloc(slots_bytes, 8)?;
        // 槽区必须清零：空槽以 seq == 0 / state == ST_EMPTY 判定
        let zeros = [0u8; SLOT_SIZE];
        let mut addr = slots_off;
        for _ in 0..slot_count {
            arena.write(addr, &zeros)?;
            addr += SLOT_SIZE;
        }
        Ok(FixedTable {
            arena,
            slots_off,
            slot_count,
            len: 0,
        })
    }

    pub fn capacity(&self) -> usize {
        self.slot_count
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    fn slot_addr(&self, i: usize) -> usize {
        self.slots_off + i * SLOT_SIZE
    }

    /// 定位键所在的槽下标。返回 `Ok(None)` 表示键不存在。
    fn find(&self, key: &[u8]) -> Result<Option<usize>, Error> {
        let start = (hash_bytes(key) as usize) % self.slot_count;
        for step in 0..MAX_PROBES.min(self.slot_count) {
            let idx = (start + step) % self.slot_count;
            let addr = self.slot_addr(idx);
            let state = self.arena.read_u16(addr + 4)?;
            if state == ST_EMPTY {
                // 空槽终止探测——无删除时这是正确的
                return Ok(None);
            }
            if state == ST_TOMB {
                // 墓碑不终止探测（M1 无删除，墓碑不会出现）
                continue;
            }
            let kl = self.arena.read_u16(addr + 6)? as usize;
            let ko = self.arena.read_u32(addr + 10)? as usize;
            if self.arena.read(ko, kl)? == key {
                return Ok(Some(idx));
            }
        }
        Ok(None)
    }

    /// 插入或更新。同键重复插入原地覆盖旧值并更新 `version`。
    ///
    /// 值长度必须与旧值一致，否则返回 `InvalidArgument`（避免移动字节导致悬垂）。
    /// 容量不足返回 `CapacityExhausted`。
    pub fn insert(&mut self, key: &[u8], value: &[u8], version: u32) -> Result<(), Error> {
        if let Some(idx) = self.find(key)? {
            let addr = self.slot_addr(idx);
            let voff = self.arena.read_u32(addr + 14)? as usize;
            let vlen = self.arena.read_u16(addr + 8)? as usize;
            if vlen != value.len() {
                return Err(Error::InvalidArgument);
            }
            self.arena.write(voff, value)?;
            self.arena.write_u32(addr + 18, version)?;
            return Ok(());
        }

        // 找空槽：只认 ST_EMPTY（M1 无删除，探测序列里不会出现墓碑）
        let start = (hash_bytes(key) as usize) % self.slot_count;
        let mut target = None;
        for step in 0..MAX_PROBES.min(self.slot_count) {
            let idx = (start + step) % self.slot_count;
            let addr = self.slot_addr(idx);
            if self.arena.read_u16(addr + 4)? == ST_EMPTY {
                target = Some(idx);
                break;
            }
        }
        let idx = target.ok_or(Error::CapacityExhausted)?;

        let key_len = u16::try_from(key.len()).map_err(|_| Error::InvalidArgument)?;
        let val_len = u16::try_from(value.len()).map_err(|_| Error::InvalidArgument)?;
        let seq = u32::try_from(self.len + 1).map_err(|_| Error::CapacityExhausted)?;

        let ko = self.arena.alloc(key.len(), 1)?;
        self.arena.write(ko, key)?;
        let vo = self.arena.alloc(value.len(), 1)?;
        self.arena.write(vo, value)?;

        let ko_u32 = u32::try_from(ko).map_err(|_| Error::CapacityExhausted)?;
        let vo_u32 = u32::try_from(vo).map_err(|_| Error::CapacityExhausted)?;

        let addr = self.slot_addr(idx);
        self.arena.write_u32(addr, seq)?;
        self.arena.write_u16(addr + 4, ST_LIVE)?;
        self.arena.write_u16(addr + 6, key_len)?;
        self.arena.write_u16(addr + 8, val_len)?;
        self.arena.write_u32(addr + 10, ko_u32)?;
        self.arena.write_u32(addr + 14, vo_u32)?;
        self.arena.write_u32(addr + 18, version)?;
        self.len += 1;
        Ok(())
    }

    /// 查键。未找到返回 `None`。
    ///
    /// 返回的切片零拷贝，借用期绑定到 `&self`：在持有该切片期间
    /// 不能对表做 `&mut` 操作。
    pub fn get(&self, key: &[u8]) -> Result<Option<&[u8]>, Error> {
        let Some(idx) = self.find(key)? else {
            return Ok(None);
        };
        let addr = self.slot_addr(idx);
        let voff = self.arena.read_u32(addr + 14)? as usize;
        let vlen = self.arena.read_u16(addr + 8)? as usize;
        Ok(Some(self.arena.peek(voff, vlen)?))
    }

    /// 查键的版本号。
    pub fn version(&self, key: &[u8]) -> Result<Option<u32>, Error> {
        let Some(idx) = self.find(key)? else {
            return Ok(None);
        };
        Ok(Some(self.arena.read_u32(self.slot_addr(idx) + 18)?))
    }

    /// 逻辑清空：所有槽的 `seq` 与 `state` 归零。
    ///
    /// arena 中残留的键值字节被浪费，但表结构可用。
    /// M1 无删除、无空间回收——真实回收属于 M2 的 arena 段复用。
    pub fn reset(&mut self) -> Result<(), Error> {
        for i in 0..self.slot_count {
            let addr = self.slot_addr(i);
            self.arena.write_u32(addr, 0)?;
            self.arena.write_u16(addr + 4, ST_EMPTY)?;
        }
        self.len = 0;
        Ok(())
    }

    /// 从 `start_seq` 起扫下一个存活键（seq 严格大于 `start_seq`）。
    ///
    /// 扫描按 **seq 顺序**（插入序）单调推进，而非物理槽下标顺序：
    /// 遍历全部槽，取存活槽中 seq 最小且 `> start_seq` 的那一项。
    /// 这样分页续扫才是确定性的——与键落在哪个桶无关。
    pub fn scan(&self, start_seq: u32) -> Result<Option<ScanEntry<'_>>, Error> {
        let mut best: Option<(u32, usize)> = None;
        for i in 0..self.slot_count {
            let addr = self.slot_addr(i);
            if self.arena.read_u16(addr + 4)? != ST_LIVE {
                continue;
            }
            let seq = self.arena.read_u32(addr)?;
            if seq <= start_seq {
                continue;
            }
            if best.map_or(true, |(best_seq, _)| seq < best_seq) {
                best = Some((seq, addr));
            }
        }
        let Some((seq, addr)) = best else {
            return Ok(None);
        };
        let kl = self.arena.read_u16(addr + 6)? as usize;
        let ko = self.arena.read_u32(addr + 10)? as usize;
        let vl = self.arena.read_u16(addr + 8)? as usize;
        let vo = self.arena.read_u32(addr + 14)? as usize;
        let version = self.arena.read_u32(addr + 18)?;
        Ok(Some(ScanEntry {
            seq,
            key: self.arena.peek(ko, kl)?,
            value: self.arena.peek(vo, vl)?,
            version,
        }))
    }
}

/// `FixedTable::scan` 的返回值。
///
/// 切片借用期由调用方的 `&FixedTable` 约束（`ScanEntry<'_>`）。
/// `seq` 是该键的写入序号，扫描游标用它单调推进。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScanEntry<'a> {
    pub seq: u32,
    pub key: &'a [u8],
    pub value: &'a [u8],
    pub version: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec;
    use std::vec::Vec;

    /// arena 与表共享缓冲，用闭包包一层避免自引用。
    ///
    /// `FixedTable` 持有 `&mut Arena`，因此闭包不能再同时借 arena；
    /// 测试体只通过表访问数据，签名收为单参数。
    fn with_table<F: FnOnce(&mut FixedTable)>(cap: usize, buf_len: usize, f: F) {
        let mut buf = vec![0u8; buf_len];
        let mut arena = Arena::new(&mut buf);
        let mut table = FixedTable::new(&mut arena, cap).unwrap();
        f(&mut table);
    }

    #[test]
    fn 插入后可读回() {
        with_table(16, 4096, |t| {
            t.insert(b"alpha", b"one", 1).unwrap();
            assert_eq!(t.get(b"alpha").unwrap(), Some(&b"one"[..]));
            assert_eq!(t.len(), 1);
        });
    }

    #[test]
    fn 未插入返回空() {
        with_table(16, 4096, |t| {
            assert_eq!(t.get(b"missing").unwrap(), None);
        });
    }

    #[test]
    fn 同键覆盖并更新版本() {
        with_table(16, 4096, |t| {
            t.insert(b"k", b"v1", 1).unwrap();
            t.insert(b"k", b"v1", 2).unwrap();
            assert_eq!(t.get(b"k").unwrap(), Some(&b"v1"[..]));
            assert_eq!(t.version(b"k").unwrap(), Some(2));
            assert_eq!(t.len(), 1);
        });
    }

    #[test]
    fn 同键不同长度值被拒() {
        with_table(16, 4096, |t| {
            t.insert(b"k", b"v1", 1).unwrap();
            // 原地覆盖要求长度一致——避免移动字节导致悬垂
            assert_eq!(
                t.insert(b"k", b"value-too-long", 2),
                Err(Error::InvalidArgument)
            );
        });
    }

    #[test]
    fn 空表构造被拒() {
        let mut buf = vec![0u8; 256];
        let mut arena = Arena::new(&mut buf);
        assert_eq!(
            FixedTable::new(&mut arena, 0).err(),
            Some(Error::InvalidArgument)
        );
    }

    #[test]
    fn 探测超限返回容量不足() {
        // 槽数 1，且 MAX_PROBES 下只允许 1 个键
        with_table(1, 1024, |t| {
            t.insert(b"a", b"1", 1).unwrap();
            assert_eq!(t.insert(b"b", b"2", 1), Err(Error::CapacityExhausted));
        });
    }

    #[test]
    fn 扫描按写入序单调推进() {
        with_table(32, 4096, |t| {
            for (i, k) in [b"a", b"b", b"c"].iter().enumerate() {
                t.insert(*k, &[(b'0' + i as u8)], 1).unwrap();
            }
            let mut seen: Vec<u8> = Vec::new();
            let mut cursor = 0;
            let mut expected_seq = 1u32;
            while let Some(e) = t.scan(cursor).unwrap() {
                assert_eq!(e.key.len(), 1);
                assert_eq!(e.seq, expected_seq, "seq 必须按插入序递增");
                seen.push(e.key[0]);
                cursor = e.seq;
                expected_seq += 1;
            }
            assert_eq!(seen, vec![b'a', b'b', b'c']);
        });
    }

    #[test]
    fn 扫描起点之后无键则为空() {
        with_table(16, 1024, |t| {
            t.insert(b"a", b"1", 1).unwrap();
            assert!(t.scan(1).unwrap().is_none());
        });
    }

    #[test]
    fn 容量与长度可观测() {
        with_table(16, 1024, |t| {
            assert_eq!(t.capacity(), 16);
            assert!(t.is_empty());
            t.insert(b"x", b"y", 1).unwrap();
            assert!(!t.is_empty());
        });
    }
}
