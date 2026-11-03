//! WCET 实测：测量 M1 核心操作的时延分布。
//!
//! 打印 p50 / p99 / max / min，与预算对比。M2 的预算以此为基线。

use embedded_database_core::{
    Arena, BTree, Config, Db, InMemoryDevice, SegmentConfig, SegmentWriter, TxnOptions,
};
use std::time::Instant;

const SEGMENT_SIZE: usize = 1 << 20;
const PAGE_SIZE: u32 = 4096;
const SLOTS_PER_PAGE: u16 = 4;
const MAX_PAGES: usize = 8;

/// 预算（纳秒）。超出即退出码 1。
///
/// 基线来自 debug 构建实测（见 README）：append_page 每次写整页 4096B
/// 并做全页 CRC；macOS 共享开发机上调度噪声可把 p99 推高一个数量级。
/// 预算按 debug 实测 p99 上界留余量，机器相关。
const BUDGET_APPEND_PAGE_NS: u128 = 150_000;
const BUDGET_BTREE_INSERT_NS: u128 = 200_000;
const BUDGET_COMMIT_NS: u128 = 30_000;
const BUDGET_DB_PUT_NS: u128 = 10_000;
const BUDGET_DB_GET_NS: u128 = 10_000;
const BUDGET_DB_COMMIT_NS: u128 = 10_000;

/// 时延统计。
#[derive(Debug)]
struct Stats {
    label: &'static str,
    samples: std::vec::Vec<u128>,
}

impl Stats {
    fn new(label: &'static str) -> Self {
        Stats {
            label,
            samples: std::vec::Vec::new(),
        }
    }

    fn record(&mut self, ns: u128) {
        self.samples.push(ns);
    }

    fn report(&self) -> (u128, u128, u128, u128) {
        let mut s = self.samples.clone();
        s.sort_unstable();
        let p50 = s[s.len() / 2];
        let p99 = s[(s.len() * 99 / 100).min(s.len() - 1)];
        (s[0], p50, p99, s[s.len() - 1])
    }
}

