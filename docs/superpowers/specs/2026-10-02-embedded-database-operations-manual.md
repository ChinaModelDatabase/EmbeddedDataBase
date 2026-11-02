# embedded-database 操作手册

> 版本：M32（2026-10-05）
> 状态：294 测试全绿，0 提交
> 适用版本：embedded-database v0.1.0

---

## 1. 快速开始

### 1.1 环境要求

| 项 | 要求 |
|----|------|
| Rust | 1.81.0+（`rust-toolchain.toml` 已锁定） |
| 目标 | aarch64-unknown-linux-gnu / loongarch64-unknown-linux-gnu / loongarch64-unknown-linux-musl / x86_64-unknown-linux-gnu / loongarch64-unknown-none |
| 依赖 | 仅 `zerocopy` + `bitflags`（运行时） |

### 1.2 构建

```bash
# 设置工具链（macOS）
export PATH="/opt/homebrew/opt/rustup/bin:$PATH"

# 构建
cargo build --release

# 测试
cargo test --workspace --features embedded-database-core/std

# 格式化 + lint
cargo fmt --all
cargo clippy --workspace --all-targets --features embedded-database-core/std -- -D warnings
```

### 1.3 第一个程序

```rust
use embedded_database_core::{Config, InMemoryDevice};
use embedded_database_kv::KvDb;

fn main() {
    // 1. 创建内存设备（生产环境用 FileDevice）
    let mut dev = InMemoryDevice::new(1 << 20); // 1 MiB

    // 2. 打开数据库
    let mut kv = KvDb::open(&mut dev, Config::default()).expect("打开失败");

    // 3. 写入
    kv.put(b"hello", b"world").expect("写入失败");

    // 4. 读取
    let value = kv.get(b"hello").expect("读取失败");
    assert_eq!(value, Some(b"world".to_vec()));

    // 5. 落盘
    kv.flush().expect("flush 失败");
}
```

### 1.4 文件后端

```rust
use embedded_database_core::Config;
use embedded_database_kv::KvDb;
use embedded_database_os::FileDevice;

fn main() {
    // 打开文件设备（100 MiB）
    let dev = FileDevice::open(std::path::Path::new("my.db"), 100 << 20)
        .expect("打开文件失败");
    let mut dev = dev;

    // 打开数据库
    let mut kv = KvDb::open(&mut dev, Config::default()).expect("初始化失败");

    // 写入
    kv.put(b"key", b"value").expect("写入失败");
    kv.flush().expect("flush 失败");
}
```

---

## 2. KvDb API

### 2.1 方法总览

| 方法 | 签名 | 说明 |
|------|------|------|
| `open` | `KvDb::open(dev, cfg) -> Result<KvDb>` | 打开数据库 |
| `put` | `kv.put(k, v) -> Result<()>` | 写入键值对（单条 commit+flush） |
| `put_batch` | `kv.put_batch(items) -> Result<()>` | 批量写入（一次 commit+flush） |
| `get` | `kv.get(k) -> Result<Option<Vec<u8>>>` | 读取键值（未命中返回 `None`） |
| `flush` | `kv.flush() -> Result<()>` | 落盘 |
| `scan` | `kv.scan(prefix) -> Result<Vec<(String, Vec<u8>)>>` | 前缀扫描 |

### 2.2 put vs put_batch

```rust
// 单条写入：每条都 commit+flush，适合低频写入
kv.put(b"key1", b"value1")?;

// 批量写入：一次 commit+flush，吞吐 5.3×，适合高频写入
let items = [
    (b"key1".as_ref(), b"value1".as_ref()),
    (b"key2".as_ref(), b"value2".as_ref()),
];
kv.put_batch(&items)?;
```

**性能对比**（M29-1/M30 实测）：
- `put`：~23690 条/秒
- `put_batch`：~124933 条/秒（5.3×）

### 2.3 scan 前缀扫描

```rust
// 扫描所有以 "users:" 开头的键
let results = kv.scan("users:")?;
for (key, value) in results {
    println!("{key}: {:?}", value);
}
```

