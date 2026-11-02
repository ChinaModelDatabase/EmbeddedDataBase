//! 内存 NOR 闪存模拟设备：字节级 1→0 编程、整块擦除、坏块与故障注入。

use alloc::vec::Vec;

use crate::raw::{FlashError, FlashResult, RawFlash};

const ERASED: u8 = 0xFF;

/// 内存 NOR 闪存。
///
/// 用于 FTL 的功能与故障注入测试：可标记坏块、可在第 N 次编程/擦除后
/// 开始持续失败（模拟磨损耗尽或断电后的介质异常）。
#[derive(Debug, Clone)]
pub struct SimFlash {
    page_size: usize,
    erase_block_size: usize,
    bytes: Vec<u8>,
    bad: Vec<bool>,
    erase_counts: Vec<u32>,
    program_ops: u64,
    fail_program_after: Option<u64>,
    fail_erase_after: Option<u64>,
}

impl SimFlash {
    /// 创建 `block_count` 个全 `0xFF` 的擦除块。
    pub fn new(block_count: u32, page_size: usize, erase_block_size: usize) -> Self {
        let total = block_count as usize * erase_block_size;
        SimFlash {
            page_size,
            erase_block_size,
            bytes: alloc::vec![ERASED; total],
            bad: alloc::vec![false; block_count as usize],
            erase_counts: alloc::vec![0u32; block_count as usize],
            program_ops: 0,
            fail_program_after: None,
            fail_erase_after: None,
        }
    }

    /// 标记物理块为坏块。
    pub fn mark_bad(&mut self, block: u32) {
        if let Some(slot) = self.bad.get_mut(block as usize) {
            *slot = true;
        }
        if let Some(slot) = self.erase_counts.get_mut(block as usize) {
            *slot = 0;
        }
    }

    /// 该块是否坏。
    pub fn is_bad(&self, block: u32) -> bool {
        self.bad.get(block as usize).copied().unwrap_or(true)
    }

    /// 从第 `after` 次编程起，后续所有编程都返回 `Program` 错误。
    pub fn inject_program_failure(&mut self, after: u64) {
        self.fail_program_after = Some(after);
    }

    /// 从第 `after` 次擦除起，后续擦除都返回 `Erase`。
    pub fn inject_erase_failure(&mut self, after: u64) {
        self.fail_erase_after = Some(after);
    }

    /// 累计成功编程次数（含被拒绝前已计入的尝试不在此列）。
    pub fn program_count(&self) -> u64 {
        self.program_ops
    }

    /// 某个物理块被成功擦除的次数。
    pub fn erase_count_of(&self, block: u32) -> u32 {
        self.erase_counts.get(block as usize).copied().unwrap_or(0)
    }

    /// 把整片介质快照拷进 `out`（不足部分不写）。用于掉电复验。
    pub fn snapshot_into(&self, out: &mut [u8]) {
        let n = out.len().min(self.bytes.len());
        out[..n].copy_from_slice(&self.bytes[..n]);
    }

    fn base(&self, block: u32) -> usize {
        block as usize * self.erase_block_size
    }

    fn check(&self, block: u32, offset: usize, len: usize) -> FlashResult<()> {
        if block as usize >= self.bad.len() {
            return Err(FlashError::OutOfRange);
        }
        if self.bad[block as usize] {
            return Err(FlashError::BadBlock { block });
        }
        if offset > self.erase_block_size || len > self.erase_block_size - offset {
            return Err(FlashError::OutOfRange);
        }
        Ok(())
    }
}

impl RawFlash for SimFlash {
    fn page_size(&self) -> usize {
        self.page_size
    }

    fn erase_block_size(&self) -> usize {
        self.erase_block_size
    }

    fn block_count(&self) -> u32 {
        self.bad.len() as u32
    }

    fn read(&self, block: u32, offset: usize, buf: &mut [u8]) -> FlashResult<()> {
        self.check(block, offset, buf.len())?;
        let at = self.base(block) + offset;
        buf.copy_from_slice(&self.bytes[at..at + buf.len()]);
        Ok(())
    }

