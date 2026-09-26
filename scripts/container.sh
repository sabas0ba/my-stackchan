#!/usr/bin/env bash
#
# コンテナ環境の操作の入り口。Windows (Git Bash) と Linux の双方から使用する。
#
# Windows ホストには nix も make も無いため、Makefile の docker-* 相当を本スクリプトが
# 提供する。コンテナ内では Makefile を使用する。
#
#   使用方法:
#     scripts/container.sh build            イメージを構築する
#     scripts/container.sh lock             flake.lock を生成する (ネットワークを使用)
#     scripts/container.sh shell            コンテナ内の開発シェルに入る
#     scripts/container.sh check            コンテナ内で make check を実行する (--network none)
#     scripts/container.sh run <cmd...>     コンテナ内で任意のコマンドを実行する
#     scripts/container.sh device <cmd...>  USB デバイスを渡してコマンドを実行する
#     scripts/container.sh daemon [args...] daemon と同梱の plugin を build して起動する
#                                           (args は stackchan daemon に渡す。例: --trace)
#     scripts/container.sh cli <args...>    daemon と同じ設定ディレクトリで stackchan を
#                                           実行する (例: notify --text done)
#
#   環境変数:
#     CONTAINER_ENGINE  既定 podman。docker も可
#     IMAGE             既定 my-stackchan-dev
#     SERIAL_DEVICE     既定 /dev/ttyACM0。device と daemon サブコマンドでコンテナに渡す
#     STACKCHAN_CONFIG_DIR     daemon と cli の設定ディレクトリ。コンテナの /config にマウントする。
#                              既定は Windows では %APPDATA%\stackchan、それ以外では
#                              ${XDG_CONFIG_HOME:-$HOME/.config}/stackchan
#     STACKCHAN_DAEMON_NETWORK daemon のコンテナのネットワーク。既定 none。LAN 上の機器を
#                              使う plugin を動かす場合にだけ明示する (例: bridge)
set -euo pipefail

engine=${CONTAINER_ENGINE:-podman}
image=${IMAGE:-my-stackchan-dev}
serial_device=${SERIAL_DEVICE:-/dev/ttyACM0}

root=$(cd "$(dirname "$0")/.." && pwd)

# Git Bash (MSYS) では /c/Users/... 形式のパスをエンジンに渡すと変換されて壊れる。
# Windows 形式に直し、以降のパス変換を抑止する。
host_os=$(uname -o 2>/dev/null || true)
case "$host_os" in
  Msys | Cygwin)
    root=$(cd "$root" && pwd -W)
    export MSYS_NO_PATHCONV=1
    ;;
esac

work="$root/.work"
mkdir -p "$work"

# 端末から呼ばれた場合のみ対話モードにする。スクリプトや CI からの呼び出しで -t を
# 付けるとエンジンが失敗するため。
tty_flags=()
if [ -t 0 ] && [ -t 1 ]; then
  tty_flags=(-it)
fi

# Dockerfile の ARG から nix のベースイメージを取り、lock の生成にも同じ版を使う。
base_image() {
  local version digest
  version=$(grep -E '^ARG NIX_VERSION=' "$root/Dockerfile" | sed -E 's/^ARG NIX_VERSION=//')
  digest=$(grep -E '^ARG NIX_IMAGE_DIGEST=' "$root/Dockerfile" | sed -E 's/^ARG NIX_IMAGE_DIGEST=//')
  printf 'docker.io/nixos/nix:%s@%s' "$version" "$digest"
}

# リポジトリを /workspace にマウントするための引数を組み立てる。
#
# git worktree では .git がファイルであり、main checkout の .git/worktrees/<name> を
# ホストの絶対パスで指す。コンテナ内からはそのパスが存在しないため nix が flake を
# 開けない。main checkout の .git を併せてマウントし、コンテナ内のパスを指す .git
# ファイルで上書きする。
workspace_mounts=()
prepare_workspace_mounts() {
  workspace_mounts=(-v "$root:/workspace")

  if [ ! -f "$root/.git" ]; then
    return
  fi

  local gitdir main_git worktree
  gitdir=$(sed -E 's/^gitdir: //' "$root/.git")
  main_git=${gitdir%/worktrees/*}
  worktree=${gitdir##*/worktrees/}

  # worktree 側の .git ファイルと、main 側の worktrees/<name>/gitdir (worktree の位置を
  # ホストの絶対パスで持つ) の双方を、コンテナ内のパスを指す内容で上書きする。
  printf 'gitdir: /workspace-git/.git/worktrees/%s\n' "$worktree" >"$work/container-gitfile"
  printf '/workspace/.git\n' >"$work/container-worktree-gitdir"
  workspace_mounts+=(
    -v "$main_git:/workspace-git/.git"
    -v "$work/container-gitfile:/workspace/.git"
    -v "$work/container-worktree-gitdir:/workspace-git/.git/worktrees/$worktree/gitdir"
  )
}

