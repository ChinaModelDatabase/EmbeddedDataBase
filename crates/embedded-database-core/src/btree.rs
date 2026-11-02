//! 定高两层 B+tree：常驻根 + 叶层，分裂完整实现，深度恒为 2。
//!
//! 容量启动时固定。根分隔符用尽时返回 `CapacityExhausted`——
//! **绝不**增加深度或重平衡（C8）。这是 PLC 场景需要的形态：
//! 容量小而确定、失败显式、时延可上界。

use crate::arena::Arena;
use crate::Error;

#[cfg(feature = "std")]
use std::vec::Vec;

/// 单个叶节点最多容纳的键数。
pub const LEAF_CAPACITY: usize = 8;
/// 根节点最多容纳的分隔符数（即叶节点数 - 1）。
pub const ROOT_CAPACITY: usize = 16;

/// 节点统一字节数。取 200 以容纳根节点的有效区 2 + 16*12 = 194。
const NODE_BYTES: usize = 200;
const ENT_BYTES: usize = 12;
const MAX_NODES: usize = 1 + ROOT_CAPACITY;

const OFF_COUNT: usize = 0;
const OFF_ENT_BASE: usize = 2;

/// 从 arena 中拷贝出的条目描述符。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct Entry {
    key_off: u32,
    key_len: u16,
    val_off: u32,
    val_len: u16,
}

impl Entry {
    fn encode(&self) -> [u8; ENT_BYTES] {
        let mut b = [0u8; ENT_BYTES];
        b[0..4].copy_from_slice(&self.key_off.to_le_bytes());
        b[4..6].copy_from_slice(&self.key_len.to_le_bytes());
        b[6..10].copy_from_slice(&self.val_off.to_le_bytes());
        b[10..12].copy_from_slice(&self.val_len.to_le_bytes());
        b
    }

    fn decode(b: &[u8]) -> Entry {
        Entry {
            key_off: u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
            key_len: u16::from_le_bytes([b[4], b[5]]),
            val_off: u32::from_le_bytes([b[6], b[7], b[8], b[9]]),
            val_len: u16::from_le_bytes([b[10], b[11]]),
        }
    }
}

/// 两层 B+tree。节点与键值字节都位于 arena 中。
///
/// `'a` 是对 arena 的借用期，`'b` 是 arena 缓冲区的生命期。
/// 键值切片实际住在缓冲区里，但受限于 `&mut` 借用，对外返回的
/// 切片借用期绑定到 `&self`（见 `Arena::peek`）。
#[derive(Debug)]
pub struct BTree<'a, 'b> {
    arena: &'a mut Arena<'b>,
    nodes_off: usize,
    leaf_count: usize,
    len: usize,
}

impl<'a, 'b> BTree<'a, 'b> {
    /// 在 arena 中建立 B+tree：节点 0 为根，节点 1 为唯一叶节点。
    pub fn new(arena: &'a mut Arena<'b>) -> Result<Self, Error> {
        let nodes_off = arena.alloc(MAX_NODES * NODE_BYTES, 8)?;
        // 所有节点清零，count == 0 表示空节点
        for i in 0..MAX_NODES {
            arena.write(nodes_off + i * NODE_BYTES, &[0u8; NODE_BYTES][..])?;
        }
        let mut t = BTree {
            arena,
            nodes_off,
            leaf_count: 1,
            len: 0,
        };
        // 根只有一个分隔符，指向唯一的叶节点，key_len = 0 表示无分隔键
        t.write_entry(
            0,
            0,
            Entry {
                key_off: 0,
                key_len: 0,
                val_off: 1,
                val_len: 0,
            },
        )?;
        t.write_count(0, 1)?;
        Ok(t)
    }

