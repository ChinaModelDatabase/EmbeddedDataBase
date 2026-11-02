# embedded-database 项目总结报告

> 生成日期：2026-10-05
> 状态：M1–M32 全部完成，294 测试全绿（core 216 + kv 8 + os 5 + sql 65），0 提交
> 数据来源：`.superpowers/sdd/progress.md`、设计文档「实施状态」小节；测试数与代码行数经实际核实。

## 1. 项目概述

embedded-database 是一个纯 Rust、`no_std`、确定性时延、Flash 友好的嵌入式数据库内核，
面向信创与 PLC/嵌入式硬实时场景。使用接口以 Rust 原生类型化 API 为主，
SQL 为辅（M16–M32 交付门面级 SQL 引擎、CLI 与可扩展架构）。

## 2. 里程碑交付

| 里程碑 | 内容 | 测试数 | 状态 |
|--------|------|--------|------|
| M1 | 段格式、CRC-32C、Arena、哈希表、B+tree、页槽分配、写入器、恢复、崩溃注入、WCET | 111 | ✅ |
| M2 | 事务管理、Commit Record、崩溃恢复、ArenaPool、PageMap、`Db::open`/`begin` | 137 | ✅ |
| M3 | Deadline 传播、范围扫描、CompactionBudget、WCET 规格书 | 153 | ✅ |
| M4 | `embedded-database-os` file 后端、真正段合并、写放大统计 | 162 | ✅ |
| M5 | 离线构建、SBOM、LoongArch 手册、GB/T 22239 安全设计 | 167 | ✅ |
| M6 | SM4/SM3/HMAC-SM3、页级加密、密钥管理、审计日志 | 181 | ✅ |
| M7 | `pending_writes` 真正落盘（`Db::flush`） | 189 | ✅ |
| M8 | SM4-CBC、每页 HMAC、审计接线、遗留修复 | 202 | ✅ |
| M9 | key→page 映射、`Txn::get` 读已提交数据、`KvDb` 接口 | 210 | ✅ |
| M10 | key_map 冷启动重建 | 214 | ✅ |
| M11 | 真正段合并（compact_with_budget 完整） | 216 | ✅ |
| M12 | 同段追加 | 223 | ✅ |
| M13 | LoongArch checklist、认证申报模板、试点部署方案、项目总结报告 | 223 | ✅ |
| M14 | cargo-vet 审计、离线构建可重复验证 | 223 | ✅ |
| M15 | 文档一致性修复、最终交付清单 | 223 | ✅ |
| M16 | `embedded-database-sql` 骨架、SQL 词法分析器 | 233 | ✅ |
| M17 | SQL AST 类型定义 | 242 | ✅ |
| M18 | SQL 执行器 + `QueryResult` | 248 | ✅ |
| M19 | WHERE 条件过滤 | 253 | ✅ |
| M20 | `embedded-database-cli` 交互式 SQL shell | 253 | ✅ |
| M21 | ORDER BY + LIMIT 解析与执行 | 260 | ✅ |
| M22 | WHERE 运算符补齐 + 多列 ORDER BY | 267 | ✅ |
| M23 | SQL JOIN（嵌套循环 INNER JOIN） | 272 | ✅ |
| M24 | 聚合函数 COUNT/SUM/AVG/MIN/MAX | 279 | ✅ |
| M25 | GROUP BY 分组聚合 | 283 | ✅ |
| M26 | HAVING 分组过滤 | 287 | ✅ |
| M27 | HAVING 遗留修复 + 交付文档更新 | 288 | ✅ |
| M28 | 全分支终审 + 收尾验收 | 288 | ✅ |
| M29 | 千万级规模验证工具（`scale_test.rs`） | 288 | ✅ |
| M29-1 | 批量插入优化（`put_batch` + `take()`） | 291 | ✅ |
| M30 | BTreeMap key_map 重构 + 亿级规模验证 | 291 | ✅ |
| M32 | 可扩展架构（SQL 函数/类型 trait + 存储后端文档） | 294 | ✅ |