# daemon と cli が同じ設定ディレクトリ (spool を含む) を使うよう、決め方を共通にする。
# 既定値は docs/plugin.md の「設定ディレクトリ」と同じ。
config_dir=
resolve_config_dir() {
  config_dir=${STACKCHAN_CONFIG_DIR:-}
  if [ -z "$config_dir" ]; then
    case "$host_os" in
      Msys | Cygwin) config_dir="$(cygpath -m "$APPDATA")/stackchan" ;;
      *) config_dir="${XDG_CONFIG_HOME:-$HOME/.config}/stackchan" ;;
    esac
  fi
  mkdir -p "$config_dir"
}

usage() {
  sed -n '2,/^set -euo pipefail/p' "$0" | sed -E 's/^# ?//' | sed '$d'
}

cmd=${1:-}
[ $# -gt 0 ] && shift

case "$cmd" in
  build)
    "$engine" build -t "$image" "$root"
    ;;
  lock)
    # flake.lock は nix でしか生成できず、イメージの構築前に必要なため、ベースイメージを
    # 直接使う。git を経由せず評価できるよう、flake の定義だけを作業ディレクトリへ複製し
    # path: 形式で与える。生成された flake.lock をリポジトリへ戻す。
    rm -rf "$work/flake-lock"
    mkdir -p "$work/flake-lock"
    cp "$root/flake.nix" "$work/flake-lock/"
    cp -r "$root/nix" "$work/flake-lock/"
    if [ -f "$root/flake.lock" ]; then
      cp "$root/flake.lock" "$work/flake-lock/"
    fi
    "$engine" run --rm -v "$work/flake-lock:/flake" "$(base_image)" \
      nix --extra-experimental-features 'nix-command flakes' \
      --option sandbox false --option filter-syscalls false \
      flake lock path:/flake
    cp "$work/flake-lock/flake.lock" "$root/flake.lock"
    echo "flake.lock を更新しました。"
    ;;
  shell)
    prepare_workspace_mounts
    "$engine" run --rm "${tty_flags[@]}" "${workspace_mounts[@]}" "$image"
    ;;
  check)
    prepare_workspace_mounts
    "$engine" run --rm --network none "${workspace_mounts[@]}" "$image" make check
    ;;
  run)
    prepare_workspace_mounts
    "$engine" run --rm "${tty_flags[@]}" "${workspace_mounts[@]}" "$image" "$@"
    ;;
  device)
    prepare_workspace_mounts
    "$engine" run --rm "${tty_flags[@]}" "${workspace_mounts[@]}" \
      --device "$serial_device:$serial_device" "$image" "$@"
    ;;
  daemon)
    # 設定ディレクトリは Windows 側の hook 等からも書けるよう、ホストのディレクトリを
    # マウントする。
    resolve_config_dir
    prepare_workspace_mounts
    # plugin の外部通信は既定で許可しない。必要な場合だけ STACKCHAN_DAEMON_NETWORK で明示する。
    # shellcheck disable=SC2016 # $@ はコンテナ内の bash で展開する。
    "$engine" run --rm "${tty_flags[@]}" --name stackchan-daemon \
      --network "${STACKCHAN_DAEMON_NETWORK:-none}" "${workspace_mounts[@]}" \
      --device "$serial_device:$serial_device" -v "$config_dir:/config" "$image" \
      bash -c 'cargo build --release --locked --offline -p my-stackchan-host -p stackchan-clock &&
        exec target/release/stackchan --config-dir /config daemon "$@"' daemon "$@"
    ;;
  cli)
    # daemon のコンテナと同じ設定ディレクトリを /config にマウントし、spool 経由の命令
    # (notify、status --via-daemon 等) が daemon に届くようにする。USB デバイスは渡さない。
    resolve_config_dir
    prepare_workspace_mounts
    # shellcheck disable=SC2016 # $@ はコンテナ内の bash で展開する。
    "$engine" run --rm "${tty_flags[@]}" --network none "${workspace_mounts[@]}" \
      -v "$config_dir:/config" "$image" \
      bash -c 'cargo run -q --locked --offline -p my-stackchan-host -- --config-dir /config "$@"' \
      cli "$@"
    ;;
  *)
    usage >&2
    exit 1
    ;;
esac