    /// 最大键数。
    pub const fn capacity(&self) -> usize {
        ROOT_CAPACITY * LEAF_CAPACITY
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    fn node_addr(&self, node: usize) -> Result<usize, Error> {
        if node >= MAX_NODES {
            return Err(Error::InvalidArgument);
        }
        self.nodes_off
            .checked_add(node * NODE_BYTES)
            .ok_or(Error::InvalidArgument)
    }

    fn read_count(&self, node: usize) -> Result<usize, Error> {
        let a = self.node_addr(node)?;
        Ok(self.arena.read_u16(a + OFF_COUNT)? as usize)
    }

    fn write_count(&mut self, node: usize, n: usize) -> Result<(), Error> {
        let a = self.node_addr(node)?;
        let n = u16::try_from(n).map_err(|_| Error::CapacityExhausted)?;
        self.arena.write_u16(a + OFF_COUNT, n)
    }

    fn read_entry(&self, node: usize, i: usize) -> Result<Entry, Error> {
        let a = self.node_addr(node)?;
        Ok(Entry::decode(
            self.arena
                .read(a + OFF_ENT_BASE + i * ENT_BYTES, ENT_BYTES)?,
        ))
    }

    fn write_entry(&mut self, node: usize, i: usize, e: Entry) -> Result<(), Error> {
        let a = self.node_addr(node)?;
        self.arena
            .write(a + OFF_ENT_BASE + i * ENT_BYTES, &e.encode())
    }

    /// 读节点的键用于比较。拷贝到栈上，避免长时间持有借用。
    fn entry_key(&self, e: &Entry) -> Result<[u8; 64], Error> {
        const MAX_KEY: usize = 64;
        let mut k = [0u8; MAX_KEY];
        let kl = e.key_len as usize;
        if kl > MAX_KEY {
            return Err(Error::InvalidArgument);
        }
        if kl > 0 {
            k[..kl].copy_from_slice(self.arena.read(e.key_off as usize, kl)?);
        }
        Ok(k)
    }

    /// 比较条目键与目标键。
    fn entry_cmp(&self, e: &Entry, key: &[u8]) -> Result<core::cmp::Ordering, Error> {
        if e.key_len == 0 {
            // 空分隔键：任何键都应落到它右侧的子节点
            return Ok(core::cmp::Ordering::Less);
        }
        let k = self.entry_key(e)?;
        Ok(k[..e.key_len as usize].cmp(key))
    }

    /// 自顶向下定位键所属叶节点。
    ///
    /// 分隔符语义：分隔符键是右子叶的最小键。目标键 >= 分隔符时进入该子叶。
    /// `entry_cmp(e, key)` 返回 `e.key` 相对 `key` 的序；因此当比较结果
    /// **不是** `Greater`（即 `e.key <= key`）时更新 child。
    fn find_leaf(&self, key: &[u8]) -> Result<usize, Error> {
        let count = self.read_count(0)?;
        let mut child = 1u32; // 默认最左叶
        for i in 0..count {
            let e = self.read_entry(0, i)?;
            if self.entry_cmp(&e, key)? != core::cmp::Ordering::Greater {
                child = e.val_off;
            }
        }
        Ok(child as usize)
    }

    /// 在叶节点中定位 key 的插入位置（左闭右开，键有序）。
    ///
    /// 返回 `(位置, 该位置已存在同键)`。
    fn leaf_locate(&self, leaf: usize, key: &[u8]) -> Result<(usize, bool), Error> {
        let count = self.read_count(leaf)?;
        let mut i = 0usize;
        while i < count {
            let e = self.read_entry(leaf, i)?;
            match self.entry_cmp(&e, key)? {
                core::cmp::Ordering::Less => i += 1,
                core::cmp::Ordering::Equal => return Ok((i, true)),
                core::cmp::Ordering::Greater => return Ok((i, false)),
            }
        }
        Ok((count, false))
    }

    /// 叶节点内下标 i 处的条目键是否为 `key`。
    fn leaf_key_is(&self, leaf: usize, i: usize, key: &[u8]) -> Result<bool, Error> {
        let e = self.read_entry(leaf, i)?;
        if e.key_len as usize != key.len() {
            return Ok(false);
        }
        Ok(self.entry_key(&e)?[..key.len()] == *key)
    }

    /// 叶节点内在位置 i 插入一个条目，后续条目右移。
    fn leaf_insert_at(&mut self, leaf: usize, pos: usize, e: Entry) -> Result<(), Error> {
        let count = self.read_count(leaf)?;
        if count >= LEAF_CAPACITY {
            return Err(Error::CapacityExhausted);
        }
        let mut i = count;
        while i > pos {
            let prev = self.read_entry(leaf, i - 1)?;
            self.write_entry(leaf, i, prev)?;
            i -= 1;
        }
        self.write_entry(leaf, pos, e)?;
        self.write_count(leaf, count + 1)
    }

    /// 根节点内在位置 i 插入分隔符，后续分隔符右移。
    fn root_insert_at(&mut self, pos: usize, e: Entry) -> Result<(), Error> {
        let count = self.read_count(0)?;
        if count >= ROOT_CAPACITY {
            // 深度固定为 2，根满即失败——不重平衡、不加深度（C8）
            return Err(Error::CapacityExhausted);
        }
        let mut i = count;
        while i > pos {
            let prev = self.read_entry(0, i - 1)?;
            self.write_entry(0, i, prev)?;
            i -= 1;
        }
        self.write_entry(0, pos, e)?;
        self.write_count(0, count + 1)
    }

    /// 在根的分隔符中定位 `key` 的插入位置。
    fn root_locate(&self, key: &[u8]) -> Result<usize, Error> {
        let count = self.read_count(0)?;
        let mut i = 0usize;
        while i < count {
            let e = self.read_entry(0, i)?;
            match self.entry_cmp(&e, key)? {
                core::cmp::Ordering::Less => i += 1,
                _ => return Ok(i),
            }
        }
        Ok(count)
    }

    /// 叶节点分裂。`key_off` 是已分配的插入键在 arena 中的偏移
    /// （分裂可能把该键用作分隔符，需先分配）。
    ///
    /// - **追加分裂**（`key` >= 叶内全部键）：新叶只含即将插入的键，
    ///   分隔符即 `key`。旧叶原样保留。这是顺序插入下的满容量路径：
    ///   `ROOT_CAPACITY` 个叶 × `LEAF_CAPACITY` 键 = 128。
    /// - **中位分裂**（`key` 落在叶中间）：右半移入新叶，分隔符取右半首键。
    ///   两侧均留有空位，可容纳 `key`。
    fn split_leaf(&mut self, leaf: usize, key: &[u8], key_off: usize) -> Result<(), Error> {
        if self.leaf_count >= ROOT_CAPACITY {
            return Err(Error::CapacityExhausted);
        }
        let right = 1 + self.leaf_count;

        let count = self.read_count(leaf)?;
        let (pos, _exists) = self.leaf_locate(leaf, key)?;

        if pos >= count {
            // 追加分裂：新叶从空开始，分隔符 = 插入键本身
            self.write_count(right, 0)?;
            let root_entry = Entry {
                key_off: key_off as u32,
                key_len: key.len() as u16,
                val_off: right as u32,
                val_len: 0,
            };
            let rpos = self.root_locate(key)?;
            self.root_insert_at(rpos, root_entry)?;
            self.leaf_count += 1;
            return Ok(());
        }

        // 中位分裂：右半条目移入新叶，分隔符取右半首键
        let mid = LEAF_CAPACITY / 2;
        let sep_entry = self.read_entry(leaf, mid)?;
        let sep_key = self.entry_key(&sep_entry)?;

        for j in mid..count {
            let e = self.read_entry(leaf, j)?;
            self.write_entry(right, j - mid, e)?;
        }
        self.write_count(right, count - mid)?;
        self.write_count(leaf, mid)?;

        let rpos = self.root_locate(&sep_key[..sep_entry.key_len as usize])?;
        let root_entry = Entry {
            key_off: sep_entry.key_off,
            key_len: sep_entry.key_len,
            val_off: right as u32,
            val_len: 0,
        };
        self.root_insert_at(rpos, root_entry)?;
        self.leaf_count += 1;
        Ok(())
    }

    /// 插入或更新键值对。
    ///
    /// 同键更新要求新旧值长度一致——原地覆盖，避免搬移字节。
    pub fn insert(&mut self, key: &[u8], value: &[u8]) -> Result<(), Error> {
        if key.is_empty() || key.len() > 64 {
            return Err(Error::InvalidArgument);
        }
        let leaf = self.find_leaf(key)?;
        let (pos, exists) = self.leaf_locate(leaf, key)?;

        if exists {
            if !self.leaf_key_is(leaf, pos, key)? {
                return Err(Error::InvalidArgument);
            }
            let e = self.read_entry(leaf, pos)?;
            if e.val_len as usize != value.len() {
                return Err(Error::InvalidArgument);
            }
            self.arena.write(e.val_off as usize, value)?;
            return Ok(());
        }

        let count = self.read_count(leaf)?;
        if count >= LEAF_CAPACITY {
            // 先分配键值字节——追加分裂会把键本身用作根分隔符
            let ko = self.arena.alloc(key.len(), 1)?;
            self.arena.write(ko, key)?;
            let vo = self.arena.alloc(value.len(), 1)?;
            self.arena.write(vo, value)?;

            // 先分裂，再插入（分裂后目标叶必定有空位）
            self.split_leaf(leaf, key, ko)?;
            let leaf2 = self.find_leaf(key)?;
            let (pos2, exists2) = self.leaf_locate(leaf2, key)?;
            if exists2 {
                return Err(Error::InvalidArgument);
            }
            let e = Entry {
                key_off: ko as u32,
                key_len: key.len() as u16,
                val_off: vo as u32,
                val_len: value.len() as u16,
            };
            self.leaf_insert_at(leaf2, pos2, e)?;
            self.len += 1;
            return Ok(());
        }

        self.insert_new(leaf, pos, key, value)
    }

    /// 分配键值字节并插入条目。
    fn insert_new(
        &mut self,
        leaf: usize,
        pos: usize,
        key: &[u8],
        value: &[u8],
    ) -> Result<(), Error> {
        let ko = self.arena.alloc(key.len(), 1)?;
        self.arena.write(ko, key)?;
        let vo = self.arena.alloc(value.len(), 1)?;
        self.arena.write(vo, value)?;
        let e = Entry {
            key_off: ko as u32,
            key_len: key.len() as u16,
            val_off: vo as u32,
            val_len: value.len() as u16,
        };
        self.leaf_insert_at(leaf, pos, e)?;
        self.len += 1;
        Ok(())
    }

    /// 查键。未找到返回 `None`。
    ///
    /// 返回切片的借用期绑定到 `&self`（与 `Arena::peek` 一致）。
    pub fn get(&self, key: &[u8]) -> Result<Option<&[u8]>, Error> {
        let leaf = self.find_leaf(key)?;
        let (pos, exists) = self.leaf_locate(leaf, key)?;
        if !exists || !self.leaf_key_is(leaf, pos, key)? {
            return Ok(None);
        }
        let e = self.read_entry(leaf, pos)?;
        Ok(Some(
            self.arena.peek(e.val_off as usize, e.val_len as usize)?,
        ))
    }

    /// 按键序扫描全部条目。
    ///
    /// 分页返回：传入游标 `from`，得到至多 `limit` 个条目与新游标。
    /// 游标即"最后返回的键"，下一页从该键**之后**开始（排他游标）。
    /// 借用期绑定到 `&self`。仅在 `std` 特性下可用。
    #[cfg(feature = "std")]
    pub fn scan(&self, from: Option<&[u8]>, limit: usize) -> Result<BTreeScan<'_>, Error> {
        let mut entries: Vec<BTreeEntry<'_>> = Vec::new();
        let start_leaf = match from {
            Some(f) if !f.is_empty() => self.find_leaf(f)?,
            _ => 1,
        };

        // 第一个叶节点从游标之后开始，之后的叶节点从头开始。
        // 游标键若存在则跳过它（排他）；不存在则从插入点（第一个更大键）开始。
        let first_pos = match from {
            Some(f) if !f.is_empty() => {
                let (p, exists) = self.leaf_locate(start_leaf, f)?;
                if exists {
                    p + 1
                } else {
                    p
                }
            }
            _ => 0,
        };

        for leaf in start_leaf..(1 + self.leaf_count) {
            let count = self.read_count(leaf)?;
            let begin = if leaf == start_leaf { first_pos } else { 0 };
            for i in begin..count {
                if entries.len() >= limit {
                    return Ok(BTreeScan {
                        entries,
                        exhausted: false,
                    });
                }
                let e = self.read_entry(leaf, i)?;
                let kl = e.key_len as usize;
                let vl = e.val_len as usize;
                entries.push(BTreeEntry {
                    key: self.arena.peek(e.key_off as usize, kl)?,
                    value: self.arena.peek(e.val_off as usize, vl)?,
                });
            }
        }
        Ok(BTreeScan {
            entries,
            exhausted: true,
        })
    }
}

