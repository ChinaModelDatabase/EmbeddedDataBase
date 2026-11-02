#!/usr/bin/env bash
# 供应链审计入口 — Spec 2
#   离线可跑：cargo vet check、cargo deny --offline check licenses/bans/sources
#   需网络  ：cargo deny check advisories（首次会克隆 RustSec advisory-db）
# 退出码：0=全部通过；2=advisories 未运行（无本地 RustSec 克隆，需网络）；其它非 0=cargo vet/deny 检查失败
set -euo pipefail

export PATH="/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH"

cd "$(dirname "$0")/.."

echo "== cargo-vet check（离线；信任来源：本地审计 + import + 带据豁免）=="
cargo vet check

echo
echo "== cargo-deny：licenses / bans / sources（离线）=="
cargo deny --offline check licenses bans sources

echo
echo "== cargo-deny：advisories（需网络）=="
# 注意：$CARGO_HOME/advisory-dbs 目录可能仅含 db.lock 而无实际克隆（实测），
# 因此必须检测真实克隆路径，否则离线时会以 cargo-deny 的不透明错误失败。
if [ -d "${CARGO_HOME:-$HOME/.cargo}/advisory-dbs/rustsec/advisory-db/.git" ]; then
  cargo deny check advisories
else
  echo "跳过：未发现本地 RustSec advisory 克隆（${CARGO_HOME:-$HOME/.cargo}/advisory-dbs/rustsec/advisory-db）。"
  echo "advisories 检查需网络，请联网后执行：cargo deny check advisories"
  exit 2
fi
