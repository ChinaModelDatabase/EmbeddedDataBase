# embedded-database GB/T 22239 安全设计

> 生成日期：2026-10-02
> 依据：《信息安全技术 网络安全等级保护基本要求》GB/T 22239-2019
> 目标等级：第二级（设计文档 §6.3）
> 证据引用验证：2026-10-02，全文档代码位置均经 `rg` 核实存在

## 1. 威胁建模

### 1.1 攻击面

| 攻击面 | 描述 | embedded-database 现状 |
|--------|------|--------------|
| 数据篡改 | 恶意修改磁盘上的页数据 | CRC-32C 逐页校验 + CRC 链（`crates/embedded-database-core/src/crc.rs`、`crates/embedded-database-core/src/segment.rs` `compute_crc`） |
| 数据丢失 | 崩溃导致未提交数据丢失 | Commit Record 原子性 + 崩溃恢复（`crates/embedded-database-core/src/recovery.rs`；`inject_crash` 覆盖 9 个崩溃点 + 基线 + 多段 + compaction 场景（exit=0）；10000 次注入验证为设计文档 §9 M2 验收标准的**目标**，当前工具为抽样验证） |
| 拒绝服务 | 耗尽固定容量结构 | 显式返回 `CapacityExhausted`，不静默降级（`crates/embedded-database-core/src/arena.rs`、`crates/embedded-database-core/src/writer.rs`） |
| 未授权访问 | 进程级访问控制 | 嵌入式数据库，访问控制由宿主应用实现（设计文档 §4、§7） |
| 供应链攻击 | 依赖被篡改 | `cargo vendor` + `cargo-vet` 审计 + SBOM（`vendor/`、`sbom.json`、设计文档 §6.4） |
| 侧信道攻击 | 时序侧信道 | 确定性时延设计（WCET 上界，`docs/superpowers/specs/2026-10-02-embedded-database-wcet-spec.md`） |

### 1.2 不在威胁模型内

- 进程级权限提升（嵌入式数据库假设宿主已授权）
- 物理攻击（Flash 芯片级）
- 网络攻击（无网络接口）

## 2. GB/T 22239 第二级技术要求对照

### 2.1 安全物理环境

| 要求 | embedded-database 应对 | 证据 |
|------|-------------|------|
| 室温、湿度、防尘 | 不适用（软件项目） | — |
| 供电、防雷 | 不适用 | — |

### 2.2 安全通信网络

| 要求 | embedded-database 应对 | 证据 |
|------|-------------|------|
| 网络架构 | 不适用（无网络接口） | 设计文档 §4 非目标：无网络协议与服务端 |
| 通信传输 | 不适用 | — |

### 2.3 安全区域边界

| 要求 | embedded-database 应对 | 证据 |
|------|-------------|------|
| 边界防护 | 不适用（嵌入式数据库） | — |
| 访问控制 | 宿主应用实现进程级访问控制 | 设计文档 §4、§7 |
| 入侵防范 | 不适用 | — |

### 2.4 安全计算环境

| 要求 | embedded-database 应对 | 证据 |
|------|-------------|------|
| 身份鉴别 | 不适用（嵌入式数据库，无用户概念） | — |
| 访问控制 | 单写者锁 + `Busy` 返回 | `crates/embedded-database-core/src/db.rs:150` `try_lock_writer` |
| 安全审计 | 错误返回含上下文（段序号/页号） | `crates/embedded-database-core/src/lib.rs:96` `Error::Corruption { segment_seq }`；`lib.rs:98` `Error::CorruptedPage { page_id }` |
| 入侵防范 | `forbid(unsafe_code)` 阻止内存破坏 | `crates/embedded-database-core/src/lib.rs:2`、`crates/embedded-database-kv/src/lib.rs:2`、`crates/embedded-database-embed/src/lib.rs:2` |
| 恶意代码防范 | 依赖最小化（2 个运行时依赖） | `crates/embedded-database-core/Cargo.toml:15-17`（`zerocopy` + `bitflags`） |
| 剩余信息保护 | `Memtable::clear()` 逻辑清空 | `crates/embedded-database-core/src/memtable.rs:84` |
| 可信验证 | CRC-32C 逐页校验 + CRC 链 | `crates/embedded-database-core/src/crc.rs`、`crates/embedded-database-core/src/segment.rs` |

### 2.5 数据完整性

