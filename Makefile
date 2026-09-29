# 本リポジトリに対する操作の入り口。利用可能な操作は `make help` で一覧する。
#
# 開発シェル (コンテナ内または nix develop) で使用する。Windows ホストからは
# scripts/container.sh を経由する。

SHELL := /usr/bin/env bash
.DEFAULT_GOAL := help

NIX ?= nix
CONTAINER_ENGINE ?= podman
IMAGE ?= my-stackchan-dev

# firmware は host とは別の workspace (target と build-std が異なるため)。
FIRMWARE_DIR := firmware

# Windows 向けの host CLI。Espressif fork の toolchain には windows-gnu の std が無いため
# build-std で構築する (docs/plugin.md)。linker の設定は開発シェルの環境変数にある。
# target dir を分けるのは、build-std の有無で std の成果物が衝突しないようにするため。
WINDOWS_TARGET := x86_64-pc-windows-gnu
WINDOWS_CARGO_FLAGS := --target $(WINDOWS_TARGET) -Z build-std=std,panic_abort \
	--target-dir target/$(WINDOWS_TARGET)-build-std

.PHONY: help
help: ## 本ヘルプを表示する
	@printf '使用方法: make <target>\n\n'
	@pattern='^([a-zA-Z0-9_-]+):.*## (.*)$$'; \
	for makefile in $(MAKEFILE_LIST); do \
		while IFS= read -r line; do \
			line=$${line%$$'\r'}; \
			if [[ $$line =~ $$pattern ]]; then \
				printf '  \033[36m%-16s\033[0m %s\n' \
					"$${BASH_REMATCH[1]}" "$${BASH_REMATCH[2]}"; \
			fi; \
		done < "$$makefile"; \
	done

# --- 環境 -------------------------------------------------------------------

.PHONY: env
env: ## 環境が構成されているかを確認する
	scripts/check-env.sh

.PHONY: lock
lock: ## flake.lock を生成する
	$(NIX) flake lock

# --- 検査 -------------------------------------------------------------------

.PHONY: check
check: ## すべての検査を実行する (nix flake check + 環境 + Rust の fmt/clippy/test)
	$(NIX) flake check
	scripts/check-env.sh
	$(MAKE) check-rust

.PHONY: check-rust
check-rust: ## Rust の検査 (fmt --check, clippy, test, firmware の build)
	cargo fmt --all --check
	cargo clippy --workspace --all-targets --locked --offline -- -D warnings
	cargo test --workspace --locked --offline
	cargo run --locked --offline -p protocol --example decoder_stress -- --cases 20000
	cargo run --locked --offline -p plugin-api --example frame_stress -- --cases 5000
	cargo clippy -p my-stackchan-host -p stackchan-clock -p stackchan-image-demo --all-targets --locked --offline $(WINDOWS_CARGO_FLAGS) -- -D warnings
	cargo build -p my-stackchan-host -p stackchan-clock -p stackchan-image-demo --locked --offline --release $(WINDOWS_CARGO_FLAGS)
	# ルートから実行し、firmware/.cargo の Xtensa/build-std 設定を適用しない。
	cargo clippy --manifest-path $(FIRMWARE_DIR)/Cargo.toml --lib --locked --offline -- -D warnings
	cargo test --manifest-path $(FIRMWARE_DIR)/Cargo.toml --lib --locked --offline
	cargo clippy --manifest-path $(FIRMWARE_DIR)/Cargo.toml --example simulate --locked --offline -- -D warnings
	cargo test --manifest-path $(FIRMWARE_DIR)/Cargo.toml --example simulate --locked --offline
	cd $(FIRMWARE_DIR) && cargo fmt --all --check
	cd $(FIRMWARE_DIR) && cargo clippy --locked --offline -- -D warnings
	cd $(FIRMWARE_DIR) && cargo build --locked --offline --release
	mkdir -p .work
	espflash save-image --skip-update-check --chip esp32s3 --flash-size 16mb \
		$(FIRMWARE_DIR)/target/xtensa-esp32s3-none-elf/release/my-stackchan-firmware .work/firmware.bin

.PHONY: fmt
fmt: ## Nix、シェルスクリプト、Rust を整形する
	$(NIX) fmt
	shfmt --write --indent 2 --case-indent scripts/*.sh
	cargo fmt --all
	cd $(FIRMWARE_DIR) && cargo fmt --all

.PHONY: lint
lint: ## 静的解析のみを実行する (整形は行わない)
	statix check .
	deadnix --fail .
	scripts/check-lock.sh
	scripts/check-pins.sh
	shellcheck scripts/*.sh
	shellcheck --shell=bash .envrc

# 開発シェルの CARGO_HOME は crates.io を store 内の vendor で置き換えており、cargo-deny が
# yank の確認に使う index を取得できない。置き換えの無い別の CARGO_HOME で実行する。
AUDIT_CARGO_HOME := $(CURDIR)/.work/cargo-online

.PHONY: audit
audit: ## 依存の advisory と license を検査する (ネットワークを使用)
	mkdir -p $(AUDIT_CARGO_HOME)
	CARGO_HOME=$(AUDIT_CARGO_HOME) cargo deny check
	cd $(FIRMWARE_DIR) && CARGO_HOME=$(AUDIT_CARGO_HOME) cargo deny check

# --- ビルド -----------------------------------------------------------------

.PHONY: build
build: build-host build-firmware ## host と firmware を build する

.PHONY: build-host
build-host: ## host CLI と protocol を build する
	cargo build --workspace --locked --release

.PHONY: build-host-windows
build-host-windows: ## host CLI と同梱の plugin を Windows 向けに cross build する (target/x86_64-pc-windows-gnu-build-std/)
	cargo build -p my-stackchan-host -p stackchan-clock -p stackchan-image-demo --locked --offline --release $(WINDOWS_CARGO_FLAGS)

.PHONY: build-firmware
build-firmware: ## firmware を build する
	cd $(FIRMWARE_DIR) && cargo build --locked --release

.PHONY: simulate
simulate: ## 実機と同じ描画処理で表示の各状態を .work/simulation/ に生成する
	cargo run --manifest-path $(FIRMWARE_DIR)/Cargo.toml --example simulate --locked --offline

.PHONY: flash
flash: build-firmware ## firmware を書き込む (SERIAL_DEVICE、既定 /dev/ttyACM0)
	cd $(FIRMWARE_DIR) && espflash flash --skip-update-check --port $${SERIAL_DEVICE:-/dev/ttyACM0} \
		target/xtensa-esp32s3-none-elf/release/my-stackchan-firmware

.PHONY: clean
clean: ## cargo の成果物を削除する
	cargo clean
	cd $(FIRMWARE_DIR) && cargo clean

.PHONY: clean-all
clean-all: clean ## 成果物と作業ディレクトリ (.work) をすべて削除する
	rm -rf .work

# --- コンテナ側の環境 (Linux ホストから) ------------------------------------

.PHONY: docker-build
docker-build: ## 同一の環境を持つコンテナイメージを構築する
	$(CONTAINER_ENGINE) build -t $(IMAGE) .

.PHONY: docker-shell
docker-shell: docker-build ## コンテナ内の開発シェルに入る
	$(CONTAINER_ENGINE) run --rm -it -v "$(CURDIR):/workspace" $(IMAGE)

.PHONY: docker-check
docker-check: docker-build ## コンテナ内で検査を実行する (--network none)
	$(CONTAINER_ENGINE) run --rm --network none -v "$(CURDIR):/workspace" $(IMAGE) make check
