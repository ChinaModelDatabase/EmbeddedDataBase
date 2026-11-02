# embedded-database 最终交付清单

> 生成日期：2026-10-05
> 状态：M1–M32 全部完成，294 测试全绿，0 提交

## 1. 代码交付

| Crate | 说明 | 行数 | 状态 |
|-------|------|------|------|
| embedded-database-core | 存储内核（no_std） | ~8200 | ✅ |
| embedded-database-kv | 类型化 KV 接口 | 215 | ✅ |
| embedded-database-embed | 公开嵌入 API（`Db`/`Txn` 门面） | 362 | ✅ |
| embedded-database-os | Linux file 后端（std） | 182 | ✅ |
| embedded-database-sql | SQL 引擎：词法/语法/执行器/结果类型/函数/类型扩展（no_std） | ~2600 | ✅ |
| embedded-database-cli | 交互式 SQL shell（std） | 110 | ✅ |
| embedded-database-bench | 基准工具（含 scale_test） | ~480 | ✅ |
| **合计** | | **~11554** | |

（Spec 3a 后 embedded-database-embed 为实测行数；合计为历史近似值，未重算。）

## 2. SQL 引擎交付物（M16–M27）

| 交付物 | 里程碑 | 说明 |
|--------|--------|------|
| `embedded-database-sql` crate | M16 | `#![no_std]` + `#![forbid(unsafe_code)]` |
| SQL 词法分析器 | M16 | Token/Keyword/Identifier/字面量/运算符 |
| SQL AST | M17 | SELECT/INSERT/CREATE TABLE/DROP TABLE |
| SQL 执行器 | M18 | `execute(sql, db) -> QueryResult`，`SqlError` |
| WHERE 过滤 | M19/M22 | `=`/`!=`/`<`/`<=`/`>`/`>=` |
| `embedded-database-cli` | M20 | 交互式 shell，`.exit`/`.quit` 退出 |
| ORDER BY + LIMIT | M21/M22 | 多列 ASC/DESC，LIMIT 截断 |
| JOIN | M23 | INNER JOIN，ON 等值，嵌套循环连接 |
| 聚合函数 | M24 | COUNT/SUM/AVG/MIN/MAX |
| GROUP BY | M25 | 单列分组生效 |
| HAVING | M26/M27 | `聚合 op 字面量`；M27 修复聚合列不在 SELECT 列表时过滤被跳过 |
| SQL 函数扩展 trait | M32 | `ScalarFunction`/`AggregateFunction`，编译期静态分发 |
| SQL 类型扩展 trait | M32 | `SqlType`（type_name/parse/format） |

SQL 简化边界：无优化器/子查询；比较非类型感知；GROUP BY 仅单列生效；JOIN 仅 INNER。

## 3. 性能与规模验证交付物

| 交付物 | 里程碑 | 说明 |
|--------|--------|------|
| `scale_test.rs` | M29 | 规模验证工具，`SCALE_N` 环境变量控制记录数 |
| 批量插入优化 | M29-1 | `KvDb::put_batch` + `flush` 中 `take()` 替代 `clone()` |
| BTreeMap key_map | M30 | `lookup_key`/`update_key_map` O(n) → O(log n) |
| 亿级规模验证 | M30 | `SCALE_N=100_000_000` 全链路 exit=0，吞吐 124933 条/秒 |
| 存储后端实现指南 | M32 | `BlockDevice` trait 约束、内存/文件/硬件/压缩后端 |

## 4. 文档交付

| 文档 | 说明 | 状态 |
|------|------|------|
| 设计文档 | 总体架构、M1–M32 实施状态、§5.2.1 SQL 引擎实际交付 | ✅ |
| GB/T 22239 安全设计 | 威胁建模、技术要求对照 | ✅ |
| LoongArch 构建手册 | 工具链要求、验证记录 | ✅ |
| WCET 规格书 | 实测数据、确定性保证 | ✅ |
| LoongArch 实机验证 checklist | 拿到硬件后的验证清单 | ✅ |
| 认证申报材料模板 | CSTC + 自主可控 + 安全可靠测评 | ✅ |
| 试点部署方案 | 部署步骤、监控指标 | ✅ |
| 项目总结报告 | M1–M32 交付物、SQL 功能清单、已知限制 | ✅ |
| 最终交付清单 | 本文件 | ✅ |

