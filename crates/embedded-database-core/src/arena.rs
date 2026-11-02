//! 固定容量 bump 分配器。不分配、不增长、不 panic。
//!
//! 所有分配都来自调用方提供的一块 `&mut [u8]`。耗尽即返回
//! `Error::CapacityExhausted`——**绝不**扩容，容量必须在启动时确定（C7）。

use crate::Error;

/// 固定容量 arena。
///
/// 生命周期与调用方借用的缓冲区绑定，不能比它活得久。
#[derive(Debug)]
pub struct Arena<'a> {
    buf: &'a mut [u8],
    used: usize,
}

impl<'a> Arena<'a> {
    /// 用调用方提供的缓冲区创建 arena。
    pub fn new(buf: &'a mut [u8]) -> Self {
        Arena { buf, used: 0 }
    }

    pub fn capacity(&self) -> usize {
        self.buf.len()
    }

    pub fn used(&self) -> usize {
        self.used
    }

    pub fn remaining(&self) -> usize {
        self.buf.len() - self.used
    }

    /// 分配 `size` 字节并按 `align` 对齐，返回 arena 内偏移。
    ///
    /// 对齐向上取整；若所需空间不足返回 `CapacityExhausted`。
    pub fn alloc(&mut self, size: usize, align: usize) -> Result<usize, Error> {
        if align == 0 || !align.is_power_of_two() {
            return Err(Error::InvalidArgument);
        }
        let base = self
            .used
            .checked_add(align - 1)
            .ok_or(Error::CapacityExhausted)?
            & !(align - 1);
        let end = base.checked_add(size).ok_or(Error::CapacityExhausted)?;
        if end > self.buf.len() {
            return Err(Error::CapacityExhausted);
        }
        self.used = end;
        Ok(base)
    }

    /// 读 `len` 字节，返回借用期绑定到 `&self` 的切片——调用方须立即拷贝。
    pub fn read(&self, off: usize, len: usize) -> Result<&[u8], Error> {
        let end = off.checked_add(len).ok_or(Error::InvalidArgument)?;
        self.buf.get(off..end).ok_or(Error::InvalidArgument)
    }

    /// 零拷贝读，返回借用期绑定到 `&self` 的切片。
    ///
    /// 这是 `FixedTable::get` / `scan` 返回值的读取路径：不拷贝字节，
    /// 返回值在下一次 `&mut` 方法调用前有效。不依赖 `unsafe` 时，
    /// 借用期最多只能绑定到 `&self`（缓冲区的 `'a` 生命期无法在
    /// 持有 `&mut Arena` 的结构体方法里安全地泄露出去，见 C2）。
    pub fn peek(&self, off: usize, len: usize) -> Result<&[u8], Error> {
        self.read(off, len)
    }

    /// 写 `data` 到 `off`。越界返回 `InvalidArgument`，**不 panic**。
    pub fn write(&mut self, off: usize, data: &[u8]) -> Result<(), Error> {
        let end = off.checked_add(data.len()).ok_or(Error::InvalidArgument)?;
        if end > self.buf.len() {
            return Err(Error::InvalidArgument);
        }
        self.buf[off..end].copy_from_slice(data);
        Ok(())
    }

    pub fn read_u16(&self, off: usize) -> Result<u16, Error> {
        let b = self.read(off, 2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    pub fn read_u32(&self, off: usize) -> Result<u32, Error> {
        let b = self.read(off, 4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn write_u16(&mut self, off: usize, v: u16) -> Result<(), Error> {
        self.write(off, &v.to_le_bytes())
    }

    pub fn write_u32(&mut self, off: usize, v: u32) -> Result<(), Error> {
        self.write(off, &v.to_le_bytes())
    }

    /// 归还全部空间。保留容量。
    pub fn clear(&mut self) {
        self.used = 0;
    }

    /// 剩余未分配空间中是否还能容纳 `size` 字节。不改变状态。
    pub fn can_alloc(&self, size: usize, align: usize) -> bool {
        if align == 0 || !align.is_power_of_two() {
            return false;
        }
        let Some(base) = self.used.checked_add(align - 1) else {
            return false;
        };
        let base = base & !(align - 1);
        base.checked_add(size)
            .is_some_and(|end| end <= self.buf.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 分配递增偏移() {
        let mut buf = [0u8; 64];
        let mut a = Arena::new(&mut buf);
        assert_eq!(a.alloc(8, 1).unwrap(), 0);
        assert_eq!(a.alloc(8, 1).unwrap(), 8);
        assert_eq!(a.used(), 16);
        assert_eq!(a.capacity(), 64);
        assert_eq!(a.remaining(), 48);
    }

    #[test]
    fn 对齐到指定边界() {
        let mut buf = [0u8; 64];
        let mut a = Arena::new(&mut buf);
        a.alloc(1, 1).unwrap();
        let off = a.alloc(8, 8).unwrap();
        assert_eq!(off % 8, 0);
    }

    #[test]
    fn 耗尽返回容量不足() {
        let mut buf = [0u8; 16];
        let mut a = Arena::new(&mut buf);
        a.alloc(12, 1).unwrap();
        assert_eq!(a.alloc(8, 1), Err(Error::CapacityExhausted));
    }

    #[test]
    fn 非二次幂对齐被拒() {
        let mut buf = [0u8; 64];
        let mut a = Arena::new(&mut buf);
        assert_eq!(a.alloc(8, 3), Err(Error::InvalidArgument));
        assert_eq!(a.alloc(8, 0), Err(Error::InvalidArgument));
    }

    #[test]
    fn 写读往返() {
        let mut buf = [0u8; 32];
        let mut a = Arena::new(&mut buf);
        a.write(4, b"EmbedDB").unwrap();
        assert_eq!(a.read(4, 7).unwrap(), b"EmbedDB");
    }

    #[test]
    fn 越界读写不panic() {
        let mut buf = [0u8; 16];
        let mut a = Arena::new(&mut buf);
        assert_eq!(a.write(14, b"toolong"), Err(Error::InvalidArgument));
        assert_eq!(a.read(15, 4), Err(Error::InvalidArgument));
    }

    #[test]
    fn 定长整数往返() {
        let mut buf = [0u8; 32];
        let mut a = Arena::new(&mut buf);
        a.write_u16(0, 0xBEEF).unwrap();
        a.write_u32(8, 0xDEAD_BEEF).unwrap();
        assert_eq!(a.read_u16(0).unwrap(), 0xBEEF);
        assert_eq!(a.read_u32(8).unwrap(), 0xDEAD_BEEF);
    }

    #[test]
    fn 清空回收全部空间() {
        let mut buf = [0u8; 16];
        let mut a = Arena::new(&mut buf);
        a.alloc(16, 1).unwrap();
        a.clear();
        assert_eq!(a.used(), 0);
        assert_eq!(a.alloc(16, 1).unwrap(), 0);
    }

    #[test]
    fn 容量探测不改变状态() {
        let mut buf = [0u8; 16];
        let mut a = Arena::new(&mut buf);
        a.alloc(12, 1).unwrap();
        assert!(!a.can_alloc(8, 1));
        assert_eq!(a.used(), 12);
    }

    #[test]
    fn 越界加法不溢出() {
        let mut buf = [0u8; 8];
        let a = Arena::new(&mut buf);
        assert_eq!(a.read(usize::MAX, 2), Err(Error::InvalidArgument));
    }
}