| 要求 | embedded-database 应对 | 证据 |
|------|-------------|------|
| 数据完整性 | CRC-32C 覆盖所有写入 | `crates/embedded-database-core/src/segment.rs` `compute_crc`（第 137/205/266/326 行，页头/页体/槽表/段头各自覆盖） |
| 恢复能力 | 崩溃恢复；`inject_crash` 覆盖 9 个崩溃点 + 基线 + 多段 + compaction 场景（exit=0）；10000 次注入验证为设计文档 §9 M2 验收标准的**目标**，当前工具为抽样验证 | `crates/embedded-database-core/src/recovery.rs`；`crates/embedded-database-bench/src/bin/inject_crash.rs` |
| 审计记录保护 | `DbStats` 计数审计（M6 实现） | `crates/embedded-database-core/src/types.rs:129-135`；`crates/embedded-database-core/src/db.rs:188`；详见 §6.4 |

### 2.6 数据保密性

| 要求 | embedded-database 应对 | 证据 |
|------|-------------|------|
| 数据保密性 | SM4-CBC 页级 payload 加密 + 每页 HMAC-SM3（M8 接线） | `crates/embedded-database-core/src/crypto.rs` `encrypt_page`/`decrypt_page`；`writer.rs` `append_page` 加密路径；`db.rs` `read_payload` 解密路径；详见 §6.1、§6.2 |
| 消息完整性 | 每页 HMAC-SM3（IV‖密文），解密前先验证 | `crates/embedded-database-core/src/crypto.rs`；`crates/embedded-database-core/src/hmac.rs` `hmac_sm3`；详见 §6.2 |
| 密钥管理 | `EncryptionKey` 由宿主提供，不落盘 | `crates/embedded-database-core/src/crypto.rs`；`crates/embedded-database-core/src/types.rs` `Config.encryption_key`；详见 §6.3 |

### 2.7 剩余信息保护

| 要求 | embedded-database 应对 | 证据 |
|------|-------------|------|
| 剩余信息清除 | `Memtable::clear()` 逻辑清空（M2 简化：不回收 arena 空间） | `crates/embedded-database-core/src/memtable.rs:84`；设计文档 M2 简化说明 |

### 2.8 可信验证

| 要求 | embedded-database 应对 | 证据 |
|------|-------------|------|
| 系统可信验证 | `forbid(unsafe_code)` + 代码审计 | 全仓约束（core/kv/embed `lib.rs:2`）；设计文档 §6.4 |
| 数据可信验证 | CRC-32C + CRC 链 | `crates/embedded-database-core/src/crc.rs`、`crates/embedded-database-core/src/recovery.rs` |

## 3. 应对措施与证据

| 措施 | 实现位置 | 测试覆盖 |
|------|----------|----------|
| 内存安全 | `#![forbid(unsafe_code)]` 全仓（core/kv/embed `lib.rs:2`；其余 crate `Cargo.toml` lints `unsafe_code = "forbid"`） | CI unsafe 扫描 |
| 数据完整性 | CRC-32C 逐页 + CRC 链（`crc.rs`、`segment.rs` `compute_crc`） | `segment.rs` 逐位翻转测试 |
| 崩溃安全 | Commit Record 原子性（`segment.rs` commit 路径） | `inject_crash` 覆盖 9 个崩溃点 + 基线 + 多段 + compaction 场景（exit=0）；10000 次注入验证为设计文档 §9 M2 验收标准的**目标**，当前工具为抽样验证（`crates/embedded-database-bench/src/bin/inject_crash.rs`） |
| 确定性时延 | WCET 上界 | `wcet` 工具（`docs/superpowers/specs/2026-10-02-embedded-database-wcet-spec.md`） |
| 供应链审计 | `cargo vendor` + `cargo-vet`（本地审计 + 上游 import）+ `cargo-deny` 四类门禁 + SBOM | `supply-chain/`、`deny.toml`、`vendor/`、`sbom.json`；入口 `scripts/supply-chain-audit.sh`，CI `supply-chain` job |
| 单写者 | `AtomicBool` CAS + `Busy`（`try_lock_writer`） | `crates/embedded-database-core/src/db.rs` 测试 |

## 4. 待补项清单

| 项 | 优先级 | 里程碑 | 状态 |
|----|--------|--------|------|
| 数据加密（SM4-CBC + HMAC） | 中 | M6/M8 | ✅ 已实现并接入生产路径（§6.1、§6.2） |
| 密钥管理 | 中 | M6 | ✅ 已实现（§6.3） |
| 安全审计日志 | 低 | M6/M8 | ✅ 已实现（§6.4，计数式，read/error 已接线） |
| 剩余信息物理擦除 | 低 | M6 | ⏳ 未实现（逻辑清空，见 §2.7） |
| 可信验证（TPM/TEE 集成） | 低 | M6 | ⏳ 留待后续（§6.5） |

