# cargo の依存を Nix の store に取り込み、ネットワーク無しで build できるようにする。
#
# 取り込む対象は 3 つ:
#   - Cargo.lock (host 側 workspace) に記録された crate
#   - firmware/Cargo.lock に記録された crate
#   - rust-src に同梱された vendor (build-std が core/alloc を build する際に、
#     library/Cargo.lock の依存を crates.io から取得しようとするため)
#
# importCargoLock は Cargo.lock の checksum で各 crate を固定して取得する。3 つを 1 つの
# directory source に合成し、開発シェルの CARGO_HOME/config.toml で crates.io の
# 置き換え先とする (nix/devshell.nix)。
#
# Cargo.lock を更新した場合はイメージの再構築が必要になる。更新の手順は
# docs/environment.md を参照する。
{
  pkgs,
  espRust,
}:

let
  hostVendor = pkgs.rustPlatform.importCargoLock { lockFile = ../Cargo.lock; };
  firmwareVendor = pkgs.rustPlatform.importCargoLock { lockFile = ../firmware/Cargo.lock; };
  rustSrcVendor = "${espRust}/lib/rustlib/src/rust/library/vendor";
in
pkgs.runCommandLocal "my-stackchan-cargo-vendor" { } ''
  mkdir -p "$out"
  # 同じ crate (name-version) が複数の元にある場合は最初のものを採る。内容は checksum で
  # 同一であることが保証されている。
  for src in ${hostVendor} ${firmwareVendor} ${rustSrcVendor}; do
    for crate in "$src"/*/; do
      crate=''${crate%/}
      name=$(basename "$crate")
      if [ ! -e "$out/$name" ]; then
        ln -s "$crate" "$out/$name"
      fi
    done
  done
''
