# Spec 3b：embedded-database-flash 闪存介质后端（`RawFlash` + FTL + core 段槽环）

日期：2026-10-19 · 状态：待实施（设计已逐段确认） · 前置：Spec 1 / Spec 2 / Spec 3a 已完成（HEAD `0c93da8`）

## 1. 背景

设计文档把 `embedded-database-flash` 定义为「`no_std`：`BlockDevice` trait + NOR/NAND FTL + 磨损均衡」（`docs/superpowers/specs/2026-10-02-embedded-database-design.md:147`，当前标注「未交付，规划中」），并在 §5.3.4 规定 Flash 对齐、§8.4 规定磨损与写放大统计。同文档 `:279` 明确「磨损均衡（擦除块级）与坏块管理**下推给 `BlockDevice` 抽象（FTL）**」；`:769` 记录「FTL 与磨损均衡未实现」。

现状：**介质层完全空缺**。core 只提供一个最小的 `BlockDevice`（`size/read_at/write_at/sync`，`crates/embedded-database-core/src/device.rs:16-27`），`embedded-database-os` 只有文件后端 `FileDevice`（`crates/embedded-database-os/src/file.rs:16`）。core 的写入地址是**单调递增**的（`device_offset = (segment_seq-1) * segment_size`，`crates/embedded-database-core/src/db.rs:434-435`、`:708`），`active_segment` 只增不退（`db.rs:561`、`:762`），旧段在设备上长期滞留而**从不物理回收**——`db.rs:111` 的恢复扫描又只覆盖 `max_segments`（默认 4，`types.rs:180`）个槽。也就是说：core 目前既不能长期运行（写完 `max_segments` 段后地址越过设备容量、写入失败），也不会向介质层发出任何「这段作废了」的信号。

本 spec 的定位：**补齐介质层并让 core 的地址空间可持续**——交付一个可测试、可断言的 FTL（块级映射 + 磨损均衡 + 坏块），并给 core 加上与之配套的最小改动（`discard` 信号 + 段槽环 + 环感知恢复 + 按需自动合并）。

### 1.1 实测事实（2026-10-19，本机，实施前快照）

| 事实 | 证据 |
|---|---|
| `BlockDevice` 只有 `size/read_at/write_at/sync`，**无 erase/discard** | `crates/embedded-database-core/src/device.rs:16-27`；`lib.rs:63` |
| `InMemoryDevice::new` 已提供 `corrupt_at` / `inject_write_failure` 故障注入 | `device.rs:35`、`:71`、`:76`（用例见 `:195`、`:211`） |
| 恢复扫描遇「段头 CRC 解不出」即 `break`，其后段全部丢弃 | `crates/embedded-database-core/src/recovery.rs:52-77`，关键 `:70` `Ok(std::option::Option::None) => break` |
| 跨段 CRC 链校验用的是**盘上** `CommitRecord.prev_crc` | `recovery.rs:33-39`（注释）、`:58-65` |
| 恢复只扫 `0..cfg.max_segments`，第 0 槽装不下即 `InvalidArgument`，后续槽装不下则 `break` | `db.rs:111-138`（`:118-126`） |
| `Db::open` 置 `active_segment = last_valid_segment + 1` | `db.rs:140-141` |
| 打开时按「段序号 ≤ active-1」重建 `page_map` | `db.rs:145-152` |
| `flush` 段选择：可追加则复用当前段，否则用 `active_segment` 开新段 | `db.rs:387-431`；`active_segment` 递增 `:561` |
| `CompactReport { segments_merged, bytes_written, interrupted, write_amplification: f64 }`，目标 `< 2.0` | `db.rs:820-826` |
| `compact_with_budget` 把全部存活页重写到**新段**，写前判 `CapacityExhausted` | `db.rs:660`、`:701-708`、`:730`、`:737`、`:762` |
| `capacity` 类错误既有 5 处（`CapacityExhausted`） | `db.rs:407,486,537,737,806` |
| 既有用例 `B1_槽位耗尽后flush写入新段而非覆写满段` 断言第 9 次 flush 落在段 2 | `db.rs:2131-2158` |
| `Config { segment_size:161, max_segments:165(默认 180), .. }`，`validate` 只查非零 | `crates/embedded-database-core/src/types.rs:158-205` |
| core **自带** CRC-32C 实现并公开再导出（无第三方 CRC 依赖） | `crates/embedded-database-core/src/crc.rs:191-230`；`lib.rs:55` `pub use crc::{fold_crcs, hash_bytes, Crc32c, CrcChain};` |
| core 依赖仅 `zerocopy` + `bitflags`（无 `crc32fast`） | `crates/embedded-database-core/Cargo.toml` |
| `Error` 变体含 `Io` / `InvalidArgument` / `CapacityExhausted` / `Corruption{segment_seq}` | `crates/embedded-database-core/src/lib.rs:94-116` |
| no_std crate 惯例：`#![no_std]` + `#[cfg(any(test, feature="std"))] extern crate std;` + `mod imp` 门控 + `Placeholder` + `[lints.rust] unsafe_code="forbid"` | `crates/embedded-database-kv/src/lib.rs:1-17`、`crates/embedded-database-kv/Cargo.toml` |
| `embedded-database-os` 仅依赖 core，dev-dep 打开 `core/std` | `crates/embedded-database-os/Cargo.toml` |
| §5.3.4 要求「段大小 = 整数倍 erase block（建议 4MB）」「页大小 = erase block 的整数分之一（建议 4KB）」 | `docs/superpowers/specs/2026-10-02-embedded-database-design.md:274-279` |
| §8.4 要求「写放大 < 2.0」「擦除次数方差」可统计 | 同上 `:532-537` |
| R6 记录「FTL 裸块内存操作可能需要局部 unsafe」 | 同上 `:562` |
| CI `host` job 门禁：`fmt` / `clippy --workspace --all-targets --features embedded-database-core/std,embedded-database-embed/std -- -D warnings` / `test --workspace --features …`；`nostd` job 只 check `embedded-database-core`/`embedded-database-kv`/`embedded-database-embed` | `.github/workflows/ci.yml:13-18`、`:51-58` |
| README 状态行 313 测试；构建片段用 `--features embedded-database-core/std,embedded-database-embed/std` | `README.md:7`、`:16` |

