# CLAUDE.md

Claude Code が本リポジトリで作業する際の補足。利用者全体の共通規約は `~/.claude/CLAUDE.md` にあり、本ファイルはリポジトリ固有の事項のみを記す。

## 文書の所在

- 設計: [docs/design.md](docs/design.md)
- プロトコル: [docs/protocol.md](docs/protocol.md)
- 開発環境と操作: [docs/environment.md](docs/environment.md)
- 依存の固定と調査記録: [docs/dependencies.md](docs/dependencies.md)

## 作業環境

作業はコンテナ内の開発シェルで行う (`scripts/container.sh shell`)。ホストに toolchain を導入しない。コンテナ内では `scripts/check-env.sh` で環境を確認する。

一時ファイルは `.work/` に置く。`CARGO_HOME` も `.work/cargo` である。

## 依存の追加・更新

- 追加前に RustSec / GHSA、侵害事例、公開日 (7 日のクールダウン) を調査し、`docs/dependencies.md` に記録する
- 配布物は sha256、crate は `Cargo.lock`、nixpkgs は rev と narHash で固定する
- Wi-Fi/BT に関わる crate (`esp-radio` 等) は利用者の許可なしに追加しない

## 検証

- `make check` (コンテナ内) または `scripts/container.sh check` (ホストから、`--network none`) を通す
- `Dockerfile` または `nix/` を変更した場合はイメージを再構築して検証する
- 実機の書込 (`make flash`) は利用者の許可を得てから行う

## 規約

- Conventional Commits
- コメントと文書は日本語。実装内容ではなく選択の理由を書く
- シェルスクリプトは `set -euo pipefail`、shellcheck と `shfmt --indent 2 --case-indent` を通す
