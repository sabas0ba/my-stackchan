{
  description = "my-stackchan: CoreS3 向け Rust firmware と host CLI の開発環境";

  inputs = {
    # 入力はリビジョンで固定する。ブランチ名による参照は flake.lock が無い環境で
    # 取得結果が変動するため使用しない。sabas0ba/dotfiles と同一の rev に揃える。
    nixpkgs.url = "github:NixOS/nixpkgs/597283ad8aa0b331c788e97c4c262d58877074ef"; # nixos-26.05
  };

  outputs =
    { self, nixpkgs }:
    let
      # Xtensa 向け Rust toolchain と GCC の配布物が x86_64-linux 向けにのみ固定してある。
      # 他の system を足す場合は nix/esp-rust.nix と nix/xtensa-gcc.nix に該当する
      # 配布物の sha256 を追加する。
      systems = [ "x86_64-linux" ];

      forAllSystems =
        f:
        nixpkgs.lib.genAttrs systems (
          system:
          f (
            import nixpkgs {
              inherit system;
              config = { };
              overlays = [ ];
            }
          )
        );
    in
    {
      # `nix develop` および direnv の `use flake` が使用する開発シェル。
      devShells = forAllSystems (pkgs: {
        default = import ./nix/devshell.nix { inherit pkgs; };
      });

      # ツール一式を 1 つの profile にまとめたもの。Dockerfile から使用する。
      packages = forAllSystems (pkgs: {
        default = pkgs.buildEnv {
          name = "my-stackchan-toolchain";
          paths = import ./nix/packages.nix { inherit pkgs; };
        };
        esp-rust = pkgs.callPackage ./nix/esp-rust.nix { };
        xtensa-gcc = pkgs.callPackage ./nix/xtensa-gcc.nix { };
      });

      # `nix flake check` および `make check` が実行する検査。
      checks = forAllSystems (
        pkgs:
        import ./nix/checks.nix {
          inherit pkgs;
          src = self;
        }
      );

      # `nix fmt` が使用するフォーマッタ。引数無しで起動された場合に対象を補う。
      formatter = forAllSystems (
        pkgs:
        pkgs.writeShellApplication {
          name = "my-stackchan-fmt";
          runtimeInputs = [
            pkgs.nixfmt
            pkgs.findutils
          ];
          text = ''
            if [ "$#" -gt 0 ]; then
              nixfmt "$@"
              exit 0
            fi
            find . \
              -type d \( -name .git -o -name .direnv -o -name .work -o -name target \) -prune -o \
              -type f -name '*.nix' -exec nixfmt {} +
          '';
        }
      );
    };
}
