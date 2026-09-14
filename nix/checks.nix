# `nix flake check` (`make check`) が実行する検査。
#
# ローカル、CI、コンテナのいずれでも同一の derivation が実行される。
# Rust の検査 (fmt, clippy, test) は cargo の vendoring が必要であり、ここではなく
# Makefile の check から開発シェル内で実行する。
{ pkgs, src }:

let
  # 各検査で共通に使用するヘルパー。検査が成功した場合のみ $out を生成する。
  mkCheck =
    name: deps: script:
    pkgs.runCommandLocal "check-${name}" { nativeBuildInputs = deps; } ''
      cd ${src}
      ${script}
      touch "$out"
    '';
in
{
  # Nix コードが nixfmt で整形済みであること。
  nixfmt = mkCheck "nixfmt" [ pkgs.nixfmt pkgs.findutils ] ''
    find . -type f -name '*.nix' -exec nixfmt --check {} +
  '';

  # Nix コードの静的解析。
  statix = mkCheck "statix" [ pkgs.statix ] ''
    statix check .
  '';

  # 未使用の let 束縛および関数引数の検出。
  deadnix = mkCheck "deadnix" [ pkgs.deadnix ] ''
    deadnix --fail .
  '';

  # flake.nix の入力と flake.lock の整合。ネットワークは使用しない。
  lock = mkCheck "lock" [ pkgs.jq ] ''
    bash scripts/check-lock.sh
  '';

  # 外部の成果物 (ベースイメージ、GitHub Actions、toolchain の配布物) が
  # 一意に固定されていること。
  pins = mkCheck "pins" [ pkgs.gnugrep pkgs.findutils ] ''
    bash scripts/check-pins.sh
  '';

  # シェルスクリプトの静的解析。
  shellcheck = mkCheck "shellcheck" [ pkgs.shellcheck ] ''
    shellcheck scripts/*.sh
    shellcheck --shell=bash .envrc
  '';

  # シェルスクリプトが shfmt で整形済みであること。
  shfmt = mkCheck "shfmt" [ pkgs.shfmt ] ''
    shfmt --diff --indent 2 --case-indent scripts/*.sh
  '';
}