    fn program(&mut self, block: u32, offset: usize, data: &[u8]) -> FlashResult<()> {
        self.check(block, offset, data.len())?;
        if data.is_empty() {
            return Ok(());
        }
        // 页边界检查：首尾落在同一页内。
        let first = offset / self.page_size;
        let last = (offset + data.len() - 1) / self.page_size;
        if first != last {
            return Err(FlashError::Unaligned);
        }
        if let Some(limit) = self.fail_program_after {
            if self.program_ops >= limit {
                return Err(FlashError::Program { block });
            }
        }
        let at = self.base(block) + offset;
        // NOR 语义：先整体校验，任一字节需要 0→1 就整体拒绝。
        for (i, &byte) in data.iter().enumerate() {
            if self.bytes[at + i] & byte != byte {
                return Err(FlashError::Program { block });
            }
        }
        self.bytes[at..at + data.len()].copy_from_slice(data);
        self.program_ops += 1;
        Ok(())
    }

    fn erase(&mut self, block: u32) -> FlashResult<()> {
        if block as usize >= self.bad.len() {
            return Err(FlashError::OutOfRange);
        }
        if self.bad[block as usize] {
            return Err(FlashError::BadBlock { block });
        }
        if let Some(limit) = self.fail_erase_after {
            let done = self.erase_counts[block as usize] as u64;
            let total: u64 = self.erase_counts.iter().map(|c| u64::from(*c)).sum();
            let _ = done;
            if total >= limit {
                return Err(FlashError::Erase { block });
            }
        }
        let at = self.base(block);
        let end = at + self.erase_block_size;
        self.bytes[at..end].fill(ERASED);
        self.erase_counts[block as usize] += 1;
        Ok(())
    }

    fn sync(&mut self) -> FlashResult<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: usize = 256;
    const BLOCK: usize = 4096;

    #[test]
    fn program_只允许_1_到_0_且整体拒绝() {
        let mut f = SimFlash::new(2, PAGE, BLOCK);
        f.program(0, 0, &[0x0F]).unwrap();
        // 需要把第 0 字节的 0 位变成 1：整体拒绝。
        assert_eq!(
            f.program(0, 0, &[0xF0]),
            Err(FlashError::Program { block: 0 })
        );
        // 位与语义允许：0x0F & 0x0C == 0x0C。
        f.program(0, 0, &[0x0C]).unwrap();
        let mut got = [0u8; 1];
        f.read(0, 0, &mut got).unwrap();
        assert_eq!(got[0], 0x0C);
    }

    #[test]
    fn program_跨页被拒() {
        let mut f = SimFlash::new(1, PAGE, BLOCK);
        assert_eq!(
            f.program(0, PAGE - 1, &[0x00, 0x00]),
            Err(FlashError::Unaligned)
        );
    }

    #[test]
    fn erase_恢复为全_ff_并累加计数() {
        let mut f = SimFlash::new(1, PAGE, BLOCK);
        f.program(0, 0, &[0x00, 0x00]).unwrap();
        f.erase(0).unwrap();
        let mut got = [0u8; 2];
        f.read(0, 0, &mut got).unwrap();
        assert_eq!(got, [0xFF, 0xFF]);
        assert_eq!(f.erase_count_of(0), 1);
    }

    #[test]
    fn 越界与坏块返回对应错误() {
        let mut f = SimFlash::new(1, PAGE, BLOCK);
        assert_eq!(f.read(1, 0, &mut [0u8; 1]), Err(FlashError::OutOfRange));
        assert_eq!(f.read(0, BLOCK, &mut [0u8; 1]), Err(FlashError::OutOfRange));
        f.mark_bad(0);
        assert_eq!(
            f.read(0, 0, &mut [0u8; 1]),
            Err(FlashError::BadBlock { block: 0 })
        );
        assert_eq!(f.erase(0), Err(FlashError::BadBlock { block: 0 }));
    }

    #[test]
    fn 编程失败注入从第_n_次起生效() {
        let mut f = SimFlash::new(1, PAGE, BLOCK);
        f.inject_program_failure(1);
        f.program(0, 0, &[0x00]).unwrap();
        assert_eq!(
            f.program(0, 1, &[0x00]),
            Err(FlashError::Program { block: 0 })
        );
    }
}
