#!/bin/sh
#
# コンテナの ENTRYPOINT。
#
# イメージのビルド時に `nix develop --profile "$MY_STACKCHAN_PROFILE"` で開発シェルを
# profile として実体化しているため、本スクリプトでは flake を再評価せずその profile に
# 入る。実行時のネットワークを必要とせず、起動が速い。
#
# ベースイメージ (nixos/nix) における bash の存在を前提としないため POSIX sh で記述する。
set -eu

profile="${MY_STACKCHAN_PROFILE:-/nix/var/nix/profiles/my-stackchan-dev}"

if [ ! -e "$profile" ]; then
  echo "開発 profile が見つかりません: $profile" >&2
  echo "イメージのビルドが失敗している可能性があります。" >&2
  exit 1
fi

# マウントしたリポジトリの所有者 (ホストのユーザー) とコンテナ内の実行ユーザー (root) が
# 異なると、git と nix (libgit2) が所有者の不一致を理由にリポジトリを開かない。
# コンテナ内に閉じた設定として、すべてのディレクトリを safe.directory に加える。
gitconfig="${HOME:-/root}/.gitconfig"
if [ ! -f "$gitconfig" ] || ! grep -q 'directory = \*' "$gitconfig"; then
  printf '[safe]\n\tdirectory = *\n' >>"$gitconfig"
fi

if [ "$#" -eq 0 ]; then
  exec nix develop "$profile" --command bash
fi

exec nix develop "$profile" --command "$@"
