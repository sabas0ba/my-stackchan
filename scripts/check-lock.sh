#!/usr/bin/env bash
#
# flake.nix の入力と flake.lock の整合を検査する。ネットワークは使用しない。
#
#   - flake.nix の入力がすべて 40 桁の rev で指定されていること
#   - その rev が flake.lock に同一の値で記録されていること
#   - flake.lock の各ノードが narHash を持つこと
#
# nix 自身は、入力がブランチ名で参照されていても正常として扱う。lock には rev が
# 記録されるため評価は再現するが、lock を再生成した時点で追従先が変わる。
# 本検査はこれをオフラインで検出する。
#
#   使用方法: scripts/check-lock.sh
set -euo pipefail

cd "$(dirname "$0")/.."

if [ ! -f flake.lock ]; then
  echo "flake.lock がありません。scripts/container.sh lock で生成してください。" >&2
  exit 1
fi

status=0

# flake.nix から `<name>.url = "github:owner/repo/<rev>"` を抽出する。
while IFS= read -r line; do
  name=$(printf '%s' "$line" | sed -E 's/^[[:space:]]*([A-Za-z0-9_-]+)\.url.*/\1/')
  ref=$(printf '%s' "$line" | sed -E 's/.*"github:([^"]+)".*/\1/')
  rev=${ref##*/}

  if ! [[ $rev =~ ^[0-9a-f]{40}$ ]]; then
    echo "NG: 入力 $name が rev で固定されていません: $ref" >&2
    status=1
    continue
  fi

  locked_rev=$(jq -r --arg n "$name" '.nodes[$n].locked.rev // empty' flake.lock)
  if [ "$locked_rev" != "$rev" ]; then
    echo "NG: 入力 $name の rev が flake.lock と一致しません (flake.nix: $rev, lock: ${locked_rev:-なし})" >&2
    status=1
    continue
  fi

  nar_hash=$(jq -r --arg n "$name" '.nodes[$n].locked.narHash // empty' flake.lock)
  if [ -z "$nar_hash" ]; then
    echo "NG: 入力 $name の narHash が flake.lock にありません" >&2
    status=1
    continue
  fi

  echo "ok: $name $rev"
done < <(grep -E '^[[:space:]]*[A-Za-z0-9_-]+\.url[[:space:]]*=[[:space:]]*"github:' flake.nix)

exit "$status"