## 2. 目标与范围

**目标**：交付 `embedded-database-flash`（NOR 模拟 + 擦除块级 FTL），并让 core 具备「段槽环 + 回收信号 + 环感知恢复 + 按需自动合并」的可持续地址空间，使得在 FTL 后端上运行的 `Db` 能长期写入且磨损可被观测、断言。

**范围内**

1. 新 crate `crates/embedded-database-flash`：`RawFlash` trait、`SimFlash`（内存 NOR 模拟 + 坏块/失败注入）、`Ftl`（块级 L2P + 磨损均衡 + 坏块 + `FtlStats`，实现 core `BlockDevice`）。
2. core 改动：`BlockDevice::discard` 默认空实现（`device.rs`）；`InMemoryDevice` 记录被通知区间；段槽环（`db.rs` 两处地址计算）；环感知恢复（`recovery.rs`）；按需自动合并与 `discard` 调用（`db.rs`）；`Config::validate` 加 `max_segments >= 3`（`types.rs`）；既有受影响用例改写。
2. `crates/embedded-database-os`：`FileRawFlash`（文件兼底 `RawFlash`）+ 文件闪存集成/掉电复验；新增依赖 `embedded-database-flash`（workspace 成员）。
3. `.github/workflows/ci.yml`：`nostd` job 的裸机目标构建新增 `embedded-database-flash`。
4. 文档如实化（设计文档文末追加「实施状态与勘误」）+ 账本记账 + M4/M11 遗留核销注记。

**范围外**

- 真实硬件驱动（SPI NOR 芯片时序、Linux MTD、DMA 对齐、ECC 引擎）；`RawFlash` 只有内存模拟与文件兼底两种实现。
- 页级 L2P、独立元数据日志 / 专用坏块表块、后台/异步垃圾回收。
- `Db::discard` 之类的用户级回收 API（回收只由 core 内部在段退役时触发）。
- `embedded-database-embed` 的任何改动（它是 core 存储 API 的门面，与介质层不交叉）。
- 断电重放 / 日志文件系统级语义：只保证「**已提交**段与块可读」，不保证未提交写入的任何可见性。
- 多进程文件锁；`supply-chain/`、`deny.toml`、`scripts/`（Spec 2 产物，本 spec 不触碰）。

## 3. 设计

### 3.1 架构与组件边界

```
embedded-database-os (std)                       embedded-database-flash (no_std + alloc)
  FileDevice : BlockDevice               RawFlash  (trait)   ← 裸介质面
  FileRawFlash : RawFlash  ────────────►  SimFlash  (内存模拟)
                    │                      Ftl<R> : BlockDevice  ← 映射/均衡/坏块
                    │                          │
                    └──────────────► core BlockDevice trait ◄─── embedded-database-core (no_std)
```

- **依赖方向**：`embedded-database-flash → embedded-database-core`（只用 `BlockDevice` trait、`Error`/`Result`、`Crc32c`）；`embedded-database-os → embedded-database-flash`（文件兼底设备 + 集成测试）。`flash` 不依赖 `os`，core 不依赖任何一方。
- **复用 core 的 `Crc32c`**（`crates/embedded-database-core/src/crc.rs:191`，`lib.rs:55` 已再导出）⇒ **零新增第三方依赖**（`crc32fast` 无需引入）。
- `embedded-database-flash` 不设 `std` 特性：FTL 真 `no_std + alloc`，只有测试需要 host `std`。

### 3.2 core：`BlockDevice::discard`

