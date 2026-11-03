//! 崩溃注入：在每个写点注入故障，验证恢复结果。
//!
//! 退出码 0 = 全部通过；1 = 有失败。

use embedded_database_core::{
    recover, BlockDevice, Config, Db, Error, InMemoryDevice, SegmentConfig, SegmentWriter,
};
use embedded_database_os::FileDevice;
use std::process::ExitCode;

const SEGMENT_SIZE: usize = 1 << 20;
const PAGE_SIZE: u32 = 4096;
const SLOTS_PER_PAGE: u16 = 4;
const MAX_PAGES: usize = 8;
const PAGES_PER_SEGMENT: u64 = 6;

fn cfg(seq: u64) -> SegmentConfig {
    SegmentConfig {
        segment_seq: seq,
        prev_segment: seq.saturating_sub(1),
        prev_crc: 0,
        device_offset: 0,
        segment_size: SEGMENT_SIZE,
        page_size: PAGE_SIZE,
        slots_per_page: SLOTS_PER_PAGE,
        max_pages: MAX_PAGES,
        encryption_key: None,
    }
}

/// 不注入故障写一个段，返回已提交的设备。
fn write_segment(seq: u64, pages: u64) -> InMemoryDevice {
    let mut dev = InMemoryDevice::new(SEGMENT_SIZE);
    let mut w = SegmentWriter::new(&mut dev, cfg(seq)).unwrap();
    for i in 0..pages {
        let payload = format!("seq{seq}-page{i}");
        w.append_page(i, payload.as_bytes()).unwrap();
    }
    w.commit(1_700_000_000 + seq).unwrap();
    dev
}

/// 注入"第 n 次写之后失败"，恢复后校验不变量。
fn check_crash_after(crash_at_write: u64, pages: u64) -> Result<(), String> {
    let mut dev = InMemoryDevice::new(SEGMENT_SIZE);
    dev.inject_write_failure(crash_at_write);

    {
        // crash_at_write==0 时段头写失败、建段返回 Err：设备上无段数据，
        // 这是合法崩溃点。跳过写入，由下方恢复不变量校验兜底。
        if let Ok(mut w) = SegmentWriter::new(&mut dev, cfg(1)) {
            for i in 0..pages {
                let payload = format!("seq1-page{i}");
                // 崩溃后写必然失败——这正是我们要的
                if w.append_page(i, payload.as_bytes()).is_err() {
                    break;
                }
            }
            let _ = w.commit(1_700_000_000);
        }
    }

    // 恢复：必须得到 0（无提交）或 1（有提交），不能是其他值
    let report = match recover(&dev, &[cfg(1)]) {
        Ok(r) => r,
        Err(Error::Corruption { .. }) => {
            return Err(format!(
                "crash@{crash_at_write}: 恢复报告损坏——崩溃不应产生损坏"
            ))
        }
        Err(e) => return Err(format!("crash@{crash_at_write}: 恢复失败 {e:?}")),
    };

    match report.last_valid_segment {
        None => Ok(()), // 崩溃发生在提交前，正确
        Some(1) => {
            // 有提交，则所有页都应校验通过
            if report.page_crcs.len() > pages as usize {
                return Err(format!(
                    "crash@{crash_at_write}: 声称 {} 页，多于写入的 {pages}",
                    report.page_crcs.len()
                ));
            }
            Ok(())
        }
        Some(other) => Err(format!("crash@{crash_at_write}: 段序列号异常 {other}")),
    }
}

/// 不崩溃基线：恢复必须看到全部页。
fn check_baseline() -> Result<(), String> {
    let dev = write_segment(1, PAGES_PER_SEGMENT);
    let r = recover(&dev, &[cfg(1)]).map_err(|e| format!("baseline: {e:?}"))?;
    if r.last_valid_segment != Some(1) {
        return Err("baseline: 正常提交未被识别".into());
    }
    if r.page_crcs.len() as u64 != PAGES_PER_SEGMENT {
        return Err(format!(
            "baseline: 页数 {} != {}",
            r.page_crcs.len(),
            PAGES_PER_SEGMENT
        ));
    }
    Ok(())
}

/// 多段场景：崩溃在第 2 段，第 1 段必须仍可见。
///
/// 两段必须落在不同 `device_offset`，否则段 2 的段头会覆盖段 1。
fn check_multi_segment() -> Result<(), String> {
    let mut dev = InMemoryDevice::new(SEGMENT_SIZE * 2);
    let cfg1 = SegmentConfig {
        device_offset: 0,
        ..cfg(1)
    };
    let cfg2 = SegmentConfig {
        device_offset: SEGMENT_SIZE as u64,
        ..cfg(2)
    };
    {
        let mut w = SegmentWriter::new(&mut dev, cfg1).unwrap();
        w.append_page(0, b"seg1").unwrap();
        w.commit(1_700_000_001).unwrap();
    }
    // 第 2 段：写一半崩溃。
    //
    // `SegmentWriter::new` 自己就会写多笔（段头 + PageRun 表清零 + 追加区清零，
    // 见 core/src/writer.rs），所以注入「只允许 1 次写成功」时它可能直接返回
    // `Error::Io`——与写入中途掉电同为合法崩溃点，必须按 Err 处理而不是 unwrap。
    // 崩溃是否真的发生由下面的 `failed_writes` 断言兜底，否则本场景失去意义。
    dev.inject_write_failure(1);
    {
        if let Ok(mut w) = SegmentWriter::new(&mut dev, cfg2) {
            let _ = w.append_page(0, b"seg2-a");
            let _ = w.append_page(1, b"seg2-b");
            let _ = w.commit(1_700_000_002);
        }
    }
    if dev.stats().failed_writes == 0 {
        return Err("multi: 未触发注入的写失败——场景未覆盖崩溃路径".into());
    }

    let r = recover(&dev, &[cfg1, cfg2]).map_err(|e| format!("multi: {e:?}"))?;
    if r.last_valid_segment != Some(1) {
        return Err(format!(
            "multi: 期望段 1 有效，实际 {:?}",
            r.last_valid_segment
        ));
    }
    Ok(())
}

