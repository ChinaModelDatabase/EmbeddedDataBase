#!/usr/bin/env bash
# scripts/offline-build.sh
# 离线构建：vendor 依赖 + 本地 registry + SBOM 生成
set -euo pipefail

export PATH="/opt/homebrew/opt/rustup/bin:$PATH"
cd "$(dirname "$0")/.."

# 若已有 vendored source 配置，先移开，保证 vendor/install 能访问 crates.io
if [ -f .cargo/config.toml ] && grep -q 'vendored-sources' .cargo/config.toml 2>/dev/null; then
  mv .cargo/config.toml .cargo/config.toml.moved-by-offline-build
fi

echo "=== 1. Vendor 依赖 ==="
cargo vendor vendor/ 2>&1 | tail -5

echo "=== 2. 安装 SBOM 工具（需网络，失败则跳过 SBOM） ==="
SBOM_TOOL_OK=0
if cargo install --list 2>/dev/null | grep -q '^cargo-cyclonedx v'; then
  echo "cargo-cyclonedx 已安装"
  SBOM_TOOL_OK=1
else
  echo "cargo-cyclonedx 未安装，尝试安装（需要网络，可能较久）..."
  # 在写入 vendored source 配置之前安装，避免 source replacement 阻断 crates.io
  # 0.5.9 需要 rustc 1.85，0.5.8 依赖链含 edition2024；1.81 均无法解析
  # 回退用 stable toolchain 安装该主机工具（与 workspace 构建 toolchain 无关）
  if cargo install cargo-cyclonedx 2>&1 | tail -5; then
    SBOM_TOOL_OK=1
  elif cargo install cargo-cyclonedx --version 0.5.8 2>&1 | tail -5; then
    SBOM_TOOL_OK=1
  elif cargo +stable install cargo-cyclonedx 2>&1 | tail -5; then
    SBOM_TOOL_OK=1
  else
    echo "WARN: cargo-cyclonedx 安装失败，将跳过 SBOM 生成"
    SBOM_TOOL_OK=0
  fi
fi

echo "=== 3. 配置本地 registry ==="
mkdir -p .cargo
cat > .cargo/config.toml <<'EOF'
[source.crates-io]
replace-with = "vendored-sources"

[source.vendored-sources]
directory = "vendor"
EOF

echo "=== 4. 离线构建测试 ==="
cargo build --offline --workspace 2>&1 | tail -5
cargo test --offline --workspace --features embedded-database-core/std 2>&1 | rg 'test result'

echo "=== 5. 生成 SBOM ==="
if [ "$SBOM_TOOL_OK" = "1" ]; then
  # cargo-cyclonedx 0.5.x 对 virtual workspace 逐 member 生成 crates/*/<name>.cdx.json
  # （无 --outfile）；聚合为根目录 sbom.json 以满足验收
  if cargo cyclonedx --format json 2>&1 | tail -3; then
    python3 - <<'PY'
import glob
import json

components = []
seen = set()
for path in sorted(glob.glob("crates/*/*.cdx.json")):
    with open(path) as f:
        data = json.load(f)
    for comp in data.get("components", []):
        key = (comp.get("name"), comp.get("version"))
        if key in seen:
            continue
        seen.add(key)
        components.append(comp)

bom = {
    "bomFormat": "CycloneDX",
    "specVersion": "1.3",
    "version": 1,
    "metadata": {
        "component": {
            "type": "application",
            "name": "embedded-database",
            "version": "0.1.0",
        }
    },
    "components": components,
}
with open("sbom.json", "w") as f:
    json.dump(bom, f, indent=2)
print(f"SBOM: {len(components)} components")
PY
  else
    echo "WARN: SBOM 生成失败，跳过"
  fi
else
  echo "跳过 SBOM 生成（cargo-cyclonedx 不可用）"
fi

echo "=== 离线构建完成 ==="
echo "Vendor 目录: vendor/"
if [ -f sbom.json ]; then
  echo "SBOM: sbom.json"
else
  echo "SBOM: 未生成（已跳过）"
fi