```rust
/// 通知设备：`[offset, offset+len)` 的数据已作废，可被回收。
///
/// 语义：
/// - 调用后**不保证**该范围仍读到旧值（实现可返回任意值）。
/// - 默认空实现（`Ok(())`）：内存/文件后端不需要回收，行为不变。
/// - 越界 ⇒ `InvalidArgument`；非对齐、重复调用、`len == 0` 均不得 panic。
fn discard(&mut self, offset: u64, len: u64) -> Result<(), Error> { Ok(()) }
```

- 加在 `device.rs` 的 `BlockDevice` trait 上（`:16-27` 内），**默认实现 ⇒ 向后兼容**：`InMemoryDevice`、`FileDevice`、既有测试设备无需改动即编译通过。
- `InMemoryDevice` 额外记录被通知的区间（`discarded_ranges()`）供断言，**不改变**数据内容（避免把「现在能读到」变成隐性契约）。
- FTL 侧对非对齐区间**宽松处理**：只回收被完整覆盖的擦除块，部分覆盖的块保留（不报错）。core 发出的区间按 §3.5 恒为段对齐 ⇒ 在 `segment_size` 为擦除块整数倍时即擦除块对齐。

### 3.3 core：段槽环

- 新段/合并段的设备偏移改为**取模**：
  - `db.rs:434`：`device_offset = ((segment_seq - 1) % max_segments) * segment_size`
  - `db.rs:708`（`compact_with_budget`）：同一公式。
- `active_segment` 仍**单调递增**（段号不回卷，`segment_seq` 是全局逻辑序号；槽位只是它的物理落点）。`segment_seq → slot` 为 `(seq-1) % max_segments`。
- 设备容量约束不变：设备至少容纳 `max_segments × segment_size`（首槽装不下即 `InvalidArgument`，`db.rs:118-126`）。
- `Config::validate` 新增 `max_segments >= 3`（`types.rs:199` 一带）。理由见 §3.5：环需要「合并目标空闲槽」这一不变量，`N = 2` 时无法维持。默认值 4 不受影响；仓库无任何测试使用 `max_segments < 4`（实测：`rg 'max_segments:\s*[0-9]+' crates/` 在 core 测试中无命中）。

### 3.4 core：环感知恢复

`recovery::recover`（`recovery.rs:40-78`）改三处：

1. **扫全槽、跳过空槽**：`Ok(None) => continue`（原 `:70` 为 `break`）。段头 CRC 解不出（未写过或已被擦除的 0xFF 槽）⇒ 跳过，不再终止扫描。
2. **按段号排序**：收集各槽的 `(segment_seq, commit_crc, disk_prev_crc, device_offset)`，按 `segment_seq` **升序**排序后再走链校验（槽位顺序 ≠ 段号顺序：槽 = `seq mod max_segments`）。
3. **链规则按段号紧邻性生效**：仅当 `seq_cur == seq_prev + 1` 时校验 `disk_prev_crc == prev_crc`；`seq` 有跳变处视作**链起点**（不校验，重置累计值）。不符仍返回 `Error::Corruption { segment_seq }`。
4. `RecoveryReport` 增 `first_valid_segment: Option<u64>`（供 `Db` 计算存活区间）；`last_valid_segment` / `last_commit_crc` / `corrupted_segment` 语义不变。

**为什么不需要新增「链重置标记」**：§3.5 的不变量保证存活段号**始终连续**（回收只发生在前缀），跳变只可能出现在「被回收的前缀」与「第一个存活段」之间——而第一个存活段本来就没有前驱，链校验自然从它开始。因此段头格式**零变更**。

**一处必须说明的语义放松（不做隐藏）**：旧实现遇到「段头 CRC 解不出」就 `break`，其后所有段全部不可见（静默数据丢失、无错误）。新实现跳过该槽继续。因此**非前缀位置**出现空槽/坏段头时，后续段仍会被接受（不再连带丢弃），代价是丢掉了「其后一律视为未提交」这条保守边界。已知后果：中段段头损坏时，该段页面对 core 静默不可见（旧实现同样不可见，只是连其后数据也一起丢），两者都不报 `Corruption`（`Corruption` 只在**段头可解**却 `prev_crc` 不符时触发）。取舍理由：前缀回收是**常规路径**，必须支持；中段头损坏是异常路径，两种处理都属「静默丢失」，但新版不牵连后续已提交数据。§5.5 配对照用例固定该语义。

**强度不退化**（对照 §5.5）：段内数据损坏仍由页 CRC 拦下；跨段篡改仍由 `prev_crc` 链拦下（要求段号紧邻，而合法回收只产生前缀跳变）；真报错路径（`Corruption`）覆盖范围不变。

### 3.4.1 `Db::open` 的配套计算

`db.rs:140-152` 处：`active_segment = last_valid_segment + 1` 不变；新增存活区间 `[first_valid, last_valid]`（空设备时 `None`），供 §3.5 判断。`page_map` 重建逻辑（`:145-152`）改为遍历**存活区间内**的段，不再依赖「`segment_seq ≤ active-1`」与槽位一一对应。

### 3.5 core：按需自动合并（不变量：存活段 ≤ `max_segments - 1`）

