#!/usr/bin/env bash
#
# flake 以外の外部の成果物が一意に固定されていることを検査する。ネットワークは使用しない。
#
#   - Dockerfile の FROM がダイジェストで固定されていること
#   - GitHub Actions の uses がコミット SHA で固定されていること
#   - ワークフローの runs-on が -latest でないこと
#   - nix/esp-rust.nix と nix/xtensa-gcc.nix の配布物が 64 桁の sha256 を持つこと
#
#   使用方法: scripts/check-pins.sh
set -euo pipefail

cd "$(dirname "$0")/.."

status=0

# --- Dockerfile ---------------------------------------------------------------
digest=$(grep -E '^ARG NIX_IMAGE_DIGEST=' Dockerfile | sed -E 's/^ARG NIX_IMAGE_DIGEST=//')
if ! [[ $digest =~ ^sha256:[0-9a-f]{64}$ ]]; then
  echo "NG: Dockerfile の NIX_IMAGE_DIGEST がダイジェストではありません: $digest" >&2
  status=1
fi
if ! grep -qE '^FROM nixos/nix:\$\{NIX_VERSION\}@\$\{NIX_IMAGE_DIGEST\}$' Dockerfile; then
  echo "NG: Dockerfile の FROM がダイジェスト付きの形式ではありません" >&2
  status=1
fi

# --- GitHub Actions -----------------------------------------------------------
while IFS= read -r file; do
  while IFS= read -r uses; do
    ref=${uses##*@}
    if ! [[ $ref =~ ^[0-9a-f]{40}$ ]]; then
      echo "NG: $file の uses がコミット SHA で固定されていません: $uses" >&2
      status=1
    fi
  done < <(grep -oE 'uses:[[:space:]]*[^[:space:]]+' "$file" | sed -E 's/uses:[[:space:]]*//')

  if grep -qE 'runs-on:.*-latest' "$file"; then
    echo "NG: $file の runs-on が -latest です" >&2
    status=1
  fi
done < <(find .github/workflows -type f \( -name '*.yml' -o -name '*.yaml' \) 2>/dev/null)

# --- toolchain の配布物 ---------------------------------------------------------
for f in nix/esp-rust.nix nix/xtensa-gcc.nix; do
  count=$(grep -cE 'sha256 = "[0-9a-f]{64}";' "$f" || true)
  urls=$(grep -cE 'url = ' "$f" || true)
  if [ "$count" -eq 0 ] || [ "$count" -ne "$urls" ]; then
    echo "NG: $f の配布物 ($urls 件) に対し 64 桁の sha256 が $count 件です" >&2
    status=1
  fi
done

if [ "$status" -eq 0 ]; then
  echo "ok: 外部の成果物はすべて固定されています"
fi

exit "$status"