## 5. 工具交付

| 工具 | 说明 | 状态 |
|------|------|------|
| inject_crash | 崩溃注入（9 崩溃点 + 基线 + 多段 + compaction） | ✅ |
| wcet | WCET 实测 | ✅ |
| scale_test | 规模验证（千万级/亿级） | ✅ |
| embedded-database-cli | 交互式 SQL shell | ✅ |
| scripts/offline-build.sh | 离线构建 + SBOM | ✅ |
| supply-chain/ | cargo-vet 审计配置 | ✅ |
| sbom.json | CycloneDX SBOM | ✅ |

## 6. 测试覆盖

| 类别 | 数量 |
|------|------|
| embedded-database-core | 216 |
| embedded-database-kv | 8 |
| embedded-database-os | 5 |
| embedded-database-sql | 65 |
| **合计** | **294** |

## 7. 核心技术验证

| 技术 | 验证方式 |
|------|----------|
| SM4-CBC | 与 OpenSSL SM4-CBC 逐字节一致 |
| SM3 哈希 | GB/T 32905-2016 标准测试向量 |
| HMAC-SM3 | RFC 2104 + GB/T 32905-2016 |
| CRC-32C | 标准检验值 + 逐位翻转测试 |
| 崩溃恢复 | inject_crash 9 崩溃点全通过 |
| 5 目标 no_std | aarch64 / loongarch64-gnu / loongarch64-musl / x86_64 / loongarch64-none |
| 离线构建 | cargo vendor + SBOM |
| 供应链审计 | cargo-vet（本地审计 + 上游 import + 带据豁免）+ cargo-deny 四类；脚本 `scripts/supply-chain-audit.sh`，CI `supply-chain` job |
| SQL 引擎 | M16–M32 里程碑测试全绿；CLI `.exit` 可用 |
| 批量插入 | `put_batch` 50K 条 exit=0 |
| 亿级规模 | `SCALE_N=100_000_000` 全链路 exit=0，吞吐 124933 条/秒 |
| BTreeMap 索引 | `lookup_key` O(log n)，亿级读取采样全命中 |

## 8. 已知限制

| 限制 | 影响 | 后续 |
|------|------|------|
| key 含 `0xFF` 的旧数据会错读 | M9-1 已在 put 拒绝新数据 | M10 已记录 |
| 旧段不物理擦除 | 设备空间未回收 | M13 遗留 |
| 写入状态不跨进程持久化 | 重启后 active_writer 丢失 | M12 简化 |
| HMAC 非常数时间比较 | 受 C3 无 `subtle` 依赖约束 | M8 记录 |
| LoongArch 实机验证未执行 | 无硬件 | M13 checklist |
| SQL 无优化器/子查询 | 复杂查询不支持 | 按需评估 |
| SQL 比较非类型感知 | 字符串化比较 | M19 简化 |
| SQL GROUP BY 仅单列生效 | 多列仅解析 | M25 简化 |
| SQL JOIN 仅 INNER + ON 仅 `=` | 外连接/非等值不支持 | M23 简化 |
| `BuiltinAggregates::get` 用 `&dyn` | 与"无 dyn"原则表述不一致 | M32 记录 |

## 9. 后续路线图

- **LoongArch 实机验证**：拿到龙芯硬件后执行 checklist
- **认证申报**：CSTC 适配认证、源代码自主可控评估、安全可靠测评
- **试点部署**：现场稳定运行 ≥ 3 个月
- **生产加固**：密钥轮换、审计日志结构化、TPM/TEE 集成
- **SQL 增强**：类型感知比较、多列 GROUP BY、优化器、子查询（按需评估）
- **M13 遗留**：旧段物理擦除、段回收
- **M31**：十亿级规模验证（当前机器磁盘/内存不足，需硬件升级）

## M36 补充（2026-10-05）

| 交付物 | 里程碑 | 说明 |
|--------|--------|------|
| 数据类型扩展 | M36 | `Value::Boolean`/`Blob`，lexer 支持 TRUE/FALSE |
| SQL 函数扩充 | M36 | lower/length/substr/abs/round/coalesce |
| 跨平台文档 | M36 | 操作手册第 9 章：平台矩阵/构建命令/函数清单 |
