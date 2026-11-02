//! 页槽分配器：管理逻辑页到物理槽组的映射。
//!
//! 一个逻辑页最多持有 `slots_per_page` 个物理槽；用尽后冻结，
//! 后续更新必须走溢出段（spec §4.2）。冻结保证定位 O(1)——
//! 不需要查链表，WCET 可上界。
//!
//! M12：page_id 为全局递增计数器（M8-3），取值可超过 `max_pages`。
//! 分配器改为槽位自由表：`max_pages` 约束的是**段内唯一逻辑页数**，
//! 而非 page_id 取值范围。page_id → 槽下标通过线性扫描映射
//!（段内页数 ≤ max_pages，上界有界，O(max_pages) 可接受）。

use crate::segment::{PageRun, SLOT_FLAG_FROZEN};
use crate::Error;
use std::vec;
use std::vec::Vec;

/// 一次槽分配的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlotAllocation {
    pub page_id: u64,
    /// 该槽组内的槽下标。
    pub slot_index: u16,
    /// 该槽组起始偏移（相对段起点）。
    pub run_offset: u64,
}

/// 槽位条目：一个物理槽组的状态。
#[derive(Debug, Clone, Copy)]
struct SlotEntry {
    /// 该槽组对应的逻辑页号。`None` 表示空闲槽位。
    page_id: Option<u64>,
    /// 已用槽数。
    used: u16,
    /// 槽组起始偏移（相对段起点）。
    run_offset: u64,
    /// 冻结标志。
    frozen: bool,
}

impl SlotEntry {
    fn free() -> Self {
        SlotEntry {
            page_id: None,
            used: 0,
            run_offset: 0,
            frozen: false,
        }
    }
}

/// 页槽分配器。容量固定，段内唯一页数有上界。
#[derive(Debug, Clone)]
pub struct SlotAllocator {
    page_size: u32,
    slots_per_page: u16,
    max_pages: usize,
    /// 槽位自由表：下标 = 槽下标，长度 = max_pages。
    slots: Vec<SlotEntry>,
}

impl SlotAllocator {
    /// `page_size` 与 `slots_per_page` 必须让单个槽组不超过段大小。
    pub fn new(page_size: u32, slots_per_page: u16, max_pages: usize) -> Result<Self, Error> {
        if page_size == 0 || slots_per_page == 0 || max_pages == 0 {
            return Err(Error::InvalidArgument);
        }
        Ok(SlotAllocator {
            page_size,
            slots_per_page,
            max_pages,
            slots: vec![SlotEntry::free(); max_pages],
        })
    }

    pub fn page_size(&self) -> u32 {
        self.page_size
    }

    pub fn slots_per_page(&self) -> u16 {
        self.slots_per_page
    }

    pub fn slot_size(&self) -> u32 {
        self.page_size
    }

    pub fn max_pages(&self) -> usize {
        self.max_pages
    }

    /// 段内已分配的唯一逻辑页数。
    pub fn committed_page_count(&self) -> usize {
        self.slots.iter().filter(|e| e.page_id.is_some()).count()
    }

    /// 按 page_id 查找槽下标。
    fn find_page(&self, page_id: u64) -> Option<usize> {
        self.slots.iter().position(|e| e.page_id == Some(page_id))
    }

    /// 找到第一个空闲槽下标。
    fn find_free(&self) -> Option<usize> {
        self.slots.iter().position(|e| e.page_id.is_none())
    }