**不变量**：任意时刻（含崩溃恢复后）存活段数 `L = last_valid - first_valid + 1 ≤ max_segments - 1`。

- **写入前判定**：在 `flush` 与 `compact_with_budget` 决定「开新段」之前，若 `L == max_segments - 1`，**先跑整段合并**，再写新段。
- **合并目标槽必然空闲**：合并产生的段号 `M = active_segment`，其槽为 `(M-1) % max_segments`；占用该槽的只可能是段号 `M - max_segments < first_valid`（已死）⇒ 空闲。`max_segments ≥ 3` 是「合并后仍有槽可写新段、且始终留 1 个空闲槽」的充分条件。
- **合并后回收**：合并**提交成功后**，对 `[first_valid, last_valid]` 的**每一段**调用 `dev.discard(该段 device_offset, segment_size)`，随后存活区间变为 `[M, M]`（`L = 1`）。
- **顺序与崩溃安全**：先写合并段并提交，再 `discard` 旧段。崩在 merge 完成前 ⇒ 旧段未动、链完整，重开读到合并前状态；崩在若干次 `discard` 之间 ⇒ 恢复按 §3.3/§3.4 跳过空槽继续，读到合并后的段。
- **真容量不足仍报错**：设备装不下 `max_segments × segment_size`（`db.rs:118-126` 语义）或逻辑数据超过一段（`db.rs:737`）⇒ `CapacityExhausted` 不变。
- **行为变化**：此前「写完 `max_segments` 段后写入失败」变成「自动合并后继续写」。因此依赖该失败路径或用例需改写（§5.5 列出 `B1` 等）。

### 3.6 flash：几何与块头布局

- 物理：擦除块 4096B / 页 256B（16 页/块）。**第 0 页 = 块头页**（仅前 32B 有效），**第 1..15 页 = 逻辑载荷 3840B**。
- `logical_block_size = 3840`；`logical_size = logical_blocks × 3840`；逻辑块 `L` 的第 `p` 页 ↔ 物理块第 `p+1` 页。
- **OP 20%**（`FtlConfig.op_percent = 20`）：`logical_blocks = floor(block_count × 4 / 5)`（= 1 : 1.25），其余物理块构成共享空闲池（磨损均衡 + 坏块兼顾）。
- 块头（32B，LE，CRC 覆盖前 28B）：

| 偏移 | 字段 | 大小 | 说明 |
|---|---|---|---|
| 0 | `magic` | 4 | `b"WHFT"` |
| 4 | `version` | 1 | 格式版本，初始 1 |
| 5 | `state` | 1 | `1 = DATA` |
| 6 | `_reserved` | 2 | 0 |
| 8 | `logical_block` | 4 | 逻辑块号 |
| 12 | `alloc_seq` | 8 | 单调分配号（最新胜） |
| 20 | `erase_count` | 4 | 编程时刻的该块擦除次数 |
| 24 | `_reserved` | 4 | 0 |
| 28 | `crc32c` | 4 | `Crc32c` over `[0,28)` |

- **块头最后编程（页 0 在数据之后写）＝提交点**：崩在数据中途 ⇒ 块头 CRC 不成立 ⇒ 该块不可用，旧块仍是有效副本。
- RAM 表：每物理块 `{ state: Free | Data | Dirty | Bad, logical, alloc_seq, erase_count, next_free_page }`（≈24B/块）。
- **几何校验**（`Ftl::new` / `FtlConfig` 校验）：`page_size` 为 2 的幂且 `≥ 64`；`erase_block_size % page_size == 0`；`erase_block_size / page_size ≥ 2`；`block_count ≥ 4` 且 `logical_blocks ≥ 2`。不满足 ⇒ `Error::InvalidArgument`。
- **core 侧 `segment_size` 建议值**：必须同时为 3840 与 4096 的整数倍 ⇒ `segment_size = 3840 × 1024 = 3_932_160`（= 1024 逻辑块 = 960 擦除块）。设计文档 §5.3.4「建议 4MB」在 flash 后端下改记为「按 FTL 逻辑块对齐选值」（§8 勘误）。

### 3.7 flash：写路径与块级搬迁

`Ftl::write_at(offset, len)`：

1. 按 `logical_block_size` 拆分（一次调用可跨多个逻辑块）。
2. 逻辑块无映射 ⇒ 分配空闲块：**从空闲池取 `erase_count` 最小者**（动态均衡），先写数据、最后写块头；`alloc_seq` 递增。
3. 目标字节当前为 `0` 而本次要写 `1`（NOR 语义下 `program` 会失败）⇒ **块级搬迁**：分配新块 → 拷整块载荷 → 应用本次写 → 写块头（新 `alloc_seq`）→ 旧块置 `Dirty` 并擦除回收。
4. `program`/`erase` 返回硬错误（非 `0→1` 冲突）⇒ 先尝试同逻辑块的搬迁（可能只是该块变坏）；搬迁也失败 ⇒ 标记该物理块 `Bad`，用副本恢复；仍不行才上抛（§3.10）。

