# 開発環境に導入するツールの単一情報源。
#
# 本ファイルの内容が
#   - `nix develop` および direnv のシェル (nix/devshell.nix 経由)
#   - `nix build .#default` の profile      (flake.nix 経由)
#   - コンテナイメージ                      (Dockerfile が上記 profile を使用)
# の 3 つすべてに反映される。ツールを追加する場合は本ファイルのみを編集する。
#
# Rust の toolchain は nixpkgs の rustc/cargo ではなく、Xtensa 対応の Espressif fork
# (nix/esp-rust.nix) を使用する。同 toolchain は x86_64-linux の std も含むため、
# host 側の crate も同じ toolchain で build する。nixpkgs の rustc/cargo を併置すると
# PATH の前後で挙動が変わるため、本ファイルに追加しない。
{ pkgs }:

with pkgs;
[
  # --- 基本ユーティリティ -------------------------------------------------
  coreutils
  findutils
  gnugrep
  gnused
  gnutar
  gzip
  xz
  less
  which

  # --- 検索・テキスト処理 -------------------------------------------------
  ripgrep
  fd
  jq
  tree

  # --- タスクランナー -----------------------------------------------------
  gnumake

  # --- バージョン管理 -----------------------------------------------------
  git

  # --- Nix の開発支援 -----------------------------------------------------
  nixfmt
  statix
  deadnix

  # --- シェルスクリプトの開発支援 -----------------------------------------
  bashInteractive
  shellcheck
  shfmt

  # --- Rust toolchain (Xtensa 対応の Espressif fork) ----------------------
  (callPackage ./esp-rust.nix { })
  (callPackage ./xtensa-gcc.nix { })

  # --- 書込ツール ---------------------------------------------------------
  espflash

  # --- 依存の検査 ---------------------------------------------------------
  # advisory と license の検査。advisory DB の取得にネットワークを要するため、
  # `make check` ではなく `make audit` から実行する。
  cargo-deny
]