    /// 为 `page_id` 分配一个槽。
    ///
    /// 首次分配该页时占用一个空闲槽位；之后依次使用组内后续槽。
    /// 槽用尽返回 `SlotFrozen`。段内唯一页数达到 `max_pages` 返回 `CapacityExhausted`。
    pub fn allocate(&mut self, page_id: u64) -> Result<SlotAllocation, Error> {
        if let Some(idx) = self.find_page(page_id) {
            if self.slots[idx].frozen {
                return Err(Error::SlotFrozen { page_id });
            }
            if self.slots[idx].used >= self.slots_per_page {
                self.slots[idx].frozen = true;
                return Err(Error::SlotFrozen { page_id });
            }
            let slot_index = self.slots[idx].used;
            self.slots[idx].used += 1;
            return Ok(SlotAllocation {
                page_id,
                slot_index,
                run_offset: self.slots[idx].run_offset,
            });
        }
        let idx = self.find_free().ok_or(Error::CapacityExhausted)?;
        self.slots[idx] = SlotEntry {
            page_id: Some(page_id),
            used: 1,
            run_offset: 0,
            frozen: false,
        };
        Ok(SlotAllocation {
            page_id,
            slot_index: 0,
            run_offset: 0,
        })
    }

    /// 设置页的槽组起始偏移。首次分配后调用。
    pub fn set_run_offset(&mut self, page_id: u64, offset: u64) -> Result<(), Error> {
        if let Some(idx) = self.find_page(page_id) {
            if self.slots[idx].run_offset == 0 && self.slots[idx].used == 1 {
                self.slots[idx].run_offset = offset;
            }
            Ok(())
        } else {
            Err(Error::InvalidArgument)
        }
    }

    /// 冻结页的槽组。显式冻结，不依赖"分配失败才冻结"。
    pub fn freeze(&mut self, page_id: u64) -> Result<(), Error> {
        if let Some(idx) = self.find_page(page_id) {
            self.slots[idx].frozen = true;
            Ok(())
        } else {
            Err(Error::InvalidArgument)
        }
    }

    pub fn is_frozen(&self, page_id: u64) -> bool {
        self.find_page(page_id)
            .is_some_and(|i| self.slots[i].frozen)
    }

    /// 某页已用槽数。页不存在返回 0（未分配 = 0 槽）。
    pub fn used_slots(&self, page_id: u64) -> Result<u16, Error> {
        Ok(self
            .find_page(page_id)
            .map(|i| self.slots[i].used)
            .unwrap_or(0))
    }

    /// 回滚到上一个已提交边界。`committed` 按**槽下标**给出每槽已提交槽数；
    /// 只回滚切片覆盖到的槽，其余槽状态不变。若某槽回滚后槽数少于
    /// `slots_per_page`，同时解冻。
    pub fn release_uncommitted(&mut self, committed: &[u16]) -> Result<(), Error> {
        if committed.len() > self.max_pages {
            return Err(Error::InvalidArgument);
        }
        for (idx, &c) in committed.iter().enumerate() {
            self.slots[idx].used = c;
            if self.slots[idx].frozen && c < self.slots_per_page {
                self.slots[idx].frozen = false;
            }
        }
        Ok(())
    }

    /// 导出某页的 PageRun，供写入段尾槽表。
    pub fn page_run(&self, page_id: u64) -> Result<PageRun, Error> {
        if let Some(idx) = self.find_page(page_id) {
            Ok(PageRun::new(
                page_id,
                self.slots[idx].run_offset,
                self.slots[idx].used,
                if self.slots[idx].frozen {
                    SLOT_FLAG_FROZEN
                } else {
                    0
                },
            ))
        } else {
            Err(Error::InvalidArgument)
        }
    }

    /// 遍历段内已分配页的 PageRun 描述（按槽下标序）。
    pub fn allocated_page_runs(&self) -> Vec<PageRun> {
        self.slots
            .iter()
            .filter_map(|e| {
                e.page_id.map(|pid| {
                    PageRun::new(
                        pid,
                        e.run_offset,
                        e.used,
                        if e.frozen { SLOT_FLAG_FROZEN } else { 0 },
                    )
                })
            })
            .collect()
    }
}

#[cfg(all(test, feature = "std"))]
mod tests {
    use super::*;