---

## 3. SQL CLI

### 3.1 启动

```bash
cargo run --release -p embedded-database-cli
```

### 3.2 支持的 SQL 语句

| 语句 | 示例 |
|------|------|
| CREATE TABLE | `CREATE TABLE users (id INT, name TEXT)` |
| INSERT | `INSERT INTO users (id, name) VALUES (1, 'alice')` |
| SELECT | `SELECT id, name FROM users` |
| WHERE | `SELECT * FROM users WHERE id > 5` |
| JOIN | `SELECT * FROM a JOIN b ON a.id = b.id` |
| GROUP BY | `SELECT dept, COUNT(*) FROM emp GROUP BY dept` |
| HAVING | `SELECT dept, COUNT(*) FROM emp GROUP BY dept HAVING COUNT(*) > 10` |
| ORDER BY | `SELECT * FROM users ORDER BY id DESC LIMIT 10` |
| DROP TABLE | `DROP TABLE users` |

### 3.3 CLI 命令

| 命令 | 说明 |
|------|------|
| `.exit` / `.quit` | 退出 |
| SQL 语句 | 执行并打印结果 |

### 3.4 SQL 简化边界

- 无优化器/子查询
- 比较非类型感知（字符串化比较）
- GROUP BY 仅单列生效
- JOIN 仅 INNER，ON 仅 `=`

---

## 4. Config 配置

### 4.1 字段说明

| 字段 | 默认值 | 说明 |
|------|--------|------|
| `segment_size` | 1 MiB | 段大小（字节） |
| `page_size` | 4096 | 页大小（字节） |
| `slots_per_page` | 4 | 每页槽数 |
| `max_pages` | 8 | 每段最大页数 |
| `max_segments` | 4 | 最大段数 |
| `arena_pool_size` | 2 | Arena 池大小 |
| `integrity` | `Commit` | 完整性级别 |
| `compaction` | 默认 | Compaction 预算 |
| `encryption_key` | `None` | 加密密钥（SM4） |

### 4.2 几何约束

```
append_start = SEGMENT_HEADER_SIZE(56) + max_pages × page_size × slots_per_page
必须 < segment_size − COMMIT_RECORD_SIZE(40)
```

否则首条 `put` 即 `QuotaExceeded`。

**示例**：8MiB 段、512 页大小、1 槽/页、8000 页
- `append_start = 56 + 8000 × 512 × 1 = 4,194,356`
- `segment_size − 40 = 8,388,568`
- `4,194,356 < 8,388,568` ✅

### 4.3 容量计算

```
理论容量 = max_pages × max_segments × slots_per_page
设备大小 = segment_size × max_segments
```

**示例**：100M 条记录
- `max_pages = 8000`
- `max_segments = 12800`
- `slots_per_page = 1`
- 容量 = 8000 × 12800 × 1 = 102,400,000 ≥ 100M ✅
- 设备 = 8MiB × 12800 = 100 GiB

### 4.4 加密配置

```rust
use embedded_database_core::{Config, EncryptionKey};

let key = EncryptionKey::from_bytes(&[0u8; 32]).expect("密钥无效");
let cfg = Config {
    encryption_key: Some(key),
    ..Config::default()
};
```

---

## 5. 错误处理

### 5.1 Error 枚举

| 错误 | 含义 | 重试策略 |
|------|------|----------|
| `Busy` | 写者锁被占用 | 重试（embedded-database 永不等待） |
| `DeadlineExceeded` | 超过 Deadline | 检查 Deadline 设置 |
| `CapacityExhausted` | 固定容量已满 | **不可恢复**，提高配置容量后重试 |
| `QuotaExceeded` | 存储配额超限 | 检查几何约束 |
| `KeyNotFound` | 键不存在 | 正常路径 |
| `Corruption { segment_seq }` | 数据损坏（段级） | 检查设备/恢复 |
| `CorruptedPage { page_id }` | 数据损坏（页级） | 检查设备/恢复 |
| `PageNotFound { page_id }` | 段中不存在 page_id | 正常路径 |
| `InvalidArgument` | 参数非法 | 检查配置/key 含 0xFF |