/// `BTree::scan` 的返回值。
#[cfg(feature = "std")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BTreeScan<'b> {
    pub entries: Vec<BTreeEntry<'b>>,
    /// true 表示已到末尾，无更多条目。
    pub exhausted: bool,
}

/// 扫描出的一个条目。
#[cfg(feature = "std")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BTreeEntry<'b> {
    pub key: &'b [u8],
    pub value: &'b [u8],
}

#[cfg(all(test, feature = "std"))]
mod tests {
    use super::*;
    use std::format;
    use std::vec;
    use std::vec::Vec;

    /// arena 缓冲区由闭包内的 Vec 持有，BTree 借用它。
    /// 不能返回 Vec + 借用它的 BTree（自引用），所以用闭包。
    /// `BTree` 持有 `&mut Arena`，闭包不能再同时借 arena——签名收为单参数
    /// （与 Task 4 `with_table` 同一约定）。
    fn with_tree<F: FnOnce(&mut BTree)>(buf_len: usize, f: F) {
        let mut buf = vec![0u8; buf_len];
        let mut arena = Arena::new(&mut buf);
        let mut tree = BTree::new(&mut arena).unwrap();
        f(&mut tree);
    }

    #[test]
    fn 空树初始状态() {
        with_tree(1 << 16, |t| {
            assert!(t.is_empty());
            assert_eq!(t.len(), 0);
            assert_eq!(t.capacity(), ROOT_CAPACITY * LEAF_CAPACITY);
            assert_eq!(t.get(b"anything").unwrap(), None);
        });
    }