`Ftl::read_at(offset, len)`：按 L2P 查物理块，读对应页；未映射的逻辑块返回全 `0xFF`（等于「已擦除」，与 core 冷启动语义一致）。

`Ftl::size()`：`logical_size`。

### 3.8 flash：`discard`、磨损均衡、坏块

- `discard(offset, len)`：对每个被**完整覆盖**的逻辑块 —— 若有映射则退役物理块（→ 擦除 → `erase_count += 1` → 回空闲池）；部分覆盖的块**不动**（不报错）；越界 ⇒ `InvalidArgument`；重复调用幂等。
- **动态均衡**：分配恒取 `erase_count` 最小空闲块。
- **静态均衡**：每完成 `K` 次擦除**或** `erase_count_max - erase_count_min > 阈值` 时，搬移 1 个冷块（`erase_count` 最小的 `Data` 块，整块载荷复制到高计数的空闲块后退役旧块），每次扫描最多搬 1 块以限成本。`K` 与阈值作为 `FtlConfig` 常量（默认 `K = 64`、阈值 `2`），测试可覆盖。
- **坏块**：读/写/擦返回硬错误 ⇒ 标 `Bad` 永久退役；容量损失由 OP 池吸收；空闲池耗尽 ⇒ `Error::CapacityExhausted`。

### 3.9 flash：挂载与运行期恢复

- `Ftl::new(raw, cfg)`：扫描全部物理块 → 解块头。规则：
  - CRC 有效 ⇒ 记录 `(logical, alloc_seq, erase_count)`；
  - **同一 `logical_block` 多个有效块头 ⇒ `alloc_seq` 最大者胜**，其余置 `Dirty` 优先回收；
  - 全 `0xFF` 或 CRC 无效且非全 `0xFF` ⇒ `Free`（用前必先擦）；读失败 ⇒ `Bad`。
- 用有效块头重建 L2P 与空闲池（无需独立元数据区）。
- **已知限制（掉电在「擦除后、再次编程前」）**：该块的 `erase_count` 随块头一起丢失。挂载时对全 `0xFF` 块保守取 `ec = 观测到的 max_ec`（视作最旧、不优先使用），最坏每次掉电低估 1 次擦除。该保守规则保证「均衡仍收敛」且不会反复优先磨损同一个块。
- 运行期：`sync()` 透传到 `RawFlash::sync()`；core 的 `BlockDevice::sync` 返回后，此前 `write_at` 的字节必须持久（`FileRawFlash` 必须先 `sync_all`）。

### 3.10 flash：公开 API 与错误映射

```rust
// raw.rs
pub trait RawFlash {
    fn page_size(&self) -> usize;
    fn erase_block_size(&self) -> usize;
    fn block_count(&self) -> u32;
    fn read(&self, block: u32, offset: usize, buf: &mut [u8]) -> FlashResult<()>;
    fn program(&mut self, block: u32, offset: usize, data: &[u8]) -> FlashResult<()>;
    fn erase(&mut self, block: u32) -> FlashResult<()>;
    fn sync(&mut self) -> FlashResult<()>;
}
pub enum FlashError { OutOfRange, Unaligned, Program { block: u32 }, Erase { block: u32 }, Read { block: u32 }, BadBlock { block: u32 } }
pub type FlashResult<T> = core::result::Result<T, FlashError>;

// sim.rs
pub struct SimFlash;   // new(blocks, layout) / mark_bad(block) / inject_program_failure(after_n)
                       // / inject_erase_failure(after_n) / program_count() / erase_count_of(block)
                       // / is_bad(block) / snapshot(&mut [u8])
// ftl.rs
pub struct FtlConfig { pub op_percent: u8, pub static_level_interval: u32, pub wear_gap_threshold: u32 }
pub struct Ftl<R: RawFlash> { /* … */ }        // new(raw, cfg) -> Result<Self>
impl<R: RawFlash> Ftl<R> { pub fn logical_size(&self) -> u64; pub fn stats(&self) -> FtlStats; }
pub struct FtlStats {
    pub logical_write_bytes: u64, pub physical_program_bytes: u64,
    pub write_amplification: f64, pub erase_ops: u64,
    pub erase_count_min: u32, pub erase_count_max: u32,
    pub block_moves: u64, pub bad_blocks: u32, pub free_blocks: u32, pub retired_blocks: u32,
}
impl<R: RawFlash> BlockDevice for Ftl<R> { size / read_at / write_at / sync / discard }
```

- `Ftl` 为泛型（无 `dyn` 需求）；core 侧仍以 `&mut dyn BlockDevice` 接收。
- 错误映射：可自愈的 `Program/Erase/Read/BadBlock` 先走坏块退役或搬迁，救不回才上抛 —— `FlashError::OutOfRange` → `Error::InvalidArgument`；`Unaligned` 与其余硬错误 → `Error::Io`；空闲池/OP 耗尽 → `Error::CapacityExhausted`。库代码绝不 panic（C4）。
- `#![no_std]` + `extern crate alloc;` + `[lints.rust] unsafe_code = "forbid"`（R6 的结果：**无需局部 unsafe**——FTL 只用 `alloc::vec::Vec` 与 `core`，`RawFlash` 的实现细节留在实现方）。

