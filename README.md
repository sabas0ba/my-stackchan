# my-stackchan

M5Stack CoreS3 (Stack-chan) に顔・テキスト・画像を表示し、PC から USB 経由で情報を流し込む。firmware と host CLI を Rust で実装する。

## 構成

```
crates/protocol   host と firmware が共有するメッセージ定義 (no_std)
crates/host       PC 側 CLI (port 検出、送信、collector)
crates/fontgen    GNU Unifont から埋め込み用ビットマップを生成する
firmware/         esp-hal ベースの firmware (独立した workspace)
nix/, Dockerfile  再現性のある開発環境
docs/             設計、プロトコル、環境、依存の記録
```

## はじめに

Windows (Podman + Git Bash):

```bash
scripts/container.sh lock
scripts/container.sh build
scripts/container.sh shell
```

詳細は [docs/environment.md](docs/environment.md) を参照する。

## ドキュメント

- [設計](docs/design.md)
- [プロトコル仕様](docs/protocol.md)
- [開発環境](docs/environment.md)
- [依存の固定と調査記録](docs/dependencies.md)

## ライセンス

[LICENSE](LICENSE) を参照する。
