# embedded-database WCET 规格书 v1

> 生成日期：2026-10-02
> 构建环境：macOS 27.0 (Apple Silicon), Rust 1.81.0, debug/release
> ⚠️ macOS 共享开发机调度噪声可使 p99 波动一个数量级；此数值为噪声容忍阈值而非确定性上界。
> 真正的 WCET 需要 release 构建 + 空闲机器或裸机/RTOS 环境。

## M1 核心操作

| 操作 | 构建 | n | min | p50 | p99 | max | 预算 |
|------|------|---|-----|-----|-----|-----|------|
| append_page | debug | 32 | 48917ns | 49500ns | 148417ns | 148417ns | 150μs |
| btree_insert | debug | 128 | 1583ns | 4167ns | 15333ns | 16542ns | 200μs |
| commit | debug | 200 | 791ns | 875ns | 9291ns | 9708ns | 30μs |
| append_page | release | 32 | 14208ns | 14500ns | 18042ns | 18042ns | 150μs |
| btree_insert | release | 128 | 83ns | 167ns | 667ns | 16167ns | 200μs |
| commit | release | 200 | 166ns | 208ns | 6208ns | 15375ns | 30μs |

## M2 事务操作

| 操作 | 构建 | n | min | p50 | p99 | max | 预算 |
|------|------|---|-----|-----|-----|-----|------|
| db_put | debug | 1000 | 125ns | 166ns | 208ns | 2250ns | 10μs |
| db_get | debug | 1000 | 0ns | 42ns | 83ns | 209ns | 10μs |
| db_commit | debug | 1000 | 83ns | 125ns | 125ns | 375ns | 10μs |
| db_put | release | 1000 | 41ns | 83ns | 125ns | 2709ns | 10μs |
| db_get | release | 1000 | 0ns | 41ns | 42ns | 42ns | 10μs |
| db_commit | release | 1000 | 41ns | 42ns | 84ns | 208ns | 10μs |

## 确定性保证

- 哈希表：开放寻址 + 固定容量，查找 O(1) 恒定，绝不退化为链式
- B-tree：定高两层，深度恒为 2，无重平衡
- 内存分配：Bump Arena，启动时一次性预分配，热路径零 malloc
- 写者竞争：try_lock，失败返回 Busy，绝不自旋
- 合并：预算化切片，可被 Deadline 中断

## 已知限制

- macOS 调试构建噪声可把 p99 推高一个数量级
- `append_page` M1 每次整页写+CRC 是真实路径成本
- M2 事务 commit 不写段，db_commit 基线只覆盖内存简化路径
