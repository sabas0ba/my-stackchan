#!/usr/bin/env bash
# コンテナエンジンの代役で、最大構成の起動引数と正常・異常終了時の後始末を検査する。
# 実コンテナでの隔離・通信の検査は docs/environment.md の手順で別途行う。
set -euo pipefail

if [ $# -gt 0 ]; then
  state=${STACKCHAN_TEST_ENGINE_STATE:?}
  command=$1
  shift
  case "$command" in
    rm)
      # Docker が受け付けない -t 等を渡すと失敗させ、コンテナを残す。
      [ "$1" = -f ] || exit 2
      shift
      for name in "$@"; do
        [[ "$name" != -* ]] || exit 2
      done
      for name in "$@"; do
        rm -f "$state/container-$name"
      done
      ;;
    volume)
      action=$1
      shift
      case "$action" in
        create) touch "$state/volume-$1" ;;
        rm)
          [ "$1" = -f ] || exit 2
          shift
          for name in "$@"; do
            rm -f "$state/volume-$name"
          done
          ;;
        *) exit 2 ;;
      esac
      ;;
    run)
      name=
      pids=0
      while [ $# -gt 0 ]; do
        case "$1" in
          --launch-plan)
            cat "$state/plan"
            exit 0
            ;;
          --name)
            name=$2
            shift
            ;;
          --pids-limit)
            pids=$2
            shift
            ;;
        esac
        shift
      done
      if [ -n "$name" ]; then
        [ ! -e "$state/container-$name" ] || exit 3
        case "$name" in
          stackchan-relay)
            # 64 宛先、各 4 接続の worker と転送 thread、および main が同時に存在する。
            [ "$pids" -ge 577 ] || exit 4
            [ "$pids" -le 640 ] || exit 4
            ;;
          stackchan-daemon) exit "${STACKCHAN_TEST_DAEMON_EXIT:-0}" ;;
          *) [ "$pids" -eq 64 ] || exit 4 ;;
        esac
        touch "$state/container-$name"
      fi
      ;;
    *) exit 2 ;;
  esac
  exit 0
fi

root=$(cd "$(dirname "$0")/.." && pwd)
mkdir -p "$root/.work"
state=$(mktemp -d "$root/.work/container-test.XXXXXX")
export STACKCHAN_TEST_ENGINE_STATE="$state"
export STACKCHAN_CONFIG_DIR="$state/config"
export CONTAINER_ENGINE="$root/scripts/test-container.sh"

for plugin in $(seq 1 16); do
  printf 'plugin\tp%s\t\n' "$plugin"
  for endpoint in $(seq 0 3); do
    printf 'relay\tp%s\t%s\t127.0.0.1:%s\n' "$plugin" "$endpoint" "$((10000 + endpoint))"
  done
done >"$state/plan"

for expected in 0 0 42; do
  # 異常終了で残ったコンテナも起動前に削除できることを確かめる。
  touch "$state/container-stackchan-plugin-p1" "$state/container-stackchan-relay"
  actual=0
  STACKCHAN_TEST_DAEMON_EXIT=$expected bash "$root/scripts/container.sh" daemon || actual=$?
  [ "$actual" -eq "$expected" ] || {
    echo "daemon exit: $actual (expected $expected)" >&2
    exit 1
  }
  shopt -s nullglob
  remaining=("$state"/container-* "$state"/volume-*)
  [ ${#remaining[@]} -eq 0 ] || {
    printf 'cleanup failed: %s\n' "${remaining[@]}" >&2
    exit 1
  }
done
echo 'container launcher: maximum routes, stale cleanup, repeated start and failure cleanup passed'
