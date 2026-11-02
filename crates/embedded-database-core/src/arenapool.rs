//! 固定数量 Arena 池。Db 持有池，Txn 按索引访问。

use crate::arena::Arena;
use crate::Error;
use std::vec;
use std::vec::Vec;

/// Arena 池。固定数量、固定容量，启动时一次性预分配。
#[derive(Debug)]
pub struct ArenaPool {
    /// 每个槽的缓冲区。
    bufs: Vec<Vec<u8>>,
    /// 槽是否已被占用。
    used: Vec<bool>,
    arena_capacity: usize,
}

impl ArenaPool {
    /// 创建池：`pool_size` 个槽，每个槽 `arena_capacity` 字节。
    pub fn new(pool_size: usize, arena_capacity: usize) -> Self {
        let mut bufs = Vec::with_capacity(pool_size);
        let mut used = Vec::with_capacity(pool_size);
        for _ in 0..pool_size {
            bufs.push(vec![0u8; arena_capacity]);
            used.push(false);
        }
        ArenaPool {
            bufs,
            used,
            arena_capacity,
        }
    }

    pub fn pool_size(&self) -> usize {
        self.bufs.len()
    }

    pub fn arena_capacity(&self) -> usize {
        self.arena_capacity
    }

    /// 获取一个空闲槽的索引。池满返回 `None`。
    pub fn acquire(&mut self) -> Option<usize> {
        let idx = self.used.iter().position(|u| !u)?;
        self.used[idx] = true;
        Some(idx)
    }

    /// 归还槽。无效索引静默忽略。
    pub fn release(&mut self, idx: usize) {
        if idx < self.used.len() {
            self.used[idx] = false;
        }
    }

    /// 向槽 `idx` 的 arena 偏移 `off` 写数据。
    pub fn write(&mut self, idx: usize, off: usize, data: &[u8]) -> Result<(), Error> {
        let buf = self.bufs.get_mut(idx).ok_or(Error::InvalidArgument)?;
        let mut arena = Arena::new(buf);
        arena.write(off, data)
    }

    /// 从槽 `idx` 的 arena 偏移 `off` 读 `len` 字节。
    pub fn read(&self, idx: usize, off: usize, len: usize) -> Result<&[u8], Error> {
        let buf = self.bufs.get(idx).ok_or(Error::InvalidArgument)?;
        let end = off.checked_add(len).ok_or(Error::InvalidArgument)?;
        buf.get(off..end).ok_or(Error::InvalidArgument)
    }

    /// 对槽 `idx` 的 arena 执行闭包。
    pub fn with_arena<R>(
        &mut self,
        idx: usize,
        f: impl FnOnce(&mut Arena<'_>) -> R,
    ) -> Result<R, Error> {
        let buf = self.bufs.get_mut(idx).ok_or(Error::InvalidArgument)?;
        let mut arena = Arena::new(buf);
        Ok(f(&mut arena))
    }
}

#[cfg(all(test, feature = "std"))]
mod tests {
    use super::*;

    #[test]
    fn 池创建与容量() {
        let pool = ArenaPool::new(2, 4096);
        assert_eq!(pool.pool_size(), 2);
        assert_eq!(pool.arena_capacity(), 4096);
    }

    #[test]
    fn 获取与归还() {
        let mut pool = ArenaPool::new(2, 4096);
        assert!(pool.acquire().is_some());
        assert!(pool.acquire().is_some());
        assert!(pool.acquire().is_none(), "池已满");
        pool.release(0);
        assert!(pool.acquire().is_some(), "归还后可再获取");
    }

    #[test]
    fn 池内缓冲区可用() {
        let mut pool = ArenaPool::new(1, 256);
        let idx = pool.acquire().unwrap();
        pool.write(idx, 0, b"hello").unwrap();
        assert_eq!(pool.read(idx, 0, 5).unwrap(), b"hello");
    }
}