### 3.11 os：`FileRawFlash`

- `FileRawFlash::open(&Path, block_count, layout) -> Result<Self>`：等长预分配（`set_len`），块/页寻址 = 文件偏移；`program` 拒绝 `0→1`（读改写前先校验）；`erase` 写入 0xFF 区间；`sync` = `File::sync_all`。
- `fail_after(n)`：第 n 次 `program`/`erase` 后开始返回错误并拒绝后续写（模拟断电截止点）。
- 单进程语义（与 `FileDevice` 口径一致，不做跨进程锁）。
- os 的集成测试直接 `Ftl::new(FileRawFlash::open(...)?, FtlConfig::default())`，再在其上开 `Db`。

## 4. 掉电安全与恢复

**提交点**：core = 段的 `CommitRecord`（既有语义不变）；FTL = 块头编程成功且 CRC 通过。

**不变式**：旧物理块在**新块块头提交前**绝不擦除；旧段在**合并段提交后**才被 `discard`。⇒ 任一时刻至少一份有效副本，已提交数据掉电不丢。

**两层恢复彼此独立**：FTL 恢复「逻辑块 → 物理块」，core 恢复「段/提交链」；层间只有 `discard` 单向信号，FTL 不回传状态 ⇒ 搬迁/均衡/搬块对 core 完全透明。

**掉电点矩阵**（确定性枚举）：

| # | 掉电点 | 期望 |
|---|---|---|
| 1 | `program` 中途（任意数据页、`[0,15]` 页） | 该块无有效块头 ⇒ 旧副本仍可读；无 `Corruption` |
| 2 | 块头页（页 0）编程前后 | 前 ⇒ 用旧副本；后 ⇒ 认为新数据已提交 |
| 3 | `erase` 中途 | 擦除幂等 ⇒ 该块 `Free`（用前再擦），L2P 不指向它 |
| 4 | 若干次 `discard` 之间（core 已提交合并段） | 重开读到合并后状态；空槽被跳过 |
| 5 | core `flush` 写入被截断 | 既有恢复语义：丢未提交段 |
| 6 | core 自动合并写入被截断（**最关键**） | 旧段尚未 `discard` ⇒ 重开读合并前全量，无 `Corruption` |

每点断言：已提交键全可读、无 `Corruption`、FTL 不变量 `free + data + dirty + bad == block_count` 成立。

## 5. 测试策略与验收标准

测试位置沿用仓库惯例：**全部写在 `src/` 内 `#[cfg(test)] mod tests`**（本仓无 `crates/*/tests/`）。`embedded-database-flash` 加 `[dev-dependencies] embedded-database-core = { workspace = true, features = ["std"] }`（resolver v2 统一 dev 特性）⇒ 可在 flash 内直接跑 `Db × Ftl` 集成测试。

### 5.1 `raw` / `sim` 单元测试

写后回读；**`0→1` 被拒**；同值重写成功；越界 `OutOfRange`；页未对齐写入 `Unaligned`；`erase` 幂等且回 0xFF；几何访问器自洽；`mark_bad` 后三类操作返回 `BadBlock`；`inject_*_failure(n)` 按次数生效；`snapshot` 与读路径一致。

### 5.2 FTL 单元测试（内存 `SimFlash`）

空设备挂载；跨页/跨逻辑块/跨擦除块写读回读；**L2P 重建**（写入 → 丢弃 `Ftl` → 同一 `SimFlash` 重新 `Ftl::new` → 数据一致）；同块二次写触发搬迁（`block_moves > 0`、`erase_ops` 增长、读到新值）；`discard` 整块 ⇒ `free_blocks` 增长且该块随后被复用；`discard` 非对齐 / 重复 ⇒ 幂等且无副作用；`discard` 越界 ⇒ `InvalidArgument`；**不变量** `free + data + dirty + bad == block_count`（固定 seed 的确定性随机操作序列，每步断言）。

### 5.3 磨损与写放大（可断言阈值，常量集中定义）

- **A3 磨损**：均匀写循环（≥ 500 次擦除操作）后 `erase_count_max - erase_count_min ≤ 2`；并与「无均衡基线」（测试内实现：按物理序号轮转分配）的方差对比，断言显著更小。
- **静态均衡**：构造常驻冷块 + 持续热写 ⇒ 断言发生搬移（`block_moves > 0`）且擦除次数差回落到阈值内。
- **A4 写放大**：顺序写场景 `write_amplification ≤ 1.5`；含静态搬移场景 `≤ 3.0`（与 §8.4 目标 `< 2.0` 的关系在文档中说明：顺序主路径满足 §8.4，均衡搬移场景放宽）。

### 5.4 掉电矩阵测试

