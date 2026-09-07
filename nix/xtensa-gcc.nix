# Xtensa 向け GCC toolchain (xtensa-esp-elf)。
#
# Espressif fork の Rust は Xtensa target のリンクに GCC を使用する。espup が既定で
# 取得する crosstool-NG の配布物を、同一の版と sha256 で固定して取得する。
#
# 更新時は version と sha256 を同時に変更する。値の取得元は GitHub Releases の
# asset digest (docs/dependencies.md)。
{
  lib,
  stdenv,
  fetchurl,
  autoPatchelfHook,
  zlib,
}:

let
  version = "15.2.0_20250920";
in
stdenv.mkDerivation {
  pname = "xtensa-esp-elf";
  inherit version;

  src = fetchurl {
    url = "https://github.com/espressif/crosstool-NG/releases/download/esp-${version}/xtensa-esp-elf-${version}-x86_64-linux-gnu.tar.xz";
    sha256 = "e3d77ad14544814527bbe7a2d0f79ec4592a4e23392c51c7388c0e686b6a6977";
  };

  nativeBuildInputs = [ autoPatchelfHook ];

  buildInputs = [
    stdenv.cc.cc.lib
    zlib
  ];

  dontConfigure = true;
  dontBuild = true;
  dontStrip = true;

  installPhase = ''
    runHook preInstall
    mkdir -p "$out"
    cp -r . "$out/"
    runHook postInstall
  '';

  # target 向けの object や library は host の ELF ではないため依存解決の対象外とする。
  autoPatchelfIgnoreMissingDeps = [ "*" ];

  meta = {
    description = "GCC toolchain for Xtensa ESP32 series (crosstool-NG, Espressif build)";
    homepage = "https://github.com/espressif/crosstool-NG";
    license = lib.licenses.gpl3Plus;
    platforms = [ "x86_64-linux" ];
  };
}