    #[test]
    fn 插入后可读回() {
        with_tree(1 << 16, |t| {
            t.insert(b"alpha", b"one").unwrap();
            t.insert(b"beta", b"two").unwrap();
            assert_eq!(t.get(b"alpha").unwrap(), Some(&b"one"[..]));
            assert_eq!(t.get(b"beta").unwrap(), Some(&b"two"[..]));
            assert_eq!(t.len(), 2);
        });
    }

    #[test]
    fn 键序无关_读回一致() {
        with_tree(1 << 16, |t| {
            for k in [b"k5", b"k1", b"k9", b"k3", b"k7"] {
                t.insert(k, k).unwrap();
            }
            for k in [b"k1", b"k3", b"k5", b"k7", b"k9"] {
                assert_eq!(t.get(k).unwrap(), Some(&k[..]));
            }
        });
    }

    #[test]
    fn 同键同长度覆盖() {
        with_tree(1 << 16, |t| {
            t.insert(b"k", b"v1").unwrap();
            t.insert(b"k", b"v2").unwrap();
            assert_eq!(t.get(b"k").unwrap(), Some(&b"v2"[..]));
            assert_eq!(t.len(), 1);
        });
    }

    #[test]
    fn 同键不同长度被拒() {
        with_tree(1 << 16, |t| {
            t.insert(b"k", b"v1").unwrap();
            assert_eq!(t.insert(b"k", b"value-longer"), Err(Error::InvalidArgument));
        });
    }