§4 表 1–6 全点。FTL 层用 `SimFlash` 确定性枚举；core × FTL 集成用 `Ftl<SimFlash>` 跑 `Db`（`flush` 截断、自动合并截断、`discard` 后崩溃）；文件设备（os）抽点复验（复制文件 → 截断 → 重开 `Ftl<FileRawFlash>` + `Db`）。

### 5.5 core 新增/改动测试

- `discard` 默认空实现：`InMemoryDevice` 记录区间，断言合并后旧段区间被通知；其余设备静默无副作用。
- 环：连续写 `> max_segments` 段不失败；槽位占用符合 `(seq-1) % max_segments`；恢复跨槽识别。
- **恢复跳过空槽**：段 1 擦成 0xFF、段 2/3 有效 ⇒ 重开读到段 3（替代旧 `break` 语义）。
- **非前缀空洞语义（对照用例）**：把**中间**某段的段头擦成 0xFF，段号在它两侧仍连续 ⇒ 新版「跳过该段并接受其后段」；同时保留一条**旧语义对照**断言（若回退到 `break`，其后段数据不可见）——把 §3.4 的取舍固定成可执行契约。
- **链强度不退化**：段号相邻但盘上 `prev_crc` 被篡改 ⇒ 仍报 `Corruption`（与「段号跳变不报错」互为对照）。
- 自动合并：存活段恒 `≤ max_segments - 1`（含「合并写一半崩溃」注入）；`Config::validate` 拒 `max_segments < 3` 且 `= 3` 可用。
- 改写依赖段号线性推进/设备耗尽的既有用例（至少 `B1_槽位耗尽后flush写入新段而非覆写满段`，`db.rs:2131`）：改为断言「不覆写存活段 + 环规则 + 重开全键可读」。

### 5.6 验收判据

1. **A1 正确性**：内存与文件两种 `RawFlash` 上，`Db` 全键回读 + 重开一致；`Ftl` 的 L2P 重建一致。
2. **A2 掉电**：§4 矩阵 1–6 全点通过（已提交数据不丢、无 `Corruption`、FTL 不变量成立）。
3. **A3 磨损**：≥ 500 次擦除操作后 `erase_count_max - erase_count_min ≤ 2`，且优于无均衡基线。
4. **A4 写放大**：顺序写 `≤ 1.5`，含均衡搬移 `≤ 3.0`。
5. **A5 坏块**：注入坏块后数据仍可读，`bad_blocks` 计入 `FtlStats`，容量损失由 OP 吸收。
6. **A6 门禁**：`cargo fmt --all --check`、`cargo clippy --workspace --all-targets --features embedded-database-core/std,embedded-database-embed/std -- -D warnings`、`cargo test --workspace --features embedded-database-core/std,embedded-database-embed/std` 全绿；`cargo test -p embedded-database-flash`（无特性）通过；`cargo test -p embedded-database-embed` 不回归；CI `nostd` job 新增 `embedded-database-flash` 后 5 目标全部通过；无 `unsafe`（CI 扫描不变）。
7. **A7 零新增第三方依赖**：`Cargo.lock` 只新增 `embedded-database-flash` 本包与 `embedded-database-core`/`embedded-database-os` 之间的 workspace 成员边；第三方包集合不变。
8. **A8 提交序列**：每个逻辑单元一提交、中文提交信息、`git status` 干净。

## 6. 执行顺序（供实施计划参考）

1. **T1** `embedded-database-flash` 骨架（`Cargo.toml` + `raw.rs` + `sim.rs` + 单元测试）+ CI `nostd` job 接线。
2. **T2** FTL 基础：几何/块头/L2P/挂载扫描/`read_at`/`write_at`（无回收）→ 基础单元测试。
3. **T3** FTL 回收与磨损：`discard`、块级搬迁、动态+静态均衡、`FtlStats` → 磨损/写放大/不变量测试。
4. **T4** core：`BlockDevice::discard` 默认方法 + `InMemoryDevice` 记录 + 用例。
5. **T5** core：段槽环（`db.rs:434`/`:708`）+ 环感知恢复（`recovery.rs`）+ `Db::open` 存活区间 + 用例。
6. **T6** core：按需自动合并 + `discard` 调用 + `Config::validate`（`max_segments ≥ 3`）+ 改写 `B1` 等用例。
7. **T7** os：`FileRawFlash` + 文件后端集成/掉电复验。
8. **T8** 文档如实化（含 §5.3.4 勘误）+ 账本 + M4/M11 核销注记 + 终验。

每步一提交（T2/T3 内部按同一逻辑单元再细分）。

## 7. 风险与错误处理