fn main() -> std::process::ExitCode {
    let iter = 20_000u32;
    let mut exceeded = std::vec::Vec::new();

    // ---- 段页追加 ----
    let mut append = Stats::new("append_page");
    {
        let mut dev = InMemoryDevice::new(SEGMENT_SIZE);
        let mut w = SegmentWriter::new(
            &mut dev,
            SegmentConfig {
                segment_seq: 1,
                prev_segment: 0,
                prev_crc: 0,
                device_offset: 0,
                segment_size: SEGMENT_SIZE,
                page_size: PAGE_SIZE,
                slots_per_page: SLOTS_PER_PAGE,
                max_pages: MAX_PAGES,
                encryption_key: None,
            },
        )
        .unwrap();
        let payload = std::vec![0xABu8; 256];
        for i in 0..iter {
            let t0 = Instant::now();
            // 超过槽配额会失败——那不是我们要测的路径，跳过
            if w.append_page(i as u64 % MAX_PAGES as u64, &payload).is_ok() {
                append.record(t0.elapsed().as_nanos());
            }
        }
    }

    // ---- B-tree 插入 ----
    let mut insert = Stats::new("btree_insert");
    {
        let mut buf = std::vec![0u8; 1 << 20];
        let mut arena = Arena::new(&mut buf);
        let mut tree = BTree::new(&mut arena).unwrap();
        for i in 0..iter {
            let k = format!("k{i:08}");
            let t0 = Instant::now();
            if tree.insert(k.as_bytes(), b"v").is_ok() {
                insert.record(t0.elapsed().as_nanos());
            }
        }
    }

    // ---- 提交 ----
    let mut commit = Stats::new("commit");
    {
        for _ in 0..200 {
            let mut dev = InMemoryDevice::new(SEGMENT_SIZE);
            let mut w = SegmentWriter::new(
                &mut dev,
                SegmentConfig {
                    segment_seq: 1,
                    prev_segment: 0,
                    prev_crc: 0,
                    device_offset: 0,
                    segment_size: SEGMENT_SIZE,
                    page_size: PAGE_SIZE,
                    slots_per_page: SLOTS_PER_PAGE,
                    max_pages: MAX_PAGES,
                    encryption_key: None,
                },
            )
            .unwrap();
            w.append_page(0, b"x").unwrap();
            let t0 = Instant::now();
            let _ = w.commit(1);
            commit.record(t0.elapsed().as_nanos());
        }
    }

    // ---- Db 事务路径（M2 / M7 / M8）----
    // M7：`begin()` 自动 flush 待落盘数据，每次 flush 占用一个新段。
    // M8-3：page_id 全局递增，SlotAllocator 要求 page_id < max_pages，
    // 因此 max_pages 必须 ≥ DB_ITER（否则第 9 次 begin 因 page_id=8
    // 触发 CapacityExhausted）。max_pages 决定段内预留槽区：
    //   append_start = 56 + max_pages * 4096 * 4
    // 段大小须大于 append_start + 提交记录 + 至少一页数据。
    const DB_ITER: u32 = 200;
    const DB_MAX_PAGES: usize = DB_ITER as usize; // ≥ 最大全局 page_id
    const DB_SEGMENT_SIZE: usize = 1 << 22; // 4 MiB（容纳 max_pages=200 预留区）
                                            // 每次 flush一个新段；DB_ITER 次事务 ≈ DB_ITER-1 次 flush，再留余量。
    const DB_MAX_SEGMENTS: usize = DB_ITER as usize + 2;
    const DB_DEV_SIZE: usize = DB_SEGMENT_SIZE * DB_MAX_SEGMENTS;
    let mut db_put = Stats::new("db_put");
    let mut db_get = Stats::new("db_get");
    let mut db_commit = Stats::new("db_commit");
    {
        let mut dev = InMemoryDevice::new(DB_DEV_SIZE);
        let cfg = Config {
            segment_size: DB_SEGMENT_SIZE,
            max_segments: DB_MAX_SEGMENTS,
            max_pages: DB_MAX_PAGES,
            ..Config::default()
        };
        let mut db = Db::open(&mut dev, cfg).unwrap();
        for i in 0..DB_ITER {
            let mut txn = db.begin(TxnOptions::default()).unwrap();
            let k = format!("k{i:08}");
            let t0 = Instant::now();
            txn.put(k.as_bytes(), b"v").unwrap();
            db_put.record(t0.elapsed().as_nanos());
            let t0 = Instant::now();
            let _ = txn.get(k.as_bytes());
            db_get.record(t0.elapsed().as_nanos());
            let t0 = Instant::now();
            let _ = txn.commit();
            db_commit.record(t0.elapsed().as_nanos());
        }
    }

    for (stats, budget) in [
        (&append, BUDGET_APPEND_PAGE_NS),
        (&insert, BUDGET_BTREE_INSERT_NS),
        (&commit, BUDGET_COMMIT_NS),
        (&db_put, BUDGET_DB_PUT_NS),
        (&db_get, BUDGET_DB_GET_NS),
        (&db_commit, BUDGET_DB_COMMIT_NS),
    ] {
        if stats.samples.is_empty() {
            eprintln!("wcet: {} 无有效样本", stats.label);
            exceeded.push(stats.label.to_string());
            continue;
        }
        let (min, p50, p99, max) = stats.report();
        println!(
            "{:<14} n={:<6} min={min:>6}ns p50={p50:>6}ns p99={p99:>6}ns max={max:>8}ns budget={budget}ns",
            stats.label,
            stats.samples.len()
        );
        if p99 > budget {
            eprintln!("wcet: {} p99 {}ns 超出预算 {}ns", stats.label, p99, budget);
            exceeded.push(stats.label.to_string());
        }
    }

    if exceeded.is_empty() {
        std::process::ExitCode::SUCCESS
    } else {
        eprintln!("wcet: {} 项超预算", exceeded.len());
        std::process::ExitCode::FAILURE
    }
}