    #[test]
    fn 首次分配返回槽零() {
        let mut a = SlotAllocator::new(4096, 4, 16).unwrap();
        let alloc = a.allocate(1).unwrap();
        assert_eq!(alloc.slot_index, 0);
        assert_eq!(alloc.page_id, 1);
    }

    #[test]
    fn 连续分配递增槽下标() {
        let mut a = SlotAllocator::new(4096, 4, 16).unwrap();
        for i in 0..4 {
            assert_eq!(a.allocate(1).unwrap().slot_index, i as u16);
        }
    }

    #[test]
    fn 槽用尽后冻结() {
        let mut a = SlotAllocator::new(4096, 2, 16).unwrap();
        a.allocate(1).unwrap();
        a.allocate(1).unwrap();
        assert_eq!(a.allocate(1), Err(Error::SlotFrozen { page_id: 1 }));
        assert!(a.is_frozen(1));
    }

    #[test]
    fn 显式冻结立即生效() {
        let mut a = SlotAllocator::new(4096, 8, 16).unwrap();
        a.allocate(2).unwrap();
        a.freeze(2).unwrap();
        assert!(a.is_frozen(2));
        assert_eq!(a.allocate(2), Err(Error::SlotFrozen { page_id: 2 }));
    }

    #[test]
    fn 页数超限返回容量不足() {
        let mut a = SlotAllocator::new(4096, 4, 2).unwrap();
        a.allocate(0).unwrap();
        a.allocate(1).unwrap();
        assert_eq!(a.allocate(2), Err(Error::CapacityExhausted));
    }

    #[test]
    fn 回滚撤销未提交槽() {
        let mut a = SlotAllocator::new(4096, 4, 16).unwrap();
        a.allocate(1).unwrap();
        a.allocate(1).unwrap();
        a.allocate(1).unwrap();
        assert_eq!(a.used_slots(1).unwrap(), 3);
        // free-list：page 1 占用 slot 0；回滚按槽下标
        // slot 0 已提交 1 槽 → used=1
        a.release_uncommitted(&[1]).unwrap();
        assert_eq!(a.used_slots(1).unwrap(), 1);
        // 回滚后可以继续分配
        assert_eq!(a.allocate(1).unwrap().slot_index, 1);
    }

    #[test]
    fn 回滚解冻已冻结页() {
        let mut a = SlotAllocator::new(4096, 1, 16).unwrap();
        a.allocate(3).unwrap();
        assert_eq!(a.allocate(3), Err(Error::SlotFrozen { page_id: 3 }));
        // 4 个 0 覆盖 idx 0..=3，其中 idx 0（page 3 所在槽）的 used=0 触发解冻
        a.release_uncommitted(&[0, 0, 0, 0]).unwrap();
        assert!(!a.is_frozen(3));
    }

    #[test]
    fn 导出页槽组描述() {
        let mut a = SlotAllocator::new(4096, 4, 16).unwrap();
        a.allocate(5).unwrap();
        a.allocate(5).unwrap();
        let run = a.page_run(5).unwrap();
        assert_eq!(run.page_id, 5);
        assert_eq!(run.next_slot, 2);
        assert!(!run.is_frozen());
    }

    #[test]
    fn 非法参数被拒() {
        assert!(SlotAllocator::new(0, 4, 16).is_err());
        assert!(SlotAllocator::new(4096, 0, 16).is_err());
        assert!(SlotAllocator::new(4096, 4, 0).is_err());
    }

    #[test]
    fn 全局page_id可超过max_pages() {
        // M12：page_id 为全局递增计数器，段内唯一页数受 max_pages 约束，
        // page_id 取值不受限。
        let mut a = SlotAllocator::new(4096, 4, 2).unwrap();
        a.allocate(100).unwrap();
        a.allocate(200).unwrap();
        assert_eq!(a.allocate(300), Err(Error::CapacityExhausted));
        assert_eq!(a.used_slots(100).unwrap(), 1);
        assert_eq!(a.committed_page_count(), 2);
    }
}