## 5. 与 Ferrocene 路线的对比

| 维度 | GB/T 22239（本项目） | Ferrocene（IEC 61508） |
|------|---------------------|------------------------|
| 认证类型 | 分级保护 | 功能安全 |
| 目标平台 | LoongArch + ARM64 + x86_64 | ARM64 only（无 LoongArch） |
| 工具链认证 | 无（依赖代码审计） | TÜV SÜD 认证 |
| 适用场景 | 信创政企、工业控制 | 汽车、医疗、工业安全 |
| 成本 | 低（文档 + 审计） | 高（€240/席位/年） |

选择 GB/T 22239 的原因见设计文档 D2：本项目主线是信创，LoongArch 是必要目标，Ferrocene 不支持 LoongArch。

## 6. M6/M8 安全措施

> 更新日期：2026-10-03
> 证据引用验证：2026-10-03，本章代码位置均经 `rg` 核实存在
> M8 更新：加密模式 ECB → SM4-CBC + 每页 HMAC-SM3（已接入生产写/读/恢复路径）

### 6.1 数据加密（SM4-CBC）

- 算法：SM4（GB/T 32907-2016），128-bit 密钥，128-bit 分组
- 位置：页级 payload 加密
  - 写入：`SegmentWriter::append_page` → `crypto::encrypt_page`
    （`crates/embedded-database-core/src/writer.rs`）
  - 读取：`Db::read_payload` → `crypto::decrypt_page`
    （`crates/embedded-database-core/src/db.rs`）
  - 恢复：`recovery.rs` 扫描时用 `decrypt_page` 解密（IV 依赖 page_id）
- 模式：**SM4-CBC**（M8 升级，替代 M6 的 ECB）
  - IV = `derive_iv(page_id)`：前 8 字节 page_id（LE），后 8 字节 0
    （`crates/embedded-database-core/src/sm4.rs` `derive_iv`）
  - 确定性 IV：相同明文 + 相同 page_id → 相同密文（简化，见 §6.6）
- 填充：PKCS#7（`sm4_cbc_encrypt`，密文长度 = ceil((len+1)/16)*16）
- 实现：自研纯 Rust（`crates/embedded-database-core/src/sm4.rs`、`crypto.rs`），无新依赖
- 验证：GB/T 32907-2016 标准测试向量（`sm4.rs` `sm4_标准测试向量`）；
  CBC 往返与不同 IV 不同密文测试
- 说明：加密仅覆盖页 payload；页头、槽表、段头仍明文存储（CRC-32C 保护完整性）
- ECB payload 辅助函数已移除；写/读/恢复路径统一走 CBC + HMAC

### 6.2 消息认证（HMAC-SM3，已接入写路径）

- 算法：HMAC-SM3（RFC 2104 + GB/T 32905-2016）
- 用途：**每页消息认证**（M8 接线，解密前先验证）
- 密文格式：`CBC密文 || HMAC-SM3(key.hmac, IV||CBC密文)`
- 位置：
  - 加密追加：`crypto::encrypt_page`（`crates/embedded-database-core/src/crypto.rs`）
  - 解密校验：`crypto::decrypt_page`（MAC 不匹配返回 `InvalidArgument`）
  - 生产路径：`writer.rs` `append_page`、`db.rs` `read_payload`、`recovery.rs`
- 实现：自研纯 Rust（`crates/embedded-database-core/src/hmac.rs`，`hmac_sm3` 函数位于第 19 行）
- 哈希底层：SM3（`crates/embedded-database-core/src/sm3.rs`，GB/T 32905-2016）
- 密钥字段：`EncryptionKey.hmac`（`crates/embedded-database-core/src/crypto.rs`，32 字节）
- 开销：每页额外 32 字节（HMAC 尾部）
- 简化：HMAC 比较为普通 `!=`（非常数时间）；侧信道防护留待后续

### 6.3 密钥管理

- 密钥由调用方通过 `Config.encryption_key` 提供（`crates/embedded-database-core/src/types.rs`）
- `EncryptionKey` 结构（`crates/embedded-database-core/src/crypto.rs`）：
  - `sm4: Sm4Key`（16 字节，页 CBC 加密）
  - `hmac: [u8; 32]`（HMAC-SM3 密钥，M8 已接入每页认证）