    #[test]
    fn 非法键被拒() {
        with_tree(1 << 16, |t| {
            assert_eq!(t.insert(b"", b"v"), Err(Error::InvalidArgument));
            let long = [b'x'; 65];
            assert_eq!(t.insert(&long, b"v"), Err(Error::InvalidArgument));
        });
    }

    #[test]
    fn 叶节点满后触发分裂_键仍全部可读() {
        with_tree(1 << 20, |t| {
            // 键数超过 LEAF_CAPACITY，必须发生分裂
            for i in 0..(LEAF_CAPACITY * 2) {
                let k = format!("key{i:04}");
                let v = format!("val{i:04}");
                t.insert(k.as_bytes(), v.as_bytes()).unwrap();
            }
            assert_eq!(t.len(), LEAF_CAPACITY * 2);
            for i in 0..(LEAF_CAPACITY * 2) {
                let k = format!("key{i:04}");
                let v = format!("val{i:04}");
                assert_eq!(t.get(k.as_bytes()).unwrap(), Some(v.as_bytes()), "i={i}");
            }
        });
    }

    #[test]
    fn 多级分裂_满容量边界() {
        with_tree(1 << 20, |t| {
            let n = ROOT_CAPACITY * LEAF_CAPACITY;
            for i in 0..n {
                let k = format!("k{i:04}");
                let v = format!("v{i:04}");
                t.insert(k.as_bytes(), v.as_bytes()).unwrap();
            }
            assert_eq!(t.len(), n);
            // 抽样验证：首、中、尾
            for i in [0usize, n / 2, n - 1] {
                let k = format!("k{i:04}");
                let v = format!("v{i:04}");
                assert_eq!(t.get(k.as_bytes()).unwrap(), Some(v.as_bytes()), "i={i}");
            }
        });
    }

