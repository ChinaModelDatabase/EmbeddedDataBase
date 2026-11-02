# embedded-database LoongArch 构建手册

> 生成日期：2026-10-02
> 适用：龙芯 3B6000/3C6000（Ⅱ级）、银河麒麟 V10 SP1、统信 UOS、openEuler

## 1. 工具链要求

| 组件 | 最低版本 | 备注 |
|------|----------|------|
| Linux kernel | 5.19 | 龙芯 target 要求 |
| glibc | 2.36 | `loongarch64-unknown-linux-gnu` |
| linux-headers | 5.19 | 内核头文件 |
| GCC | 15.x | 交叉编译 |
| Binutils | 2.45 | 交叉编译 |
| Rust | 1.81+ | `loongarch64-unknown-linux-musl` 自 1.81 起提供预构建产物 |

## 2. 已知问题：麒麟 V10 SP1 内核版本

麒麟桌面 V10 SP1 内核为 **5.10**，低于龙芯 target 要求的 **5.19**。rustup 安装器会中止并报错。

**解决方案**（按优先级）：

### 方案 A：musl 静态链接（推荐）

```bash
rustup target add loongarch64-unknown-linux-musl
cargo build --target loongarch64-unknown-linux-musl
```

musl 静态链接不依赖 glibc 版本，规避内核版本约束。

### 方案 B：自建工具链

使用 <https://github.com/loongson/build-tools> 构建 GCC 12.1+ / Binutils 2.40+ 工具链。

### 方案 C：`-Z build-std`（实验性）

```bash
rustup toolchain install nightly
cargo +nightly build -Z build-std=std,panic_abort --target loongarch64-unknown-linux-gnu
```

⚠️ 需要 Rust nightly，且构建时间显著增加。

## 3. 本地验证记录

以下验证在 macOS Apple Silicon 上完成（2026-10-03，`rustc 1.81.0 (eeb90cda1 2024-09-04)`）：

| 目标 | std 安装 | `cargo check` | 结果输出 |
|------|----------|---------------|----------|
| `loongarch64-unknown-linux-gnu` | ✅ 已安装 | ✅ 通过 | `Finished \`dev\` profile [unoptimized + debuginfo] target(s) in 4.79s` |
| `loongarch64-unknown-linux-musl` | ✅ 已安装 | ✅ 通过 | `Finished \`dev\` profile [unoptimized + debuginfo] target(s) in 1.53s` |
| `loongarch64-unknown-none` | ✅ 已安装 | ✅ 通过 | `Finished \`dev\` profile [unoptimized + debuginfo] target(s) in 1.54s` |

验证命令：

```bash
rustup target list --installed | rg loongarch
# loongarch64-unknown-linux-gnu
# loongarch64-unknown-linux-musl
# loongarch64-unknown-none

cargo check -p embedded-database-core --target loongarch64-unknown-linux-gnu
cargo check -p embedded-database-core --target loongarch64-unknown-linux-musl
cargo check -p embedded-database-core --target loongarch64-unknown-none
```

三条 `cargo check` 命令 exit code 均为 0。

### 3.1 加密模块验证（M6，2026-10-03）

在上述基础上，追加验证 M6 加密模块（SM4 / SM3 / HMAC-SM3，均在 `embedded-database-core` 中，纯 Rust 无新依赖）：

| 模块 | 状态 | 备注 |
|------|------|------|
| SM4 加密 | ✅ cargo check 通过 | 自研纯 Rust，无新依赖 |
| SM3 哈希 | ✅ cargo check 通过 | 自研纯 Rust，无新依赖 |
| HMAC-SM3 | ✅ cargo check 通过 | 自研纯 Rust，无新依赖 |

验证命令与结果：

```bash
cargo check -p embedded-database-core --target loongarch64-unknown-linux-gnu  # Finished in 0.04s
cargo check -p embedded-database-core --target loongarch64-unknown-linux-musl # Finished in 0.01s
cargo check -p embedded-database-core --target loongarch64-unknown-none       # Finished in 0.01s
```

三条命令 exit code 均为 0，`embedded-database-core`（含加密模块）在全部三个 LoongArch target 上编译通过。

## 4. 实机验证（待执行）

拿到龙芯硬件后：

1. 安装目标 OS（麒麟 V10 SP1 / 统信 UOS / openEuler）
2. 验证内核版本 ≥ 5.19（或使用 musl 静态链接）
3. 执行 `cargo test --workspace --features embedded-database-core/std`
4. 执行 `cargo run -p embedded-database-bench --bin inject_crash`
5. 执行 `cargo run -p embedded-database-bench --bin wcet`
6. 记录 WCET 实测值与 macOS 基线对比

## 5. 风险与缓解

| 风险 | 影响 | 缓解 |
|------|------|------|
| 麒麟 V10 SP1 内核 5.10 | rustup 安装失败 | musl 静态链接 |
| 国产发行版 fork 内核版本号异常 | 工具链安装失败 | 用 loongson/build-tools 自建 |
| 实机 WCET 与 macOS 差异大 | 性能承诺失效 | 以实机数据为准更新 WCET 规格书 |
