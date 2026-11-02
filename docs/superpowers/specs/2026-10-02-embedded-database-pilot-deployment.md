# embedded-database 试点部署方案

> 生成日期：2026-10-02
> 依据：设计文档 §9 M6（试点现场稳定运行 ≥ 3 个月）

## 1. 试点目标

- 验证 embedded-database 在真实工业场景下的稳定性
- 收集 WCET 实测数据（LoongArch 实机）
- 发现并修复生产环境问题
- 为认证申报提供现场测试报告

## 2. 试点环境

| 项 | 配置 |
|----|------|
| CPU | 龙芯 3B6000 / 3C6000 |
| OS | 银河麒麟 V10 SP1 / 统信 UOS |
| 内核 | ≥ 5.19 |
| 存储 | eMMC / SSD（Flash 友好） |
| 数据库容量 | 按实际场景配置（建议 ≤ 64 MiB） |

## 3. 部署步骤

1. 交叉编译：`cargo build --release --target loongarch64-unknown-linux-musl`
2. 静态链接二进制拷贝到目标机
3. 运行 `inject_crash` 验证崩溃恢复
4. 运行 `wcet` 记录实测时延
5. 集成到宿主应用

## 4. 监控指标

| 指标 | 预算 | 采集方式 |
|------|------|----------|
| append_page p99 | 150μs | wcet 工具 |
| btree_insert p99 | 200μs | wcet 工具 |
| commit p99 | 30μs | wcet 工具 |
| 崩溃恢复成功率 | 100% | inject_crash |
| 磁盘写放大 | < 2.0 | CompactReport |

## 5. 验收标准

- [ ] 连续运行 ≥ 3 个月无数据丢失
- [ ] 崩溃恢复 100% 成功
- [ ] WCET 实测值在预算内
- [ ] 无内存泄漏（长期运行稳定）

## 6. 风险与应对

| 风险 | 应对 |
|------|------|
| LoongArch 内核版本不足 | 使用 musl 静态链接 |
| Flash 磨损超预期 | 监控写放大，调整 compaction 频率 |
| 容量耗尽 | 监控 `CapacityExhausted` 错误，提前扩容 |