### 5.2 重试模式

```rust
use std::thread;
use std::time::Duration;

fn put_with_retry(kv: &mut KvDb, k: &[u8], v: &[u8]) -> Result<()> {
    let mut attempts = 0;
    loop {
        match kv.put(k, v) {
            Ok(()) => return Ok(()),
            Err(embedded_database_core::Error::Busy) if attempts < 3 => {
                thread::sleep(Duration::from_millis(1));
                attempts += 1;
            }
            Err(e) => return Err(e),
        }
    }
}
```

---

## 6. 工具使用

### 6.1 inject_crash 崩溃注入

```bash
cargo run --release -p embedded-database-bench --bin inject_crash
```

验证 9 个崩溃点的恢复能力。退出码 0 = 全部通过。

### 6.2 wcet WCET 实测

```bash
cargo run --release -p embedded-database-bench --bin wcet
```

输出各操作的 min/p50/p99/max 时延。退出码 0 = 全部在预算内。

**注意**：macOS 调度噪声可能导致偶发 exit=1，3 次全过即可。

### 6.3 scale_test 规模验证

```bash
# 默认 100M 条（约 15 分钟）
cargo run --release -p embedded-database-bench --bin scale_test

# 自定义记录数
SCALE_N=50000 cargo run --release -p embedded-database-bench --bin scale_test

# 指定数据库路径
SCALE_N=50000 cargo run --release -p embedded-database-bench --bin scale_test /tmp/test.db
```

**输出**：
- 写入吞吐
- 读取采样（1000 条）
- 崩溃恢复
- 写放大估算

退出码 0 = 全部通过。

---

## 7. 故障排查

### 7.1 常见错误

| 错误 | 原因 | 解决方案 |
|------|------|----------|
| `QuotaExceeded` | 几何约束不满足 | 调整 `segment_size`/`max_pages`/`page_size` |
| `CapacityExhausted` | 容量已满 | 提高 `max_segments` 或 `max_pages` |
| `InvalidArgument` | key 含 `0xFF` | 避免 key 中出现 `0xFF` 字节 |
| `Busy` | 并发写入 | 重试或串行化写入 |
| `Corruption` | 设备损坏/位翻转 | 检查设备健康，用 `inject_crash` 验证恢复 |

### 7.2 性能调优

| 场景 | 建议 |
|------|------|
| 高频写入 | 用 `put_batch` 而非 `put` |
| 大容量 | 增加 `max_segments`，保持 `page_size=512` |
| 读取密集 | BTreeMap 已优化 O(log n)，无需额外调优 |
| 加密开销 | SM4-CBC 每页加密，吞吐下降约 20% |

### 7.3 内存占用

| 结构 | 估算（每条记录） |
|------|------------------|
| `key_map` (BTreeMap) | ~68 字节 |
| `page_map` (Vec) | ~24 字节 |
| 总计 | ~92 字节 |

**示例**：100M 条记录 ≈ 9.2 GB 内存。

---

## 8. 信创适配

### 8.1 LoongArch 构建

```bash
# 安装目标
rustup target add loongarch64-unknown-linux-gnu
rustup target add loongarch64-unknown-linux-musl
rustup target add loongarch64-unknown-none

# 交叉编译
cargo build --release --target loongarch64-unknown-linux-gnu
```

详细步骤见 `2026-10-02-embedded-database-loongarch-build-guide.md`。

### 8.2 国密配置

```rust
use embedded_database_core::{Config, EncryptionKey};

// SM4 密钥（32 字节）
let key_bytes = [
    0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08,
    0x09, 0x0A, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F, 0x10,
    0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18,
    0x19, 0x1A, 0x1B, 0x1C, 0x1D, 0x1E, 0x1F, 0x20,
];
let key = EncryptionKey::from_bytes(&key_bytes).expect("密钥无效");

let cfg = Config {
    encryption_key: Some(key),
    ..Config::default()
};
```