测试数为各里程碑终审关闭时的 workspace 全量 passed 数（与进度账本逐条核对一致；
M5 起 workspace 含 embedded-database-kv，M16 起含 embedded-database-sql，M20 起含 embedded-database-cli）。

## 3. 核心交付物

### 3.1 代码

- 7 个 crate：`embedded-database-core`（~8200 行）、`embedded-database-kv`（215 行）、
  `embedded-database-embed`（362 行，公开门面，Spec 3a）、`embedded-database-os`（182 行）、
  `embedded-database-sql`（~2600 行）、`embedded-database-cli`（110 行）、`embedded-database-bench`（~480 行）
- 合计约 11554 行 Rust 源码（合计为历史近似值，未含 Spec 3a 的 embed 门面增量）
- 294 测试全绿（core 216 + kv 8 + os 5 + sql 65）
- 全仓 `forbid(unsafe_code)`，`no_std` 路径零 `alloc`
- 5 目标 no_std 编译通过（aarch64-unknown-linux-gnu / loongarch64-unknown-linux-gnu /
  loongarch64-unknown-linux-musl / x86_64-unknown-linux-gnu / loongarch64-unknown-none）

### 3.2 SQL 引擎功能清单（M16–M32）

| 能力 | 里程碑 | 说明 |
|------|--------|------|
| 词法分析 | M16 | Token/Keyword/Identifier/字面量/运算符，`table.column` 单标识符 |
| 语法分析 | M17 | SELECT / INSERT / CREATE TABLE / DROP TABLE AST |
| 执行器 + 结果类型 | M18 | `execute(sql, db) -> QueryResult`，`SqlError { Lex, Parse, Db }` |
| WHERE 过滤 | M19 / M22 | `=` / `!=` / `<` / `<=` / `>` / `>=` |
| CLI | M20 | `embedded-database-cli` 交互式 shell（`.exit` / `.quit` 退出） |
| ORDER BY + LIMIT | M21 / M22 | 多列 ASC/DESC 排序，LIMIT 截断 |
| JOIN | M23 | INNER JOIN，ON 等值，嵌套循环连接 |
| 聚合函数 | M24 | COUNT / SUM / AVG / MIN / MAX |
| GROUP BY | M25 | 单列分组生效 |
| HAVING | M26 / M27 | `聚合 op 字面量`；M27 修复聚合列不在 SELECT 列表时过滤被跳过 |
| 函数扩展 trait | M32 | `ScalarFunction` / `AggregateFunction`，编译期静态分发 |
| 类型扩展 trait | M32 | `SqlType`（type_name / parse / format） |

SQL 简化：无优化器/子查询；比较非类型感知；GROUP BY 仅单列生效；JOIN 仅 INNER。

### 3.3 性能与规模验证

| 交付物 | 里程碑 | 说明 |
|--------|--------|------|
| `scale_test.rs` | M29 | 规模验证工具，`SCALE_N` 环境变量控制记录数 |
| 批量插入优化 | M29-1 | `KvDb::put_batch`，吞吐 23690 → 124933 条/秒（5.3×） |
| BTreeMap key_map | M30 | `lookup_key`/`update_key_map` O(n) → O(log n) |
| 亿级规模验证 | M30 | `SCALE_N=100_000_000` 全链路 exit=0，写入 800s，读取采样全命中 |

### 3.4 文档

- 设计文档（总体架构、M1–M32 实施状态、§5.2.1 SQL 引擎实际交付）
- GB/T 22239 安全设计文档
- LoongArch 构建手册
- WCET 规格书
- LoongArch 实机验证 checklist
- 认证申报材料模板
- 试点部署方案
- 项目总结报告（本文件）
- 最终交付清单
- 存储后端实现指南（M32）

### 3.5 工具

- `inject_crash`：崩溃注入（9 崩溃点 + 基线 + 多段 + compaction）
- `wcet`：WCET 实测
- `scale_test`：规模验证（千万级/亿级）
- `embedded-database-cli`：交互式 SQL shell
- `scripts/offline-build.sh`：离线构建 + SBOM（`vendor/` 38 crates + `sbom.json`）

### 3.6 供应链审计