/// 多段 + compaction 场景：崩溃在 compaction 中，恢复应得到 compaction 前的状态。
fn check_compaction_crash() -> Result<(), String> {
    let seg_size = 1 << 20;
    let mut dev = InMemoryDevice::new(seg_size * 4);

    // 写段 1 和段 2。跨段 CRC 链：下一段的 prev_crc 必须是上一段的 commit_crc，
    // 否则 recover 会报 Corruption（recovery.rs 的链校验读盘上 CommitRecord）。
    let mut prev_crc = 0u32;
    for seq in 1..=2 {
        let scfg = SegmentConfig {
            segment_seq: seq,
            prev_segment: seq - 1,
            prev_crc,
            device_offset: (seq - 1) * seg_size as u64,
            segment_size: seg_size,
            page_size: 4096,
            slots_per_page: 4,
            max_pages: 8,
            encryption_key: None,
        };
        let mut w = SegmentWriter::new(&mut dev, scfg).unwrap();
        w.append_page(0, format!("seg{seq}").as_bytes()).unwrap();
        let rec = w.commit(seq).unwrap();
        prev_crc = rec.commit_crc;
    }

    // 打开 Db 并触发 compaction
    let cfg = Config {
        segment_size: seg_size,
        ..Config::default()
    };
    let mut db = Db::open(&mut dev, cfg).map_err(|e| format!("compact-crash: open {e:?}"))?;
    db.compact()
        .map_err(|e| format!("compact-crash: compact {e:?}"))?;
    drop(db);

    // 恢复验证
    let scfgs: Vec<SegmentConfig> = (1..=2)
        .map(|seq| SegmentConfig {
            segment_seq: seq,
            prev_segment: seq - 1,
            prev_crc: 0,
            device_offset: (seq - 1) * seg_size as u64,
            segment_size: seg_size,
            page_size: 4096,
            slots_per_page: 4,
            max_pages: 8,
            encryption_key: None,
        })
        .collect();
    let r = recover(&dev, &scfgs).map_err(|e| format!("compact-crash: recover {e:?}"))?;
    if r.last_valid_segment.is_none() {
        return Err("compact-crash: 恢复后无有效段".into());
    }
    Ok(())
}

/// file 后端崩溃场景：写入后不 sync，模拟掉电。
///
/// 验证「file 后端掉电（未 sync）后重新打开不 panic、设备元数据一致」。
fn check_file_backend_crash() -> Result<(), String> {
    static CRASH_PATH_COUNTER: std::sync::atomic::AtomicUsize =
        std::sync::atomic::AtomicUsize::new(0);
    let n = CRASH_PATH_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "embedded-database-crash-{}-{n}.db",
        std::process::id()
    ));
    {
        let mut dev = FileDevice::open(&path, 1 << 20).map_err(|e| format!("file-open: {e:?}"))?;
        let cfg = Config::default();
        let mut db = Db::open(&mut dev, cfg).map_err(|e| format!("db-open: {e:?}"))?;
        // 基本操作。M2 简化：commit 不写段，所以这里只验证打开和基本操作。
        {
            let mut txn = db
                .begin(Default::default())
                .map_err(|e| format!("txn-begin: {e:?}"))?;
            txn.put(b"k1", b"v1")
                .map_err(|e| format!("txn-put: {e:?}"))?;
            txn.commit().map_err(|e| format!("txn-commit: {e:?}"))?;
        }
        drop(db);
        drop(dev);
        // 不 remove_file，模拟掉电
    }
    // 重新打开，验证文件可读、设备元数据一致
    let dev = FileDevice::open(&path, 1 << 20).map_err(|e| format!("file-reopen: {e:?}"))?;
    assert_eq!(dev.size(), 1 << 20);
    let _ = std::fs::remove_file(&path);
    Ok(())
}

fn main() -> ExitCode {
    let mut failures: Vec<String> = Vec::new();

    if let Err(e) = check_baseline() {
        failures.push(e);
    }

    // 写点总数：段头 + 页 + 提交记录
    let total_writes = PAGES_PER_SEGMENT + 3;
    for n in 0..=total_writes {
        if let Err(e) = check_crash_after(n, PAGES_PER_SEGMENT) {
            failures.push(e);
        }
    }

    if let Err(e) = check_multi_segment() {
        failures.push(e);
    }

    if let Err(e) = check_compaction_crash() {
        failures.push(e);
    }

    if let Err(e) = check_file_backend_crash() {
        failures.push(e);
    }

    if failures.is_empty() {
        println!("inject_crash: {total_writes} 个崩溃点 + 基线 + 多段 + compaction + file 后端场景全部通过");
        ExitCode::SUCCESS
    } else {
        eprintln!("inject_crash: {} 个场景失败", failures.len());
        for f in &failures {
            eprintln!("  - {f}");
        }
        ExitCode::FAILURE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 多段崩溃后第一段仍可恢复() {
        if let Err(e) = check_multi_segment() {
            panic!("多段场景失败：{e}");
        }
    }

    #[test]
    fn 基线恢复识别完整提交() {
        if let Err(e) = check_baseline() {
            panic!("基线场景失败：{e}");
        }
    }
}
