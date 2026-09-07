# ホストの `nix develop` と同一の環境をコンテナ内に構築する。
#
# 方針は sabas0ba/dotfiles の Dockerfile に従う。
#   - ツールの一覧を本ファイルに記述しない。flake.nix および nix/packages.nix を参照する。
#   - ビルド時に開発シェルを Nix の profile として実体化し、実行時はその profile に
#     入るだけとする (scripts/docker-entrypoint.sh)。
#   - `# syntax=docker/dockerfile:1` は指定しない (外部フロントエンドの取得を避ける)。
#
# 使用方法 (scripts/container.sh が同じ操作を包む):
#   podman build -t my-stackchan-dev .
#   podman run --rm -it -v "$PWD:/workspace" my-stackchan-dev
#   podman run --rm -v "$PWD:/workspace" my-stackchan-dev scripts/check-env.sh

# ベースイメージはタグに加えてダイジェストで固定する。
# 更新時は NIX_VERSION と NIX_IMAGE_DIGEST を同時に変更する。
ARG NIX_VERSION=2.35.1
ARG NIX_IMAGE_DIGEST=sha256:377d4887aca98f0dfa12971c1ea6d6a625a435d8b610d4c95a436843da6fbfd1
FROM nixos/nix:${NIX_VERSION}@${NIX_IMAGE_DIGEST}

# sandbox と filter-syscalls を無効化しているのは、コンテナの seccomp と Nix の
# サンドボックスが競合するため。本イメージでは配布物の取得と展開のみを行う。
# flake-registry を空にし、名前 `nixpkgs` は後段で固定した nixpkgs に解決させる。
RUN mkdir -p /etc/nix \
  && printf '%s\n' \
  'experimental-features = nix-command flakes' \
  'sandbox = false' \
  'filter-syscalls = false' \
  'max-jobs = auto' \
  'flake-registry = ' \
  >> /etc/nix/nix.conf

# 実体化した開発シェルの配置先。scripts/docker-entrypoint.sh と一致させる。
ENV MY_STACKCHAN_PROFILE=/nix/var/nix/profiles/my-stackchan-dev

# cargo の vendor の書込可能な複製の配置先 (理由は nix/devshell.nix)。マウントされる
# /workspace の外に置き、開発シェルの実体化時に作成する。
ENV MY_STACKCHAN_VENDOR_DIR=/opt/my-stackchan/cargo-vendor

WORKDIR /workspace

# 環境の定義のみを先に配置する。これを独立したレイヤにすることで、ソースの変更で
# toolchain の再取得が発生しない。Cargo.lock は依存の vendoring (nix/cargo-vendor.nix)
# が参照するため、ここに含める。
COPY flake.nix flake.lock Cargo.lock ./
COPY firmware/Cargo.lock ./firmware/
COPY nix ./nix

# 開発シェルを profile として実体化する。profile は GC ルートであるため、以降この
# 閉包は削除されない。toolchain の配布物 (約 300 MB) はここで取得される。
RUN nix develop --profile "$MY_STACKCHAN_PROFILE" --command true \
  && nix flake archive --json > /dev/null \
  && nix registry add nixpkgs \
  "path:$(nix eval --raw --impure --expr '(builtins.getFlake "/workspace").inputs.nixpkgs.outPath')" \
  && rm -rf /root/.cache/nix

# ソース本体を配置する。実行時に -v "$PWD:/workspace" を指定した場合はマウントで
# 上書きされる。
COPY . .

COPY scripts/docker-entrypoint.sh /usr/local/bin/my-stackchan-entrypoint.sh
RUN chmod +x /usr/local/bin/my-stackchan-entrypoint.sh

ENTRYPOINT ["/bin/sh", "/usr/local/bin/my-stackchan-entrypoint.sh"]
CMD []