| 项 | 状态 |
|----|------|
| 运行时依赖 ≤ 2 个 | ✅ zerocopy + bitflags |
| `cargo vendor` 离线构建 | ✅ 已验证 |
| SBOM 生成 | ✅ CycloneDX |
| `cargo-vet` 审计 | ✅ deploy 图 7 个依赖来源明确：本地审计 `zerocopy`/`zerocopy-derive`/`bitflags`/`proc-macro2`/`syn`/`unicode-ident`；上游 import 覆盖 `quote` |
| `cargo-deny` 门禁 | ✅ advisories / licenses / bans / sources 四类（`deny.toml`，CI `supply-chain` job） |
| 全仓 `forbid(unsafe_code)` | ✅ |

## 4. 核心技术

| 技术 | 说明 |
|------|------|
| Append-Only Segment | 100% 顺序写，Flash 友好 |
| CRC-32C + CRC 链 | 数据完整性，跨段校验 |
| Commit Record | 原子提交，单次定长写 |
| SM4-CBC + HMAC-SM3 | 页级加密 + 消息认证（国密标准） |
| 定高两层 B+tree | 深度恒为 2，WCET 可上界 |
| 开放寻址哈希表 | O(1) 查找，固定容量 |
| BTreeMap key_map | O(log n) 查找，亿级规模可用 |
| 同段追加 | 减少段数，提高空间利用率 |
| 崩溃恢复 | 冷启动扫描 + key_map 重建 |
| 批量插入 | `put_batch` 批量提交，吞吐 5.3× |
| SQL 引擎 | 词法+语法+WHERE+ORDER BY+LIMIT+JOIN+聚合+GROUP BY+HAVING+CLI |
| 可扩展架构 | SQL 函数/类型 trait + 存储后端文档 |

## 5. 已知限制

| 限制 | 影响 | 后续 |
|------|------|------|
| key 含 `0xFF` 的旧数据会错读 | M9-1 已在 put 拒绝新数据 | M10 已记录 |
| 旧段不物理擦除 | 设备空间未回收 | M13 遗留 |
| 写入状态不跨进程持久化 | 重启后 active_writer 丢失 | M12 简化 |
| HMAC 非常数时间比较 | 受 C3 无 `subtle` 依赖约束 | M8 记录 |
| LoongArch 实机验证未执行 | 无硬件 | M13 checklist |
| SQL 无优化器/子查询 | 复杂查询不支持 | M26+ 未交付 |
| SQL 比较非类型感知 | 字符串化比较，语义近似 | M19 简化 |
| SQL GROUP BY 仅单列生效 | 多列仅解析不生效 | M25 简化 |
| SQL JOIN 仅 INNER + ON 仅 `=` | 外连接/非等值不支持 | M23 简化 |
| SQL 行值序列化含 `\|`/`=` | payload 含该字符破坏解析 | M18 简化 |
| `BuiltinAggregates::get` 用 `&dyn` | 与"无 dyn"原则表述不一致 | M32 记录 |
| 十亿级规模验证未执行 | 磁盘 1TB / 内存 68GB 需求超出当前机器 | M31 记录 |

## 6. 后续路线图

- **LoongArch 实机验证**：拿到龙芯硬件后执行 checklist
- **认证申报**：CSTC 适配认证、源代码自主可控评估、安全可靠测评
- **试点部署**：现场稳定运行 ≥ 3 个月
- **生产加固**：密钥轮换、审计日志结构化、TPM/TEE 集成
- **SQL 增强**：类型感知比较、多列 GROUP BY、优化器、子查询（按需评估）
- **M13 遗留**：旧段物理擦除、段回收
- **M31**：十亿级规模验证（需硬件升级）

## M36 补充（2026-10-05）

- **M36-0**：数据类型扩展——`Value::Boolean`/`Blob`，lexer 支持 `TRUE`/`FALSE`。
- **M36-1**：SQL 函数扩充——`lower`/`length`/`substr`/`abs`/`round`/`coalesce`。
- **M36-2**：操作手册追加「跨平台支持」章节。
- 测试数：301（core 216 + kv 8 + os 5 + sql 72）。
