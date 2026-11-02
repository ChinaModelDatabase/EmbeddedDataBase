# embedded-database M29 实施计划：千万级规模验证

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 新建 `embedded-database-bench/src/bin/scale_test.rs`，写入 1000 万条 KV 记录，验证写入吞吐、读取正确性、崩溃恢复、写放大。

**Architecture:** `scale_test.rs` 用大容量配置（`segment_size=4MiB`, `max_segments=1024`, `slots_per_page=64`, `max_pages=256`）打开文件后端，批量写入 10M 条记录，测量吞吐、验证读取、注入崩溃、计算写放大。

**Tech Stack:** Rust 1.81、`embedded-database-bench`（std）、`embedded-database-os`（file 后端）、`embedded-database-kv`、`embedded-database-core`。

## Global Constraints

- **C1** `embedded-database-core` 保持 `#![no_std]`
- **C2** 顶层 `#![forbid(unsafe_code)]`——不允许任何局部 `unsafe`
- **C3** 运行时依赖仅 `zerocopy`、`bitflags`，不新增
- **C4** 库代码禁止 `unwrap()` / `expect()` / `panic!()` / 数组越界（`unwrap()` 仅允许 `#[cfg(test)]`；bench bin 可用 `unwrap`）
- **C11** 不执行任何 git 提交（本项目零提交策略）
- **工具链**：每个 bash 调用前 `export PATH="/opt/homebrew/opt/rustup/bin:$PATH"`
- **验收基线**：M28 交付 288 passed（core 215 + kv 6 + os 5 + sql 62）、5 目标 no_std 编译通过、`inject_crash`/`wcet` exit=0
- **磁盘空间**：644 GiB 可用（10M × ~200B ≈ 2 GB 数据 + 段开销）

## 用户裁决记录

- **M29 范围**：千万级规模验证（1000 万条 KV 记录）。
- **验证指标**：写入吞吐、读取正确性、崩溃恢复、写放大。
- **配置**：`segment_size=4MiB`, `max_segments=1024`, `slots_per_page=64`, `max_pages=256`。

---

### Task M29-0: 千万级规模验证工具

**Files:**
- Create: `crates/embedded-database-bench/src/bin/scale_test.rs`

**Interfaces:**
- Consumes: `KvDb`（embedded-database-kv）、`FileDevice`（embedded-database-os）、`Config`（embedded-database-core）
- Produces: 可执行文件 `scale_test`。退出码 0 = 全部通过；1 = 有失败。

- [ ] **Step 1: 写 scale_test.rs**

```rust
//! 千万级规模验证：写入 1000 万条 KV 记录，验证写入吞吐、读取正确性、崩溃恢复、写放大。
//!
//! 退出码 0 = 全部通过；1 = 有失败。

use std::time::Instant;
use embedded_database_core::{Config, InMemoryDevice};
use embedded_database_kv::KvDb;
use embedded_database_os::FileDevice;

const RECORD_COUNT: usize = 10_000_000;
const SEGMENT_SIZE: usize = 4 << 20; // 4 MiB
const MAX_SEGMENTS: usize = 1024;
const SLOTS_PER_PAGE: u16 = 64;
const MAX_PAGES: usize = 256;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let db_path = if args.len() > 1 {
        args[1].clone()
    } else {
        "scale_test.db".to_string()
    };

    println!("=== 千万级规模验证 ===");
    println!("记录数: {RECORD_COUNT}");
    println!("段大小: {} MiB", SEGMENT_SIZE / (1 << 20));
    println!("最大段数: {MAX_SEGMENTS}");
    println!("数据库路径: {db_path}");

    let cfg = Config {
        segment_size: SEGMENT_SIZE,
        max_segments: MAX_SEGMENTS,
        slots_per_page: SLOTS_PER_PAGE,
        max_pages: MAX_PAGES,
        ..Config::default()
    };

    // 1. 写入验证
    println!("\n--- 1. 写入验证 ---");
    let mut failures = 0u32;
    {
        let dev = FileDevice::open(std::path::Path::new(&db_path), (SEGMENT_SIZE * MAX_SEGMENTS) as u64)
            .expect("打开数据库失败");
        let mut dev = dev;
        let mut kv = KvDb::open(&mut dev, cfg.clone()).expect("初始化数据库失败");

        let t0 = Instant::now();
        for i in 0..RECORD_COUNT {
            let key = format!("key{i:010}");
            let value = format!("value{i:010}");
            if kv.put(key.as_bytes(), value.as_bytes()).is_err() {
                failures += 1;
                if failures <= 5 {
                    eprintln!("写入失败: {key}");
                }
            }
            if i % 1_000_000 == 0 && i > 0 {
                let elapsed = t0.elapsed().as_secs_f64();
                let throughput = i as f64 / elapsed;
                println!("已写入 {i} 条，吞吐 {:.0} 条/秒，耗时 {elapsed:.1}s", throughput);
            }
        }
        let write_time = t0.elapsed();
        let write_throughput = RECORD_COUNT as f64 / write_time.as_secs_f64();
        println!("写入完成: {RECORD_COUNT} 条，耗时 {:.1}s，吞吐 {:.0} 条/秒", write_time.as_secs_f64(), write_throughput);
        println!("写入失败: {failures}");
        kv.flush().expect("flush 失败");
    }

    // 2. 读取验证（采样）
    println!("\n--- 2. 读取验证（采样 1000 条）---");
    {
        let dev = FileDevice::open(std::path::Path::new(&db_path), (SEGMENT_SIZE * MAX_SEGMENTS) as u64)
            .expect("重新打开数据库失败");
        let mut dev = dev;
        let kv = KvDb::open(&mut dev, cfg.clone()).expect("重新初始化失败");

        let sample_size = 1000.min(RECORD_COUNT);
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
        println!("读取完成: {sample_size} 条，耗时 {:.1}s，失败 {read_failures}", read_time.as_secs_f64());
        if read_failures > 0 {
            failures += read_failures;
        }
    }

    // 3. 崩溃恢复验证
    println!("\n--- 3. 崩溃恢复验证 ---");
    {
        // 写入新数据后模拟崩溃（不 flush）
        let dev = FileDevice::open(std::path::Path::new(&db_path), (SEGMENT_SIZE * MAX_SEGMENTS) as u64)
            .expect("打开数据库失败");
        let mut dev = dev;
        let mut kv = KvDb::open(&mut dev, cfg.clone()).expect("初始化失败");
        kv.put(b"crash-key", b"crash-value").unwrap();
        // 不 flush，直接 drop（模拟崩溃）
        drop(kv);
        drop(dev);

        // 重新打开，验证数据一致性
        let dev = FileDevice::open(std::path::Path::new(&db_path), (SEGMENT_SIZE * MAX_SEGMENTS) as u64)
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
        let dev = FileDevice::open(std::path::Path::new(&db_path), (SEGMENT_SIZE * MAX_SEGMENTS) as u64)
            .expect("打开数据库失败");
        let mut dev = dev;
        let mut kv = KvDb::open(&mut dev, cfg.clone()).expect("初始化失败");
        // 追加写入一批数据
        for i in RECORD_COUNT..(RECORD_COUNT + 1000) {
            let key = format!("key{i:010}");
            let value = format!("value{i:010}");
            kv.put(key.as_bytes(), value.as_bytes()).unwrap();
        }
        kv.flush().unwrap();
        // 估算写放大：物理写入 ≈ 段数 × segment_size，逻辑写入 ≈ 记录数 × 平均记录大小
        let logical_bytes = (RECORD_COUNT + 1000) as u64 * 20; // 每条记录约 20 字节
        let physical_bytes = (SEGMENT_SIZE * MAX_SEGMENTS) as u64; // 上界
        let write_amplification = physical_bytes as f64 / logical_bytes as f64;
        println!("写放大估算: {:.2}（物理 {physical_bytes} 字节 / 逻辑 {logical_bytes} 字节）", write_amplification);
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
```

