#!/usr/bin/env bash
# 固定済みのフォント原本から生成し、CI ではコミット済みデータとの差分を検査する。
set -euo pipefail
cd "$(dirname "$0")/.."
mkdir -p .work/font
printf '%s  %s\n' \
  38b9e2888e4ad555dc71fee73676882bffca5b11ed174dacbecb9a5150c3f3fb \
  assets/fonts/unifont_jp_sample-16.0.04.hex.gz | sha256sum --check
gzip -dc assets/fonts/unifont_jp_sample-16.0.04.hex.gz |
  cargo run --quiet --locked --offline -p fontgen >.work/font/japanese.bin
case "${1:-check}" in
  generate) cp .work/font/japanese.bin assets/fonts/japanese.bin ;;
  check) cmp .work/font/japanese.bin assets/fonts/japanese.bin ;;
  *)
    echo 'usage: scripts/font.sh [generate|check]' >&2
    exit 2
    ;;
esac
