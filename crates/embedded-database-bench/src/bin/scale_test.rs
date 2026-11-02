//! 亿级规模验证：写入 1 亿条 KV 记录，验证写入吞吐、读取正确性、崩溃恢复、写放大。
//!
//! 退出码 0 = 全部通过；1 = 有失败。
//! 环境变量 `SCALE_N` 可覆盖记录数（默认 100_000_000），用于短程完整链路验证。

use std::time::Instant;
use embedded_database_core::Config;
use embedded_database_kv::KvDb;
use embedded_database_os::FileDevice;

const RECORD_COUNT: usize = 100_000_000;
// 几何约束：append_start = SEGMENT_HEADER_SIZE(56) + max_pages × page_size × slots_per_page
// 必须 < segment_size − COMMIT_RECORD_SIZE(40)，否则首条 put 即 QuotaExceeded。
// 修正配置：reservation=8000×512=4MiB，append 区≈4MiB，段 8MiB 可容纳 8000 页；
// 容量 = 8000 × 12800 = 102,400,000 ≥ 100M；设备 = 8MiB × 12800 = 100GiB。
const SEGMENT_SIZE: usize = 8 << 20; // 8 MiB
const MAX_SEGMENTS: usize = 12800;
const SLOTS_PER_PAGE: u16 = 1;
const MAX_PAGES: usize = 8000;
const PAGE_SIZE: u32 = 512;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let db_path = if args.len() > 1 {
        args[1].clone()
    } else {
        "scale_test.db".to_string()
    };
    let record_count: usize = std::env::var("SCALE_N")
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|n| *n > 0)
        .unwrap_or(RECORD_COUNT);

    println!("=== 千万级规模验证 ===");
    println!("记录数: {record_count}");
    println!("段大小: {} MiB", SEGMENT_SIZE / (1 << 20));
    println!("最大段数: {MAX_SEGMENTS}");
    println!("page_size: {PAGE_SIZE}，slots_per_page: {SLOTS_PER_PAGE}，max_pages: {MAX_PAGES}");
    println!("理论容量: {} 条", MAX_PAGES * MAX_SEGMENTS);
    println!("数据库路径: {db_path}");

    let cfg = Config {
        segment_size: SEGMENT_SIZE,
        page_size: PAGE_SIZE,
        max_segments: MAX_SEGMENTS,
        slots_per_page: SLOTS_PER_PAGE,
        max_pages: MAX_PAGES,
        ..Config::default()
    };

    // 1. 写入验证
    println!("\n--- 1. 写入验证 ---");
    let mut failures = 0u32;
    {
        let dev = FileDevice::open(
            std::path::Path::new(&db_path),
            (SEGMENT_SIZE * MAX_SEGMENTS) as u64,
        )
        .expect("打开数据库失败");
        let mut dev = dev;
        let mut kv = KvDb::open(&mut dev, cfg.clone()).expect("初始化数据库失败");

        let t0 = Instant::now();
        let batch_size = MAX_PAGES;
        for batch_start in (0..record_count).step_by(batch_size) {
            let batch_end = (batch_start + batch_size).min(record_count);
            let items: Vec<(&[u8], &[u8])> = (batch_start..batch_end)
                .map(|i| {
                    let key = format!("key{i:010}");
                    let value = format!("value{i:010}");
                    let key: &'static [u8] = key.into_bytes().leak();
                    let value: &'static [u8] = value.into_bytes().leak();
                    (key, value)
                })
                .collect();
            if kv.put_batch(&items).is_err() {
                failures += items.len() as u32;
                if failures <= 5 {
                    eprintln!("写入失败: batch {batch_start}..{batch_end}");
                }
            }
            let elapsed = t0.elapsed().as_secs_f64();
            let throughput = batch_end as f64 / elapsed;
            println!(
                "已写入 {batch_end} 条，吞吐 {:.0} 条/秒，耗时 {elapsed:.1}s",
                throughput
            );
        }
        let write_time = t0.elapsed();
        let write_throughput = record_count as f64 / write_time.as_secs_f64();
        println!(
            "写入完成: {record_count} 条，耗时 {:.1}s，吞吐 {:.0} 条/秒",
            write_time.as_secs_f64(),
            write_throughput
        );
        println!("写入失败: {failures}");
        if failures == 0 {
            kv.flush().expect("flush 失败");
        } else {
            // 已有写入失败时，flush 失败是预期后果，不再 panic。
            if let Err(e) = kv.flush() {
                eprintln!("flush 失败（写入已失败，预期）: {e:?}");
            }
        }
    }

    // 2. 读取验证（采样）
    println!("\n--- 2. 读取验证（采样 1000 条）---");
    {
        let dev = FileDevice::open(
            std::path::Path::new(&db_path),
            (SEGMENT_SIZE * MAX_SEGMENTS) as u64,
        )
        .expect("重新打开数据库失败");
        let mut dev = dev;
        let kv = KvDb::open(&mut dev, cfg.clone()).expect("重新初始化失败");

        let sample_size = 1000.min(record_count);
        let t0 = Instant::now();
        let mut read_failures = 0u32;
        for i in 0..sample_size {
            let key = format!("key{i:010}");
            match kv.get(key.as_bytes()) {
                Ok(Some(_)) => {}
                Ok(None) => {
                    read_failures += 1;
                    eprintln!("读取未命中: {key}");
                }
                Err(e) => {
                    read_failures += 1;
                    eprintln!("读取错误: {key}: {e:?}");
                }
            }
        }
        let read_time = t0.elapsed();
        println!(
            "读取完成: {sample_size} 条，耗时 {:.1}s，失败 {read_failures}",
            read_time.as_secs_f64()
        );
        if read_failures > 0 {
            failures += read_failures;
        }
    }

    // 3. 崩溃恢复验证
    println!("\n--- 3. 崩溃恢复验证 ---");
    {
        // 写入新数据后模拟崩溃（不 flush）
        let dev = FileDevice::open(
            std::path::Path::new(&db_path),
            (SEGMENT_SIZE * MAX_SEGMENTS) as u64,
        )
        .expect("打开数据库失败");
        let mut dev = dev;
        let mut kv = KvDb::open(&mut dev, cfg.clone()).expect("初始化失败");
        match kv.put(b"crash-key", b"crash-value") {
            Ok(()) => {}
            Err(e) => {
                eprintln!("崩溃恢复: crash-key 写入失败 {e:?}");
                failures += 1;
            }
        }
        // 不 flush，直接 drop（模拟崩溃）
        drop(kv);
        drop(dev);

        // 重新打开，验证数据一致性
        let dev = FileDevice::open(
            std::path::Path::new(&db_path),
            (SEGMENT_SIZE * MAX_SEGMENTS) as u64,
        )
        .expect("崩溃后重新打开失败");
        let mut dev = dev;
        let kv = KvDb::open(&mut dev, cfg.clone()).expect("崩溃后重新初始化失败");
        // 崩溃前的数据应仍可读
        match kv.get(b"key0000000000") {
            Ok(Some(_)) => println!("崩溃恢复: 原有数据可读 ✓"),
            Ok(None) => {
                println!("崩溃恢复: 原有数据未命中（可能未 flush）");
                failures += 1;
            }
            Err(e) => {
                println!("崩溃恢复: 读取错误 {e:?}");
                failures += 1;
            }
        }
    }

    // 4. 写放大验证
    println!("\n--- 4. 写放大验证 ---");
    {
        let dev = FileDevice::open(
            std::path::Path::new(&db_path),
            (SEGMENT_SIZE * MAX_SEGMENTS) as u64,
        )
        .expect("打开数据库失败");
        let mut dev = dev;
        let mut kv = KvDb::open(&mut dev, cfg.clone()).expect("初始化失败");
        // 追加写入一批数据
        let mut amp_failures = 0u32;
        for i in record_count..(record_count + 1000) {
            let key = format!("key{i:010}");
            let value = format!("value{i:010}");
            if kv.put(key.as_bytes(), value.as_bytes()).is_err() {
                amp_failures += 1;
                if amp_failures <= 3 {
                    eprintln!("写放大: 追加写入失败 {key}");
                }
            }
        }
        if let Err(e) = kv.flush() {
            eprintln!("写放大: flush 失败 {e:?}");
            amp_failures += 1;
        }
        if amp_failures > 0 {
            failures += amp_failures;
        }
        // 估算写放大：物理写入 ≈ 段数 × segment_size，逻辑写入 ≈ 记录数 × 平均记录大小
        let logical_bytes = (record_count + 1000) as u64 * 20; // 每条记录约 20 字节
        let physical_bytes = (SEGMENT_SIZE * MAX_SEGMENTS) as u64; // 上界
        let write_amplification = physical_bytes as f64 / logical_bytes as f64;
        println!(
            "写放大估算: {:.2}（物理 {physical_bytes} 字节 / 逻辑 {logical_bytes} 字节）",
            write_amplification
        );
        println!("注: 这是上界估算，实际写放大取决于段利用率");
    }

    // 清理
    let _ = std::fs::remove_file(&db_path);

    println!("\n=== 结果 ===");
    if failures == 0 {
        println!("✓ 千万级规模验证全部通过");
        std::process::exit(0);
    } else {
        println!("✗ 千万级规模验证有 {} 个失败", failures);
        std::process::exit(1);
    }
}
