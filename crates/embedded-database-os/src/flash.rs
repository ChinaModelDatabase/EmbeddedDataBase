//! 文件支撑的 `RawFlash`：用普通文件模拟 NOR 闪存，供跨进程/掉电复验。

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use embedded_database_flash::raw::{FlashError, FlashResult, RawFlash};

/// 基于文件的 NOR 闪存模拟。
///
/// 语义与 `wasser` 的 `SimFlash` 一致（字节级 1→0、整块擦除、页对齐编程），
/// 额外支持"第 N 次编程起失败"以模拟断电。
pub struct FileRawFlash {
    file: File,
    path: PathBuf,
    page_size: usize,
    erase_block_size: usize,
    block_count: u32,
    programs: u64,
    fail_after: Option<u64>,
}

impl FileRawFlash {
    /// 打开或创建一个容量为 `block_count × erase_block_size` 的文件。
    /// 新文件初始化为全 `0xFF`（擦除态）。
    pub fn open(
        path: &Path,
        block_count: u32,
        page_size: usize,
        erase_block_size: usize,
    ) -> std::io::Result<Self> {
        let total = block_count as u64 * erase_block_size as u64;
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;
        if file.metadata()?.len() != total {
            file.set_len(total)?;
            // 新文件/尺寸变更后以擦除态 0xFF 填充（`set_len` 补零不是擦除态）。
            file.seek(SeekFrom::Start(0))
                .map_err(|_| std::io::Error::other("seek to init erased state"))?;
            let chunk = vec![0xFFu8; 64 * 1024];
            let mut left = total;
            while left > 0 {
                let take = chunk.len().min(left as usize) as u64;
                file.write_all(&chunk[..take as usize])
                    .map_err(|_| std::io::Error::other("fill erased state"))?;
                left -= take;
            }
            file.seek(SeekFrom::Start(0))
                .map_err(|_| std::io::Error::other("seek after erased-state fill"))?;
        }
        Ok(FileRawFlash {
            file,
            path: path.to_path_buf(),
            page_size,
            erase_block_size,
            block_count,
            programs: 0,
            fail_after: None,
        })
    }

    /// 使第 `n` 次（从 0 计数）及之后的编程全部失败，模拟断电。
    pub fn fail_after(&mut self, n: u64) {
        self.fail_after = Some(n);
    }

    /// 文件路径。
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 重新打开（模拟进程重启）。
    pub fn reopen(self) -> std::io::Result<Self> {
        FileRawFlash::open(
            &self.path,
            self.block_count,
            self.page_size,
            self.erase_block_size,
        )
    }

    fn base(&self, block: u32) -> u64 {
        u64::from(block) * self.erase_block_size as u64
    }

    fn check(&self, block: u32, offset: usize, len: usize) -> FlashResult<()> {
        if block >= self.block_count {
            return Err(FlashError::OutOfRange);
        }
        if offset > self.erase_block_size || len > self.erase_block_size - offset {
            return Err(FlashError::OutOfRange);
        }
        Ok(())
    }
}

impl RawFlash for FileRawFlash {
    fn page_size(&self) -> usize {
        self.page_size
    }
    fn erase_block_size(&self) -> usize {
        self.erase_block_size
    }
    fn block_count(&self) -> u32 {
        self.block_count
    }

    fn read(&self, block: u32, offset: usize, buf: &mut [u8]) -> FlashResult<()> {
        self.check(block, offset, buf.len())?;
        let mut f = &self.file;
        f.seek(SeekFrom::Start(self.base(block) + offset as u64))
            .map_err(|_| FlashError::Read { block })?;
        f.read_exact(buf).map_err(|_| FlashError::Read { block })?;
        Ok(())
    }

    fn program(&mut self, block: u32, offset: usize, data: &[u8]) -> FlashResult<()> {
        self.check(block, offset, data.len())?;
        if data.is_empty() {
            return Ok(());
        }
        let first = offset / self.page_size;
        let last = (offset + data.len() - 1) / self.page_size;
        if first != last {
            return Err(FlashError::Unaligned);
        }
        if let Some(limit) = self.fail_after {
            if self.programs >= limit {
                return Err(FlashError::Program { block });
            }
        }
        let at = self.base(block) + offset as u64;
        let mut cur = vec![0u8; data.len()];
        self.file
            .seek(SeekFrom::Start(at))
            .map_err(|_| FlashError::Program { block })?;
        self.file
            .read_exact(&mut cur)
            .map_err(|_| FlashError::Program { block })?;
        for (i, &byte) in data.iter().enumerate() {
            if cur[i] & byte != byte {
                return Err(FlashError::Program { block });
            }
        }
        self.file
            .seek(SeekFrom::Start(at))
            .map_err(|_| FlashError::Program { block })?;
        self.file
            .write_all(data)
            .map_err(|_| FlashError::Program { block })?;
        self.programs += 1;
        Ok(())
    }

    fn erase(&mut self, block: u32) -> FlashResult<()> {
        if block >= self.block_count {
            return Err(FlashError::OutOfRange);
        }
        let at = self.base(block);
        let zeros = vec![0xFFu8; self.erase_block_size];
        self.file
            .seek(SeekFrom::Start(at))
            .map_err(|_| FlashError::Erase { block })?;
        self.file
            .write_all(&zeros)
            .map_err(|_| FlashError::Erase { block })?;
        Ok(())
    }

    fn sync(&mut self) -> FlashResult<()> {
        self.file.sync_all().map_err(|_| FlashError::Sync)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use embedded_database_core::BlockDevice;
    use embedded_database_flash::ftl::{Ftl, FtlConfig};

    #[test]
    fn 文件中重建_ftl_后数据仍在() {
        let dir = std::env::temp_dir().join("embedded_database_flash_file_test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("dev0.bin");
        let _ = std::fs::remove_file(&path);
        let raw = FileRawFlash::open(&path, 64, 256, 4096).unwrap();
        let mut ftl = Ftl::new(raw, FtlConfig::default()).unwrap();
        let data = vec![0x5Au8; 3840];
        ftl.write_at(0, &data).unwrap();
        ftl.sync().unwrap();
        drop(ftl);
        let raw = FileRawFlash::open(&path, 64, 256, 4096).unwrap();
        let ftl = Ftl::new(raw, FtlConfig::default()).unwrap();
        let mut got = vec![0u8; 3840];
        ftl.read_at(0, &mut got).unwrap();
        assert_eq!(got, data);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn 编程失败注入后可重挂() {
        let dir = std::env::temp_dir().join("embedded_database_flash_fail_test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("dev1.bin");
        let _ = std::fs::remove_file(&path);
        let mut raw = FileRawFlash::open(&path, 64, 256, 4096).unwrap();
        raw.fail_after(2);
        let mut ftl = Ftl::new(raw, FtlConfig::default()).unwrap();
        // 第一次写必然触发搬迁/失败路径，但不得 panic；之后重挂仍可用。
        let _ = ftl.write_at(0, &vec![0x11u8; 3840]);
        drop(ftl);
        let raw = FileRawFlash::open(&path, 64, 256, 4096).unwrap();
        let ftl = Ftl::new(raw, FtlConfig::default()).unwrap();
        let mut got = vec![0u8; 16];
        assert!(ftl.read_at(0, &mut got).is_ok());
        let _ = std::fs::remove_file(&path);
    }
}