- `Db` 持有密钥（经 `SegmentConfig` 传递），不落盘
- 密钥材料由宿主应用管理（embedded-database 不负责密钥存储）
- `encryption_key = None` 时不做加密（向后兼容，默认配置即为 `None`）

### 6.4 安全审计日志

- `Db::stats()` 返回操作计数（`crates/embedded-database-core/src/db.rs:188`）
- `DbStats` 结构（`crates/embedded-database-core/src/types.rs:129-135`）：
  - `commit_count`：提交次数
  - `read_count`：读取次数
  - `write_count`：写入变更数
  - `error_count`：错误次数
  - `last_error: Option<Vec<u8>>`：最近一次错误
- 计数更新点：commit 路径（`db.rs:501-502`）
- no_std 下不引入日志框架，通过计数实现审计

### 6.5 可信验证

- CRC-32C 逐页校验 + CRC 链（已有，见 §2.5、§2.8）
- SM4-CBC + HMAC-SM3 提供数据保密性与消息认证（§6.1、§6.2）
- `forbid(unsafe_code)` 提供内存安全保证（全仓 core/kv/embed `lib.rs:2`）
- TPM/TEE 集成留待后续（当前无硬件依赖）

### 6.6 M8 加密简化

- CBC IV 为确定性（page_id 派生），相同明文+相同 page_id 产生相同密文。
  随机 IV 需额外存储，no_std/Flash 场景留待后续。
- HMAC 比较为普通 `!=`（非常数时间）。
- page_id 全局递增，但 key→page 映射仍未实现（KV 语义需 M9）。

## 7. 证据引用验证记录

以下位置于 2026-10-02 经 `rg` 核实存在（§6 M6 项于 2026-10-03 核实；M8 加密接线于 2026-10-03 核实）：

| 简报引用 | 核实位置 | 结果 |
|----------|----------|------|
| `db.rs` `try_lock_writer` | `crates/embedded-database-core/src/db.rs:150` | ✅ |
| `crc.rs` | `crates/embedded-database-core/src/crc.rs` | ✅ |
| `segment.rs` `compute_crc` | `crates/embedded-database-core/src/segment.rs:137/205/266/326` | ✅ |
| `recovery.rs` | `crates/embedded-database-core/src/recovery.rs` | ✅ |
| `inject_crash` | `crates/embedded-database-bench/src/bin/inject_crash.rs` | ✅ |
| `memtable.rs` `Memtable::clear()` | `crates/embedded-database-core/src/memtable.rs:84` | ✅ |
| `Error::Corruption { segment_seq }` | `crates/embedded-database-core/src/lib.rs:96` | ✅ |
| `Error::CorruptedPage { page_id }` | `crates/embedded-database-core/src/lib.rs:98` | ✅（补强：页号上下文） |
| `#![forbid(unsafe_code)]` | core/kv/embed `lib.rs:2`；其余 crate Cargo.toml lints | ✅ |
| 2 个运行时依赖 | `crates/embedded-database-core/Cargo.toml:15-17`（`zerocopy`、`bitflags`） | ✅ |
| M6 `sm4.rs` | `crates/embedded-database-core/src/sm4.rs`（`sm4_cbc_encrypt` PKCS#7，CBC 模式） | ✅ |
| M6 `sm3.rs` | `crates/embedded-database-core/src/sm3.rs` | ✅ |
| M6 `hmac.rs` | `crates/embedded-database-core/src/hmac.rs:19` `hmac_sm3` | ✅ |
| M6 `crypto.rs` `EncryptionKey` | `crates/embedded-database-core/src/crypto.rs` | ✅ |
| M8 页加密写入路径 | `writer.rs` `append_page` → `crypto::encrypt_page` | ✅ |
| M8 页解密读取路径 | `db.rs` `read_payload` → `crypto::decrypt_page` | ✅ |
| M8 恢复解密路径 | `recovery.rs` → `crypto::decrypt_page`（IV 依赖 page_id） | ✅ |
| M6 `Config.encryption_key` | `crates/embedded-database-core/src/types.rs` | ✅ |
| M6 `DbStats` | `crates/embedded-database-core/src/types.rs`；`db.rs` `stats()` | ✅ |
| M8 加密模式 SM4-CBC | `sm4.rs` `sm4_cbc_encrypt` + `derive_iv`；`crypto.rs` `encrypt_page` | ✅ |
| M8 每页 HMAC-SM3 | `crypto.rs` `encrypt_page`/`decrypt_page`（MAC over IV‖密文） | ✅ |
