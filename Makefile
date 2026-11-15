-include .env

ifeq ($(OS),Windows_NT)
	GIT_EXE := $(shell where git.exe 2>NUL)
	GIT_EXE_FIRST := $(firstword $(GIT_EXE))
	GIT_EXE_NORM := $(subst \,/,$(GIT_EXE_FIRST))
	DETECTED_BASH := $(subst /cmd/git.exe,/bin/bash.exe,$(GIT_EXE_NORM))

	ifeq ($(wildcard $(DETECTED_BASH)),)
		DETECTED_BASH := C:/Program Files/Git/bin/bash.exe
	endif

	SHELL := $(DETECTED_BASH)
	.SHELLFLAGS := -euo pipefail -c
else
	SHELL := /bin/bash
	.SHELLFLAGS := -euo pipefail -c
endif

BIN_NAME := meerai
BIN_PKG  := meerai-cli
DIST_DIR := dist

UNAME_S := $(shell uname -s 2>/dev/null || echo Windows)
UNAME_M := $(shell uname -m 2>/dev/null || echo x86_64)

ifeq ($(OS),Windows_NT)
	HOST_TARGET ?= x86_64-pc-windows-msvc
	BIN_EXT     := .exe
else ifeq ($(UNAME_S),Darwin)
	ifeq ($(UNAME_M),arm64)
		HOST_TARGET ?= aarch64-apple-darwin
	else
		HOST_TARGET ?= x86_64-apple-darwin
	endif
	BIN_EXT :=
else
	ifeq ($(UNAME_M),aarch64)
		HOST_TARGET ?= aarch64-unknown-linux-gnu
	else
		HOST_TARGET ?= x86_64-unknown-linux-gnu
	endif
	BIN_EXT :=
endif

BUILD_TARGET := $(if $(target),$(target),$(HOST_TARGET))

.DEFAULT_GOAL := help
.SILENT:

.PHONY: build build-debug check ci clean clean-dist dev fmt fmt-check format help \
	install lint package run test verify watch

# =============================================================================
# Help & Information
# =============================================================================

##@ Help & Information

help: ## Show this help message
	@awk 'BEGIN {FS = ":.*##"; printf "\n\033[1mUsage:\033[0m make \033[36m<target>\033[0m\n"} \
		/^[a-zA-Z0-9_-]+:.*?##/ { printf "  \033[36m%-20s\033[0m %s\n", $$1, $$2 } \
		/^##@/ { printf "\n\033[1m%s\033[0m\n", substr($$0, 5) } ' $(MAKEFILE_LIST)

check: ## Check required toolchain commands
	@command -v rustup >/dev/null 2>&1 || { echo "rustup is not installed"; exit 1; }
	@command -v rustc >/dev/null 2>&1 || { echo "rustc is not installed"; exit 1; }
	@command -v cargo >/dev/null 2>&1 || { echo "cargo is not installed"; exit 1; }
	@echo "Toolchain OK: $$(rustc --version)"

# =============================================================================
# Development
# =============================================================================

##@ Development

dev: ## Run meerai locally in development mode (pass flags via ARGS="...")
	cargo run -p $(BIN_PKG) -- $(ARGS)

run: dev ## Alias for dev

watch: ## Watch source changes and rerun meerai (requires cargo-watch)
	@command -v cargo-watch >/dev/null 2>&1 || { echo "Install cargo-watch first: cargo install cargo-watch"; exit 1; }
	cargo watch -x "run -p $(BIN_PKG) -- $(ARGS)"

fmt: ## Format all workspace Rust crates
	cargo fmt --all

format: fmt ## Alias for fmt

fmt-check: ## Check workspace formatting without modifying files
	cargo fmt --all -- --check

lint: ## Run clippy across all workspace targets
	cargo clippy --workspace --all-targets

test: ## Run all workspace tests
	cargo test --workspace

ci: fmt-check lint test ## Run local CI verification (fmt-check -> lint -> test)

verify: ci ## Alias for ci, matching the CI "verify" job

# =============================================================================
# Build & Packaging
# =============================================================================

##@ Build & Packaging

build: ## Build release binary (optional target=<triple>, e.g. target=x86_64-unknown-linux-gnu)
	cargo build --release --locked -p $(BIN_PKG) $(if $(target),--target $(target),) $(ARGS)
	@BIN_PATH="target/$(if $(target),$(target)/,)release/$(BIN_NAME)$(BIN_EXT)"; \
	echo "Built binary: $$BIN_PATH"

build-debug: ## Build debug binary
	cargo build -p $(BIN_PKG) $(ARGS)

install: ## Install meerai locally into Cargo bin directory
	cargo install --path $(BIN_PKG) --locked

package: ## Build and package release archive + SHA256 into dist/ (optional target=<triple>)
	@mkdir -p $(DIST_DIR)
	cargo build --release --locked -p $(BIN_PKG) --target $(BUILD_TARGET) $(ARGS)
	@ARCHIVE_BASE="$(BIN_NAME)-$(BUILD_TARGET)"; \
	if [[ "$(BUILD_TARGET)" == *windows* ]]; then \
		ARCHIVE="$$ARCHIVE_BASE.zip"; \
		powershell -NoProfile -Command "Compress-Archive -Force -Path 'target/$(BUILD_TARGET)/release/$(BIN_NAME).exe' -DestinationPath '$(DIST_DIR)/$$ARCHIVE'"; \
	else \
		ARCHIVE="$$ARCHIVE_BASE.tar.gz"; \
		tar -czf "$(DIST_DIR)/$$ARCHIVE" -C "target/$(BUILD_TARGET)/release" "$(BIN_NAME)"; \
	fi; \
	(cd $(DIST_DIR) && \
		if command -v sha256sum >/dev/null 2>&1; then \
			sha256sum "$$ARCHIVE" > "$$ARCHIVE.sha256"; \
		elif command -v shasum >/dev/null 2>&1; then \
			shasum -a 256 "$$ARCHIVE" > "$$ARCHIVE.sha256"; \
		else \
			openssl dgst -sha256 -r "$$ARCHIVE" | awk '{print $$1 "  " $$2}' > "$$ARCHIVE.sha256"; \
		fi); \
	echo "Packaged: $(DIST_DIR)/$$ARCHIVE"

# =============================================================================
# Cleanup
# =============================================================================

##@ Cleanup

clean: clean-dist ## Clean Cargo build and release artifacts
	cargo clean

clean-dist: ## Remove packaged release artifacts in dist/
	rm -rf $(DIST_DIR)
