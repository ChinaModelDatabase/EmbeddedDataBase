//! 范围扫描游标。M3 简化：持有 Vec，std 门控。

/// 扫描游标。通过 `Iterator` 迭代返回 (key, value) 对。
///
/// M3 简化：持有 `Vec`，不支持零拷贝。M4 引入 arena 池后可改为零拷贝。
#[derive(Debug, PartialEq)]
pub struct Cursor<'a> {
    items: std::vec::Vec<(std::vec::Vec<u8>, std::vec::Vec<u8>)>,
    pos: usize,
    _marker: core::marker::PhantomData<&'a ()>,
}

impl<'a> Cursor<'a> {
    pub fn new(items: std::vec::Vec<(std::vec::Vec<u8>, std::vec::Vec<u8>)>) -> Self {
        Cursor {
            items,
            pos: 0,
            _marker: core::marker::PhantomData,
        }
    }

    pub fn pos(&self) -> usize {
        self.pos
    }

    pub fn is_exhausted(&self) -> bool {
        self.pos >= self.items.len()
    }
}

impl<'a> Iterator for Cursor<'a> {
    type Item = (std::vec::Vec<u8>, std::vec::Vec<u8>);

    fn next(&mut self) -> Option<Self::Item> {
        let (k, v) = self.items.get_mut(self.pos)?;
        self.pos += 1;
        Some((core::mem::take(k), core::mem::take(v)))
    }
}

#[cfg(all(test, feature = "std"))]
mod tests {
    use super::*;
    use std::format;
    use std::vec;
    use std::vec::Vec;

    fn make_items(n: usize) -> Vec<(Vec<u8>, Vec<u8>)> {
        (0..n)
            .map(|i| {
                (
                    format!("k{i:04}").into_bytes(),
                    format!("v{i:04}").into_bytes(),
                )
            })
            .collect()
    }

    #[test]
    fn 迭代器按序返回() {
        let c = Cursor::new(make_items(3));
        let items: Vec<(Vec<u8>, Vec<u8>)> = c.into_iter().collect();
        assert_eq!(
            items,
            vec![
                (b"k0000".to_vec(), b"v0000".to_vec()),
                (b"k0001".to_vec(), b"v0001".to_vec()),
                (b"k0002".to_vec(), b"v0002".to_vec()),
            ]
        );
    }

    #[test]
    fn 空游标立即耗尽() {
        let mut c = Cursor::new(Vec::new());
        assert_eq!(c.next(), None);
        assert!(c.is_exhausted());
    }

    #[test]
    fn 位置跟踪() {
        let mut c = Cursor::new(make_items(5));
        assert_eq!(c.pos(), 0);
        c.next();
        c.next();
        assert_eq!(c.pos(), 2);
    }
}