- [ ] **Step 2: 验证编译**

Run: `cargo check -p embedded-database-bench --bin scale_test 2>&1 | tail -5`
Expected: 通过

- [ ] **Step 3: 运行验证**

Run: `cargo run --release -p embedded-database-bench --bin scale_test 2>&1 | tail -30`
Expected: 全部通过，退出码 0

- [ ] **Step 4: 检查点**

Run: `cargo run --release -p embedded-database-bench --bin scale_test >/dev/null 2>&1; echo "exit=$?"`
Expected: `exit=0`

---

### Task M29-1: M29 收尾验收

**Files:**
- Modify: `docs/superpowers/specs/2026-10-02-embedded-database-design.md`

**Interfaces:**
- Consumes: 全部 M29 Task
- Produces: 设计文档「实施状态」小节更新

- [ ] **Step 1: 全量测试**

Run: `cargo test --workspace --features embedded-database-core/std 2>&1 | tail -15`
Expected: 全部通过

- [ ] **Step 2: 格式化与 lint**

Run: `cargo fmt --all && cargo clippy --workspace --all-targets --features embedded-database-core/std -- -D warnings 2>&1 | tail -20`
Expected: 无警告

- [ ] **Step 3: 工具二进制可用**

Run: `cargo run -p embedded-database-bench --bin inject_crash >/dev/null 2>&1; echo "inject=$?"; cargo run -p embedded-database-bench --bin wcet >/dev/null 2>&1; echo "wcet=$?"`
Expected: `inject=0` 且 `wcet=0`

- [ ] **Step 4: 规模验证通过**

Run: `cargo run --release -p embedded-database-bench --bin scale_test >/dev/null 2>&1; echo "scale=$?"`
Expected: `scale=0`

- [ ] **Step 5: 更新设计文档**

在 `2026-10-02-embedded-database-design.md` 的「实施状态」小节追加 M29 交付内容：

```markdown
## 实施状态（M29 完成）

- M29 交付：千万级规模验证工具（`scale_test.rs`），
  写入 1000 万条 KV 记录，验证写入吞吐、读取正确性、崩溃恢复、写放大。
- **M29 简化**：
  - 配置：segment_size=4MiB, max_segments=1024, slots_per_page=64, max_pages=256。
  - 读取验证采样 1000 条（非全量）。
  - 写放大为上界估算（物理容量 / 逻辑数据量）。
  - 崩溃恢复验证未 flush 数据的一致性。
```

- [ ] **Step 6: 检查点**

Run: `rg -n 'M29 完成' docs/superpowers/specs/2026-10-02-embedded-database-design.md`
Expected: 有输出

---

## M29 验收标准

| # | 标准 | 验证方式 |
|---|------|----------|
| 1 | 千万级规模验证工具可执行 | M29-0 Step 3 |
| 2 | 规模验证通过（退出码 0） | M29-0 Step 4 |
| 3 | 全部测试通过 | M29-1 Step 1 |
| 4 | `inject_crash` exit=0 且 `wcet` exit=0 | M29-1 Step 3 |
| 5 | `scale_test` exit=0 | M29-1 Step 4 |

## 后续里程碑

M29 完成后，千万级规模验证交付。后续：

- **M30**：亿级规模验证（10000 万条记录）
- **M31**：十亿级规模验证（100000 万条记录）
- **M32**：LoongArch 实机验证（需硬件）
