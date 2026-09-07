# Xtensa (ESP32-S3) 対応の Rust toolchain。
#
# esp-rs/rust-build が配布する Espressif fork の tarball を sha256 で固定して取得し、
# 同梱の install.sh で sysroot として配置する。rustup および espup は使用しない。
# 配置後の binary は Nix store 外の loader を参照しているため autoPatchelf で修正する。
#
# 更新時は version と 2 つの sha256 を同時に変更する。値の取得元は GitHub Releases の
# asset digest (docs/dependencies.md)。
{
  lib,
  stdenv,
  fetchurl,
  autoPatchelfHook,
  zlib,
  zstd,
  openssl,
}:

let
  version = "1.97.0.0";
  hostTriple = "x86_64-unknown-linux-gnu";
  baseUrl = "https://github.com/esp-rs/rust-build/releases/download/v${version}";

  rustSrc = fetchurl {
    url = "${baseUrl}/rust-src-${version}.tar.xz";
    sha256 = "568d688b9f8f332ec4d04657544fad23e99ce11e9e6cf5835979e68a28c68b73";
  };
in
stdenv.mkDerivation {
  pname = "esp-rust";
  inherit version;

  src = fetchurl {
    url = "${baseUrl}/rust-${version}-${hostTriple}.tar.xz";
    sha256 = "a99bfee69221e9ff6d86388f6811ee688cd405e6a0400a3cd1784e8d463e9d99";
  };

  nativeBuildInputs = [ autoPatchelfHook ];

  # rustc は libLLVM (同梱) と libstdc++ を、cargo は zlib/openssl を動的に参照する。
  buildInputs = [
    stdenv.cc.cc.lib
    zlib
    zstd
    openssl
  ];

  dontConfigure = true;
  dontBuild = true;

  installPhase = ''
    runHook preInstall

    # rust-docs は容量が大きく開発シェルでは使用しないため除外する。
    bash ./install.sh \
      --destdir="$out" \
      --prefix= \
      --without=rust-docs-json-preview,rust-docs \
      --disable-ldconfig

    # build-std に必要な標準ライブラリのソースを同じ sysroot に配置する。
    mkdir -p rust-src
    tar -xf ${rustSrc} -C rust-src --strip-components=1
    bash ./rust-src/install.sh \
      --destdir="$out" \
      --prefix= \
      --disable-ldconfig

    # install.sh が残す manifest は store には不要であり、他の derivation との衝突を
    # 避けるため削除する。
    rm -rf "$out/lib/rustlib/install.log" "$out/lib/rustlib/uninstall.sh" \
      "$out/lib/rustlib/manifest-"* "$out/lib/rustlib/rust-installer-version"

    runHook postInstall
  '';

  # rust-src の .rlib 等には ELF でないファイルが多く、autoPatchelf の警告を抑える。
  autoPatchelfIgnoreMissingDeps = [ "*" ];

  meta = {
    description = "Rust toolchain with Xtensa support (Espressif fork)";
    homepage = "https://github.com/esp-rs/rust-build";
    license = with lib.licenses; [
      mit
      asl20
    ];
    platforms = [ "x86_64-linux" ];
  };
}
