# `nix develop` および direnv が使用する開発シェルの定義。
#
# 本ファイルはシェルの構成のみを定義し、ツールの一覧は nix/packages.nix に置く。
# コンテナイメージも同一の packages.nix を参照するため、内容は常に一致する。
#
# mkShell (mkShellNoCC ではない) を使うのは、host 向け crate のリンクに cc が必要なため。
{ pkgs }:

let
  espRust = pkgs.callPackage ./esp-rust.nix { };
  cargoVendor = import ./cargo-vendor.nix { inherit pkgs espRust; };
in
pkgs.mkShell {
  name = "my-stackchan";

  packages = import ./packages.nix { inherit pkgs; };

  env = {
    # 開発シェル内であることをスクリプトから判定するために使用する。
    MY_STACKCHAN_ENV = "nix-develop";

    # ロケールによる挙動の差異を排除する。
    LC_ALL = "C.UTF-8";

    # cargo の状態 (registry cache 等) をホームではなくリポジトリ内に閉じ込める。
    # リポジトリ外へ書き込まず、`make clean-all` で完全に消せる状態を保つため。
    CARGO_HOME = "/workspace/.work/cargo";

    # vendoring した依存の所在 (Nix store)。shellHook が書込可能な複製を作る。
    MY_STACKCHAN_CARGO_VENDOR = "${cargoVendor}";
  };

  shellHook = ''
    # リポジトリのルートを基準とした PATH。scripts/ 配下を直接実行できるようにする。
    if root="$(git rev-parse --show-toplevel 2>/dev/null)"; then
      export MY_STACKCHAN_ROOT="$root"
      export PATH="$root/scripts:$PATH"
      # マウント先が /workspace でない環境 (Linux ホスト等) でもリポジトリ内に閉じ込める。
      export CARGO_HOME="$root/.work/cargo"
    fi
    mkdir -p "$CARGO_HOME"

    # vendor の書込可能な複製。
    #
    # Nix store のファイルは mode 0444 であり、build script が fs::copy で OUT_DIR へ
    # 複製すると mode が保存される (esp-rom-sys 等)。build script が再実行されたとき、
    # その 0444 ファイルへの上書きは、コンテナのマウント (9p) 上では root であっても
    # 拒否される。store を直接 directory source にせず、mode を落とした複製を使う。
    #
    # 複製先は MY_STACKCHAN_VENDOR_DIR で指定する。コンテナイメージでは /opt 以下を
    # 構築時に作る (Dockerfile)。指定が無い場合は CARGO_HOME 以下に作る。
    # 複製元の store パスを記録し、変わった場合 (Cargo.lock の更新) にのみ作り直す。
    vendor_dir="''${MY_STACKCHAN_VENDOR_DIR:-$CARGO_HOME/vendor}"
    if [ "$(cat "$vendor_dir.source" 2>/dev/null)" != "$MY_STACKCHAN_CARGO_VENDOR" ]; then
      echo "cargo: vendor の複製を作成中: $vendor_dir"
      rm -rf "$vendor_dir" "$vendor_dir.source"
      mkdir -p "$(dirname "$vendor_dir")"
      cp -rL --no-preserve=mode,ownership "$MY_STACKCHAN_CARGO_VENDOR" "$vendor_dir"
      chmod -R u+w "$vendor_dir"
      printf '%s\n' "$MY_STACKCHAN_CARGO_VENDOR" > "$vendor_dir.source"
    fi

    # 既定では crates.io を上記の複製で置き換え、ネットワークを使わない。
    # 依存の追加・更新 (Cargo.lock の変更) を行う場合のみ MY_STACKCHAN_ONLINE=1 で
    # 置き換えを外す。
    if [ "''${MY_STACKCHAN_ONLINE:-0}" = 1 ]; then
      rm -f "$CARGO_HOME/config.toml"
      echo "cargo: online (crates.io を直接参照)"
    else
      cat > "$CARGO_HOME/config.toml" <<EOF
    # nix/devshell.nix が生成する。手で編集しない。
    [source.crates-io]
    replace-with = "vendored"

    [source.vendored]
    directory = "$vendor_dir"
    EOF
    fi

    echo "my-stackchan dev shell (nixpkgs ${pkgs.lib.versions.majorMinor pkgs.lib.version}, ${pkgs.stdenv.hostPlatform.system})"
    echo "  make help: 利用可能な操作の一覧"
  '';
}
