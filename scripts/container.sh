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
#     scripts/container.sh daemon [args...] daemon と同梱の plugin を build し、plugin ごとの
#                                           コンテナと daemon を起動する
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
#     STACKCHAN_RELAY_NETWORK  中継のコンテナのネットワーク。既定はエンジンの既定値。
#                              daemon と plugin のコンテナは常にネットワークを持たない
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
    mkdir -p "$config_dir/logs" "$config_dir/spool"
    # build は書込み可能なマウントで先に行う。
    prepare_workspace_mounts
    "$engine" run --rm --network none "${workspace_mounts[@]}" "$image" \
      cargo build --release --locked --offline -p my-stackchan-host -p stackchan-clock -p stackchan-image-demo
    # 以降のコンテナは、開発シェルの初期化 (リポジトリに書き込む) を通さず、build 済みの
    # バイナリを直接起動する (バイナリは Nix store の動的リンカを絶対パスで参照するため、
    # イメージ内で動く)。
    stackchan_bin="$root/target/release/stackchan"

    # plugin ごとにコンテナを分ける (docs/plugin.md の「plugin ごとのコンテナ分離」)。
    # 起動に必要な項目は、設定の構文の解釈をここに重複させないよう stackchan に出力させる。
    plan=$("$engine" run --rm --network none \
      -v "$stackchan_bin:/stackchan:ro" -v "$config_dir:/config:ro" \
      --entrypoint /stackchan "$image" --config-dir /config config --launch-plan)

    started=()
    volumes=()
    stop_plugins() {
      if [ ${#started[@]} -gt 0 ]; then
        "$engine" rm -f -t 0 "${started[@]}" >/dev/null 2>&1 || true
      fi
      if [ ${#volumes[@]} -gt 0 ]; then
        "$engine" volume rm -f "${volumes[@]}" >/dev/null 2>&1 || true
      fi
    }
    trap stop_plugins EXIT

    daemon_volumes=()
    while IFS=$'\t' read -r kind id dir; do
      dir=${dir%$'\r'}
      [ "$kind" = plugin ] || continue
      # socket は named volume に置く。設定ディレクトリは Windows 側にあり、その上では
      # Unix socket を作成できない場合がある。volume は daemon と当該 plugin にだけ渡す。
      volume="stackchan-run-$id"
      container="stackchan-plugin-$id"
      # 前回の起動が異常終了して残したものを取り除く。
      "$engine" rm -f -t 0 "$container" >/dev/null 2>&1 || true
      "$engine" volume rm -f "$volume" >/dev/null 2>&1 || true
      "$engine" volume create "$volume" >/dev/null
      volumes+=("$volume")
      daemon_volumes+=(-v "$volume:/run/stackchan/$id")
      # plugin のコンテナには、ネットワークも設定ディレクトリもリポジトリも渡さない。
      # 渡すのは自身の実行ファイルのディレクトリ、自身の volume、起動補助だけである。
      "$engine" run -d --name "$container" \
        --network none --read-only --cap-drop=all --security-opt no-new-privileges \
        --pids-limit 64 --memory 256m \
        -v "${dir:-$root/target/release}:/plugin:ro" \
        -v "$volume:/run/stackchan" \
        -v "$stackchan_bin:/stackchan:ro" \
        --entrypoint /stackchan "$image" \
        plugin-run --socket /run/stackchan/plugin.sock >/dev/null
      started+=("$container")
    done <<<"$plan"

    # 利用者設定の net.allow に列挙された宛先への中継 (docs/plugin.md の「接続先の許可」)。
    # ネットワークを持つのはこのコンテナだけである。宛先ごとの socket を、当該 plugin の
    # volume に置く。宛先は引数で固定し、設定ディレクトリは渡さない。
    relay_volumes=()
    relay_routes=()
    relayed_id=
    while IFS=$'\t' read -r kind id index addr; do
      addr=${addr%$'\r'}
      [ "$kind" = relay ] || continue
      # 起動計画は同じ plugin の宛先を続けて出力するため、直前の id と比べれば重複しない。
      if [ "$id" != "$relayed_id" ]; then
        relay_volumes+=(-v "stackchan-run-$id:/run/stackchan/$id")
        relayed_id=$id
      fi
      relay_routes+=(--route "/run/stackchan/$id/net/$index.sock=$addr")
    done <<<"$plan"
    if [ ${#relay_routes[@]} -gt 0 ]; then
      relay_network=()
      if [ -n "${STACKCHAN_RELAY_NETWORK:-}" ]; then
        relay_network=(--network "$STACKCHAN_RELAY_NETWORK")
      fi
      "$engine" rm -f -t 0 stackchan-relay >/dev/null 2>&1 || true
      "$engine" run -d --name stackchan-relay \
        "${relay_network[@]}" --read-only --cap-drop=all --security-opt no-new-privileges \
        --pids-limit 64 --memory 256m \
        "${relay_volumes[@]}" \
        -v "$stackchan_bin:/stackchan:ro" \
        --entrypoint /stackchan "$image" \
        relay "${relay_routes[@]}" >/dev/null
      started+=(stackchan-relay)
    fi

    # daemon にはネットワークを渡さない。リポジトリは読取り専用とし (ピッチ補正の既定の
    # 保存先を読むため)、git のディレクトリは渡さない。設定ディレクトリも読取り専用とし、
    # daemon が書く logs と spool だけを書込み可能にする。
    "$engine" run --rm "${tty_flags[@]}" --name stackchan-daemon \
      --network none \
      -v "$root:/workspace:ro" \
      -v "$config_dir:/config:ro" \
      -v "$config_dir/logs:/config/logs" \
      -v "$config_dir/spool:/config/spool" \
      "${daemon_volumes[@]}" \
      --device "$serial_device:$serial_device" \
      --entrypoint /workspace/target/release/stackchan \
      "$image" --config-dir /config daemon --socket-dir /run/stackchan "$@"
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
