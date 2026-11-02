//! Linux file 后端：用 `std::fs::File` 实现 `BlockDevice`。

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use embedded_database_core::{BlockDevice, Error, Result};

/// 基于文件的块设备。
pub struct FileDevice {
    file: File,
    size: u64,
}

impl FileDevice {
    /// 打开或创建文件。文件不存在时创建并扩展到 `size` 字节。
    pub fn open(path: &Path, size: u64) -> Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .map_err(|_| Error::Io)?;
        let file_size = file.metadata().map_err(|_| Error::Io)?.len();
        if file_size < size {
            file.set_len(size).map_err(|_| Error::Io)?;
        }
        Ok(FileDevice { file, size })
    }
}

impl BlockDevice for FileDevice {
    fn size(&self) -> u64 {
        self.size
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        let end = offset
            .checked_add(buf.len() as u64)
            .ok_or(Error::InvalidArgument)?;
        if end > self.size {
            return Err(Error::InvalidArgument);
        }
        let mut f = self.file.try_clone().map_err(|_| Error::Io)?;
        f.seek(SeekFrom::Start(offset)).map_err(|_| Error::Io)?;
        f.read_exact(buf).map_err(|_| Error::Io)?;
        Ok(())
    }

    fn write_at(&mut self, offset: u64, data: &[u8]) -> Result<()> {
        let end = offset
            .checked_add(data.len() as u64)
            .ok_or(Error::InvalidArgument)?;
        if end > self.size {
            return Err(Error::InvalidArgument);
        }
        self.file
            .seek(SeekFrom::Start(offset))
            .map_err(|_| Error::Io)?;
        self.file.write_all(data).map_err(|_| Error::Io)?;
        Ok(())
    }

    fn sync(&mut self) -> Result<()> {
        self.file.sync_all().map_err(|_| Error::Io)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_path() -> PathBuf {
        static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        std::env::temp_dir().join(format!("embedded-database-test-{}-{}.db", std::process::id(), n))
    }

    #[test]
    fn 创建写入读回() {
        let path = temp_path();
        let mut dev = FileDevice::open(&path, 4096).unwrap();
        dev.write_at(0, b"hello").unwrap();
        dev.sync().unwrap();
        let mut buf = [0u8; 5];
        dev.read_at(0, &mut buf).unwrap();
        assert_eq!(&buf, b"hello");
        assert_eq!(dev.size(), 4096);
        drop(dev);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn 越界读写被拒() {
        let path = temp_path();
        let mut dev = FileDevice::open(&path, 16).unwrap();
        assert!(dev.write_at(16, b"x").is_err());
        let mut buf = [0u8; 4];
        assert!(dev.read_at(14, &mut buf).is_err());
        drop(dev);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn 打开已存在文件() {
        let path = temp_path();
        {
            let mut dev = FileDevice::open(&path, 1024).unwrap();
            dev.write_at(0, b"persisted").unwrap();
            dev.sync().unwrap();
        }
        // 重新打开，数据仍在
        let dev = FileDevice::open(&path, 1024).unwrap();
        let mut buf = [0u8; 9];
        dev.read_at(0, &mut buf).unwrap();
        assert_eq!(&buf, b"persisted");
        let _ = std::fs::remove_file(&path);
    }
}