**加密算法**：
- 数据加密：SM4-CBC（国密标准）
- 消息认证：HMAC-SM3（GB/T 32905-2016）
- 哈希：SM3

### 8.3 认证申报

详细材料见 `2026-10-02-embedded-database-certification-template.md`。

---

## 附录

### A. Crate 一览

| Crate | 用途 | no_std | unsafe |
|-------|------|--------|--------|
| `embedded-database-core` | 存储内核 | ✅ | 禁止 |
| `embedded-database-kv` | KV 接口 | ✅（std 门控） | 禁止 |
| `embedded-database-embed` | 嵌入 API（公开门面，std 门控） | ✅（std 门控） | 禁止 |
| `embedded-database-os` | 文件后端 | ❌（std） | 禁止 |
| `embedded-database-sql` | SQL 引擎 | ✅（alloc 门控） | 禁止 |
| `embedded-database-cli` | SQL shell | ❌（std） | 禁止 |
| `embedded-database-bench` | 基准工具 | ❌（std） | 禁止 |

### B. 性能基准（M30 实测）

| 操作 | 吞吐 | 说明 |
|------|------|------|
| `put_batch` 写入 | 124933 条/秒 | 批量模式，BTreeMap |
| `put` 写入 | 23690 条/秒 | 单条模式 |
| `get` 读取 | O(log n) | BTreeMap 查找 |
| 100M 写入 | 800 秒 | 全链路验证 |
| 100M 冷启动重建 | ~45 秒 | key_map 重建 |

### C. 已知限制

| 限制 | 影响 |
|------|------|
| key 含 `0xFF` 会错读 | M9-1 已在 put 拒绝新数据 |
| 旧段不物理擦除 | 设备空间未回收 |
| HMAC 非常数时间比较 | 受 C3 无 `subtle` 依赖约束 |
| SQL 无优化器/子查询 | 复杂查询不支持 |
| LoongArch 实机验证未执行 | 无硬件 |

---

## 9. 跨平台支持

| 平台 | 状态 | 说明 |
|------|------|------|
| Linux | ✅ | 主要目标平台 |
| macOS | ✅ | 开发验证（FileDevice） |
| Windows | ✅ | 理论支持（std::fs 跨平台），未验证 |
| LoongArch | ✅ | 信创路线（需实机验证） |
| 裸机 | ✅ | no_std 路径 |

### 构建命令

```bash
# Linux/macOS
cargo build --release

# LoongArch 交叉编译
cargo build --release --target loongarch64-unknown-linux-gnu
cargo build --release --target loongarch64-unknown-linux-musl
cargo build --release --target loongarch64-unknown-none
```

### 数据类型支持

| 类型 | Value 变体 | SQL 字面量 |
|------|-----------|-----------|
| 整数 | `Int(i64)` | `123` |
| 浮点 | `Float(f64)` | `3.14` |
| 文本 | `Text(String)` | `'hello'` |
| 布尔 | `Boolean(bool)` | `TRUE` / `FALSE` |
| 二进制 | `Blob(Vec<u8>)` | （API 层） |
| 空值 | `Null` | `NULL` |

### 标量函数

| 函数 | 说明 |
|------|------|
| `upper(s)` | 转大写 |
| `lower(s)` | 转小写 |
| `length(s)` | 字符串长度 |
| `substr(s, start, len)` | 子串 |
| `abs(n)` | 绝对值 |
| `round(n, d)` | 四舍五入到 d 位小数 |
| `coalesce(a, b, ...)` | 返回第一个非 Null 值 |

### 聚合函数

| 函数 | 说明 |
|------|------|
| `count(*)` | 计数 |
| `sum(col)` | 求和 |
| `avg(col)` | 平均值 |
| `min(col)` | 最小值 |
| `max(col)` | 最大值 |
