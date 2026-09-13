#!/usr/bin/env bash
#
# 開発環境が構成されているかを確認するスモークテスト。
#
#   1. nix/packages.nix のツールがコマンドとして揃っていること
#   2. その実体が Nix の store にあること
#   3. Rust toolchain が Xtensa target を持つこと
#
# 1 だけではホストに元から入っているツールを拾ってしまうため、2 を併せて見る。
#
#   使用方法: scripts/check-env.sh
set -euo pipefail

# nix/packages.nix に含まれるツールのうち、コマンドとして使用するもの。
# 本リストを変更した場合は nix/packages.nix 側にも同じものを追加する。
required_commands=(
  bash
  cargo
  cargo-deny
  deadnix
  espflash
  fd
  git
  jq
  make
  nixfmt
  rg
  rustc
  shellcheck
  shfmt
  statix
  tree
  xtensa-esp32s3-elf-gcc
)

store_dir=${NIX_STORE_DIR:-/nix/store}

resolve_path() {
  realpath -e -- "$1" 2>/dev/null || printf '%s' "$1"
}

missing=()
foreign=()

for cmd in "${required_commands[@]}"; do
  if ! path=$(command -v "$cmd" 2>/dev/null); then
    printf '  MISSING %-24s\n' "$cmd"
    missing+=("$cmd")
    continue
  fi

  real=$(resolve_path "$path")

  case "$real" in
    "$store_dir"/*)
      printf '  ok      %-24s %s\n' "$cmd" "$real"
      ;;
    *)
      printf '  FOREIGN %-24s %s\n' "$cmd" "$real"
      foreign+=("$cmd")
      ;;
  esac
done

echo

if [ "${#missing[@]}" -ne 0 ]; then
  echo "不足しているコマンド: ${missing[*]}" >&2
  echo "開発環境の外で実行されている可能性があります。scripts/container.sh shell または 'nix develop' を使用してください。" >&2
  exit 1
fi

if [ "${#foreign[@]}" -ne 0 ]; then
  echo "Nix の store 由来でないコマンド: ${foreign[*]}" >&2
  echo "ホストのツールが混ざっています。同名のコマンドが PATH の前方にある可能性があります。" >&2
  exit 1
fi

# Xtensa target の有無。Espressif fork でなければここで失敗する。
if ! rustc --print target-list | grep -qx 'xtensa-esp32s3-none-elf'; then
  echo "rustc が xtensa-esp32s3-none-elf target を持ちません: $(rustc --version)" >&2
  exit 1
fi
echo "rustc: $(rustc --version) (xtensa-esp32s3-none-elf 対応)"

if [ "${MY_STACKCHAN_ENV:-}" = nix-develop ]; then
  state="開発シェル内、MY_STACKCHAN_ENV=nix-develop"
else
  state="開発シェル外、PATH 上のツールが store を指している"
fi

echo "開発環境は正常です ($state)。"
