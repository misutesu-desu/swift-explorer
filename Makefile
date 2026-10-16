# Swift Explorer: run, test and build helpers.
#
#   make run           run the app (release); ARGS="/some/dir" opens a folder
#   make build         build portable packages for every OS and architecture into output/
#   make help          list every target
#
# Cross builds use Zig as the C toolchain (cargo-zigbuild), so no Docker, root or
# MinGW is needed: `make tools` installs everything into your user account.

SHELL := /bin/bash

APP     := swift-explorer
VERSION := $(shell sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
OUT     := output
CACHE   := .cache
VENV    := $(CACHE)/venv
STAGE   := $(CACHE)/stage
APP_ID  := io.github.misutesu_desu.SwiftExplorer

# Zig is installed from PyPI into a private venv; cargo-zigbuild finds it through PATH.
export PATH := $(CURDIR)/$(VENV)/bin:$(PATH)

# Oldest glibc the Linux builds run on
GLIBC := 2.17

LINUX_TARGETS   := x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu
WINDOWS_TARGETS := x86_64-pc-windows-gnu i686-pc-windows-gnu aarch64-pc-windows-gnullvm
ALL_TARGETS     := $(LINUX_TARGETS) $(WINDOWS_TARGETS)

# x86_64-unknown-linux-gnu -> linux-x86_64, i686-pc-windows-gnu -> windows-i686
label = $(if $(findstring linux,$(1)),linux,windows)-$(firstword $(subst -, ,$(1)))

LINUX_PACKAGES   := $(foreach t,$(LINUX_TARGETS),$(OUT)/$(APP)-$(VERSION)-$(call label,$(t))-portable.tar.gz)
WINDOWS_PACKAGES := $(foreach t,$(WINDOWS_TARGETS),$(OUT)/$(APP)-$(VERSION)-$(call label,$(t))-portable.zip)

SOURCES := $(shell find src assets packaging -type f 2>/dev/null) Cargo.toml Cargo.lock build.rs

.DEFAULT_GOAL := help
.PHONY: help run dev build build-local portable linux windows test check fmt lint tools clean distclean checksums

help: ## Show this help
	@awk 'BEGIN {FS = ":.*## "} /^[a-zA-Z_-]+:.*## / {printf "  \033[1m%-12s\033[0m %s\n", $$1, $$2}' $(MAKEFILE_LIST)

# ---------------------------------------------------------------- run and test

run: ## Run the app (release build). Example: make run ARGS=/tmp
	cargo run --release -- $(ARGS)

dev: ## Run the app (debug build, faster to compile)
	cargo run -- $(ARGS)

test: ## Run the unit tests
	cargo test

check: ## Type-check without building
	cargo check

fmt: ## Format the code
	cargo fmt --all

lint: ## Run clippy
	cargo clippy --all-targets

# ---------------------------------------------------------------- build

build: portable ## Build portable packages for all OSes and architectures into output/

portable: tools $(LINUX_PACKAGES) $(WINDOWS_PACKAGES) checksums ## Same as build

linux: tools $(LINUX_PACKAGES) ## Only the Linux packages (x86_64, aarch64)

windows: tools $(WINDOWS_PACKAGES) ## Only the Windows packages (x86_64, i686, aarch64)

build-local: ## Quick release build for this machine only, copied to output/local/
	cargo build --release
	@mkdir -p $(OUT)/local
	cp target/release/$(APP) $(OUT)/local/

# Linux: tar.gz with the binary plus the desktop files, like the release archives
define linux_package
$(OUT)/$(APP)-$(VERSION)-$(call label,$(1))-portable.tar.gz: $(SOURCES) | tools
	rustup target add $(1)
	cargo zigbuild --release --target $(1).$(GLIBC)
	rm -rf $(STAGE)/$(1) && mkdir -p $(STAGE)/$(1)/$(APP) $(OUT)
	cp target/$(1)/release/$(APP) LICENSE $(STAGE)/$(1)/$(APP)/
	cp packaging/linux/$(APP_ID).desktop packaging/linux/$(APP_ID).metainfo.xml $(STAGE)/$(1)/$(APP)/
	cp assets/icon.png $(STAGE)/$(1)/$(APP)/$(APP_ID).png
	chmod 755 $(STAGE)/$(1)/$(APP)/$(APP)
	tar -C $(STAGE)/$(1) -czf $$@ $(APP)
endef

# Windows: zip with a single self-contained exe. WINDRES points the icon resource at Zig's
# resource compiler (see packaging/build/windres-zig); AR=ar because no MinGW is installed.
define windows_package
$(OUT)/$(APP)-$(VERSION)-$(call label,$(1))-portable.zip: $(SOURCES) | tools
	rustup target add $(1)
	WINDRES=$(CURDIR)/packaging/build/windres-zig AR=ar cargo zigbuild --release --target $(1)
	rm -rf $(STAGE)/$(1) && mkdir -p $(STAGE)/$(1)/SwiftExplorer $(OUT)
	cp target/$(1)/release/$(APP).exe $(STAGE)/$(1)/SwiftExplorer/SwiftExplorer.exe
	cp LICENSE $(STAGE)/$(1)/SwiftExplorer/
	rm -f $$@ && cd $(STAGE)/$(1) && zip -qr $(CURDIR)/$$@ SwiftExplorer
endef

$(foreach t,$(LINUX_TARGETS),$(eval $(call linux_package,$(t))))
$(foreach t,$(WINDOWS_TARGETS),$(eval $(call windows_package,$(t))))

checksums: ## Write output/SHA256SUMS for the packages
	@cd $(OUT) && sha256sum *.tar.gz *.zip > SHA256SUMS && echo "wrote $(OUT)/SHA256SUMS"

# ---------------------------------------------------------------- tooling

tools: $(VENV)/.zig-installed $(HOME)/.cargo/bin/cargo-zigbuild ## Install Zig and cargo-zigbuild (user-level, no root)

$(VENV)/.zig-installed:
	python3 -m venv $(VENV)
	$(VENV)/bin/pip install --quiet ziglang
	touch $@

$(HOME)/.cargo/bin/cargo-zigbuild:
	cargo install --locked cargo-zigbuild

clean: ## Remove build output and packages (keeps the tool cache)
	cargo clean
	rm -rf $(OUT) $(STAGE)

distclean: clean ## Also remove the cached tools in .cache/
	rm -rf $(CACHE)