| 风险 | 影响 | 缓解 |
|---|---|---|
| core 语义改动面大（环 + 恢复 + 自动合并） | 现有恢复语义回归 | 逐逻辑单元提交；每个不变量有专门用例；掉电矩阵含「合并截断」；链强度有对照用例 |
| 阈值脆弱（磨损差、写放大） | 测试噪声导致失败 | 阈值集中为常量并注明依据；补「相对无均衡基线」比较兜底 |
| 自动合并增加同步读写 | 写路径延迟 | 复用既有同步整段合并路径，不引入新内存峰值；文档记录 WA/耗时与 `max_segments` 的关系 |
| 擦除后未编程块的 `erase_count` 丢失 | 均衡计数偏差 | 保守取 `max_ec`；用例覆盖「擦除后掉电」，断言不变量与均衡仍收敛 |
| `segment_size` 与 FTL 逻辑块对齐 | 配置错误导致 layout 断言失败 | 文档给出建议值（3,932,160）；FTL 对非对齐 `discard` 宽松处理，不在运行期 panic |
| R6（原本担心 FTL 需局部 unsafe） | 供应链叙事 | 设计落地为**零 unsafe**；设计文档 R6 记「无需 unsafe」 |

## 8. 文档同步与账本

- `docs/superpowers/specs/2026-10-02-embedded-database-design.md`（**不改历史正文**，文末追加「实施状态与勘误」，与 Spec 3a 口径一致）：
  - §5.2 `:147` flash 行 → 已交付（描述改为 `RawFlash` + 擦除块级 FTL + 磨损均衡 + 坏块）；
  - §5.3.3（段/CRC 链）→ 勘误：恢复支持「跳过空槽 / 段号跳变视作链起点」+ 槽位环；
  - §5.3.4 `:274-279` → `segment_size` 由「建议 4MB」改记「按 FTL 逻辑块对齐（例：3,932,160 B）」；
  - §8.4 `:532-537` → 磨损/写放大由 `FtlStats` 落地；
  - R6 `:562` → 记「FTL 无需 unsafe」；
  - `:769` / M4/M11 的「FTL 与磨损均衡未实现」「旧段物理擦除留待后续」→ 标注由本 spec 收口。
- `README.md`：状态行测试计数；如新增构建/后端说明则补一节。
- `project-summary` / `delivery-checklist` / `certification-template` / `operations-manual`：仅对提及 flash/介质后端的行做如实化。
- `.superpowers/sdd/progress.md`：本 spec 记账行 + M4/M11 核销注记。

## 9. 与其它 spec 的关系

- **与 Spec 3a（embed 门面）**：无依赖。3a 的缝在 `crates/embedded-database-embed/`，3b 的缝在 `crates/embedded-database-flash/`、`crates/embedded-database-os/`、`crates/embedded-database-core/src/{device,db,recovery,types}.rs`；embed 不新增类型（见 `2026-10-19-embedded-database-embed-public-api-design.md` §8）。
- **与 Spec 1（工程基线）/ Spec 2（供应链审计）**：本 spec 不触碰 `supply-chain/`、`deny.toml`、`scripts/`；新增依赖仅 workspace 成员（`embedded-database-flash`、`embedded-database-os → embedded-database-flash`），第三方包集合不变 ⇒ 不影响 Spec 2 的审计范围。
- **与设计文档 M4/M11 遗留**：M4 交付的是 Linux 文件后端（已完成的记录见 `docs/superpowers/plans/2026-10-02-embedded-database-m4-flash-backend.md`），M11/M12 遗留的「旧段不物理回收」由本 spec 的 `discard` + 段槽环收口。

## 10. 勘误与实施注记（Task 8）

- **勘误（§3.8 / 坏块语义）**：原稿「坏块用副本恢复」不成立——本设计无冗余副本。
  坏块的实际保证只有两条：**分配时避开**（挂载扫描将坏块标记为 `Bad`，不再分配）
  + **编程/擦除失败即退役**（搬迁有效页后把该物理块标记为 `Bad`）。
  已提交数据所在的块若介质失效，数据即丢失；该丢失由 core 的段 CRC / 提交链在
  恢复期检出为 `Corruption`（显式失败），不静默返回脏数据。
- **集成测试与计划勘误（Task 8）**：`core_db_建立在_ftl_之上可写满多段并重开`
  在计划样例的默认 `Config`（`max_pages = 8`）下 60 个不同键必然超出 4 槽环容量
  （合并装不下 ⇒ 环回覆写淘汰最旧段），改为 `max_pages = 15`（60 = 4 × 15，
  恰好落满 4 个存活段、不触发覆写）。
- **实施期 core 缺陷修复（Spec 3b Task 8 回归驱动）**：
  1. `SegmentWriter::new` 清零 PageRun 表预留区——槽位复用时陈旧表条目会让
     `Db::open` 的 page_map 重建登记指向已覆写区域的幽灵页（crates/embedded-database-core/src/writer.rs）；
  2. `compact_into` 退役段 discard 跳过与合并新段槽位冲突的偏移（避免清掉刚写好的合并段）；
  3. `flush` 环回覆写最旧槽时经 `evict_overwritten_segment` 显式摘除被覆写段的
     `page_map`/`key_map` 映射——旧键读取降级为 `Ok(None)`，不再留下指向被覆写
     区域的悬空引用（否则读报 `CorruptedPage`）。