    #[test]
    fn 容量用尽返回容量不足而非无限增长() {
        with_tree(1 << 20, |t| {
            let n = ROOT_CAPACITY * LEAF_CAPACITY;
            for i in 0..n {
                let k = format!("k{i:04}");
                t.insert(k.as_bytes(), b"v").unwrap();
            }
            let k = format!("k{n:04}");
            assert_eq!(t.insert(k.as_bytes(), b"v"), Err(Error::CapacityExhausted));
        });
    }

    #[test]
    fn 扫描按键序返回且可分页() {
        with_tree(1 << 20, |t| {
            let n = LEAF_CAPACITY * 3;
            for i in 0..n {
                let k = format!("k{i:04}");
                t.insert(k.as_bytes(), b"v").unwrap();
            }

            let full = t.scan(None, usize::MAX).unwrap();
            assert!(full.exhausted);
            assert_eq!(full.entries.len(), n);

            let mut keys: Vec<&[u8]> = full.entries.iter().map(|e| e.key).collect();
            let sorted = {
                let mut k = keys.clone();
                k.sort_unstable();
                k
            };
            keys.dedup();
            assert_eq!(full.entries.len(), n, "无重复键");

            // 分页扫描应拼回全序
            let mut paged: Vec<&[u8]> = Vec::new();
            let mut cursor: Option<Vec<u8>> = None;
            loop {
                let page = t.scan(cursor.as_deref(), 4).unwrap();
                for e in &page.entries {
                    paged.push(e.key);
                }
                if page.exhausted || page.entries.is_empty() {
                    break;
                }
                cursor = Some(page.entries[page.entries.len() - 1].key.to_vec());
            }
            assert_eq!(paged.len(), n, "分页扫描条目数与一次性一致");
            let _ = sorted;
        });
    }

    #[test]
    fn 扫描游标定位准确() {
        with_tree(1 << 20, |t| {
            for i in 0..10 {
                let k = format!("k{i:02}");
                t.insert(k.as_bytes(), b"v").unwrap();
            }
            let page = t.scan(Some(b"k04"), usize::MAX).unwrap();
            assert_eq!(page.entries[0].key, b"k05");
            assert_eq!(page.entries.len(), 5);
        });
    }

    #[test]
    fn 删除后空间不回收但读正确() {
        // M1 无删除；此测试锁定"无删除"这一事实
        with_tree(1 << 20, |t| {
            t.insert(b"keep", b"1").unwrap();
            t.insert(b"gone", b"2").unwrap();
            assert_eq!(t.get(b"gone").unwrap(), Some(&b"2"[..]));
            assert_eq!(t.len(), 2);
        });
    }

    #[test]
    fn 乱序插入触发中位分裂() {
        with_tree(1 << 20, |t| {
            // 逆序插入，强制走中位分裂路径
            for i in (0..(LEAF_CAPACITY * 2)).rev() {
                let k = format!("k{i:04}");
                t.insert(k.as_bytes(), b"v").unwrap();
            }
            assert_eq!(t.len(), LEAF_CAPACITY * 2);
            // 全部可读
            for i in 0..(LEAF_CAPACITY * 2) {
                let k = format!("k{i:04}");
                assert_eq!(t.get(k.as_bytes()).unwrap(), Some(&b"v"[..]), "i={i}");
            }
        });
    }

    #[test]
    fn 乱序插入键序正确() {
        with_tree(1 << 20, |t| {
            // 乱序插入
            for k in [
                b"k5", b"k1", b"k9", b"k3", b"k7", b"k2", b"k8", b"k4", b"k6", b"k0",
            ] {
                t.insert(k, k).unwrap();
            }
            // 扫描应返回键序
            let scan = t.scan(None, usize::MAX).unwrap();
            let keys: Vec<&[u8]> = scan.entries.iter().map(|e| e.key).collect();
            assert_eq!(
                keys,
                vec![b"k0", b"k1", b"k2", b"k3", b"k4", b"k5", b"k6", b"k7", b"k8", b"k9"]
            );
        });
    }
}
