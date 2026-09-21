#!/usr/bin/env bash
# Ping 対応 firmware を書き込んだ実機で実行する。書き込みは別途 make flash で行う。
set -euo pipefail

cd "$(dirname "$0")/.."
port=${1:-${SERIAL_DEVICE:-/dev/ttyACM0}}

cargo run --locked --offline -p my-stackchan-host -- ping --port "$port"
STACKCHAN_TEST_PORT="$port" cargo test --locked --offline -p my-stackchan-host \
  -- --ignored --exact tests::hardware_ping_and_frame_recovery

# 実機の版を変えずに、独立した host のコピーで互換性不一致を再現する。
# 通常ビルドの成果物とソースを変更せず、検証記録は .work 内に保持する。
mkdir -p .work
snapshot=$(mktemp -d .work/ping-device.XXXXXX)
cp Cargo.toml Cargo.lock "$snapshot/"
cp -R crates "$snapshot/"
version_file="$snapshot/crates/protocol/src/lib.rs"
current_version=$(sed -n 's/^pub const VERSION: u8 = \([0-9]*\);$/\1/p' "$version_file")
if [[ ! $current_version =~ ^[0-9]+$ ]]; then
  echo 'protocol::VERSION を読み取れません' >&2
  exit 1
fi
incompatible_version=$(((current_version + 1) % 256))
sed -i "s/^pub const VERSION: u8 = $current_version;$/pub const VERSION: u8 = $incompatible_version;/" "$version_file"
CARGO_TARGET_DIR="$PWD/$snapshot/target" cargo build \
  --manifest-path "$snapshot/Cargo.toml" --locked --offline -p my-stackchan-host

status=0
"$snapshot/target/debug/stackchan" ping --port "$port" >"$snapshot/mismatch.log" 2>&1 || status=$?
cat "$snapshot/mismatch.log"
if [[ $status != 1 ]] || ! grep -Fq \
  "プロトコル版が一致しません: host=$incompatible_version, firmware=$current_version" "$snapshot/mismatch.log"; then
  echo '実機応答に対する版不一致の拒否を確認できませんでした' >&2
  exit 1
fi

# 不一致の試験後も通常の host で実機と疎通できることを確認する。
cargo run --locked --offline -p my-stackchan-host -- ping --port "$port"
printf '実機検証成功。版不一致の記録: %s/mismatch.log\n' "$snapshot"
