# embedded-database LoongArch 实机验证 Checklist

> 生成日期：2026-10-02
> 适用：龙芯 3B6000/3C6000（Ⅱ级）、银河麒麟 V10 SP1、统信 UOS、openEuler
> 前置：已阅读 `2026-10-02-embedded-database-loongarch-build-guide.md`

## 1. 环境准备

- [ ] 确认 CPU 型号（`lscpu`）与内核版本（`uname -r`）
- [ ] 确认内核 ≥ 5.19（或使用 musl 静态链接）
- [ ] 确认 Rust 工具链已安装（`rustc --version` ≥ 1.81）
- [ ] 确认 3 个 loongarch target 的 std 已安装

## 2. 构建验证

- [ ] `cargo check -p embedded-database-core --target loongarch64-unknown-linux-gnu`
- [ ] `cargo check -p embedded-database-core --target loongarch64-unknown-linux-musl`
- [ ] `cargo check -p embedded-database-core --target loongarch64-unknown-none`
- [ ] `cargo build --workspace --features embedded-database-core/std`（本机构建）

## 3. 测试验证

- [ ] `cargo test --workspace --features embedded-database-core/std`（预期 223 passed）
- [ ] `cargo run -p embedded-database-bench --bin inject_crash`（预期 exit=0）
- [ ] `cargo run -p embedded-database-bench --bin wcet`（记录实测值）

## 4. WCET 对比

- [ ] 记录 LoongArch 实机 WCET（append_page / btree_insert / commit / db_put / db_get / db_commit）
- [ ] 与 macOS 基线对比（`2026-10-02-embedded-database-wcet-spec.md`）
- [ ] 若差异 > 2 倍，分析原因（缓存、分支预测、内存带宽）

## 5. 加密验证

- [ ] SM4/SM3/HMAC-SM3 标准测试向量在 LoongArch 上通过
- [ ] CBC+HMAC 加密写入→解密读回端到端通过

## 6. 记录与报告

- [ ] 填写验证结果表格（目标、状态、备注）
- [ ] 记录发现的问题与解决方案
- [ ] 更新 WCET 规格书（`2026-10-02-embedded-database-wcet-spec.md`）
- [ ] 更新设计文档「实施状态」小节
