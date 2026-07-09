.PHONY: setup build test clippy fmt fmt-check check clean

CARGO_BIN := $(HOME)/.cargo/bin
RUSTUP := $(CARGO_BIN)/rustup
CARGO := $(CARGO_BIN)/cargo

# Default target
build: setup
	$(CARGO) build

# Idempotent machine setup — safe to run repeatedly
setup:
	@if [ ! -f "$(RUSTUP)" ]; then \
		echo "Installing rustup..."; \
		curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y > /dev/null 2>&1; \
		echo ""; \
		echo "✓ Rust toolchain installed."; \
		echo "  Run: source ~/.cargo/env && make"; \
		exit 0; \
	fi
	@$(RUSTUP) show active-toolchain > /dev/null 2>&1 || $(RUSTUP) toolchain install stable --quiet
	@$(RUSTUP) component list --installed | grep -q clippy || $(RUSTUP) component add clippy --quiet
	@$(RUSTUP) component list --installed | grep -q rustfmt || $(RUSTUP) component add rustfmt --quiet
	@echo "Toolchain ready: $$($(CARGO_BIN)/rustc --version)"

test:
	$(CARGO) test

clippy:
	$(CARGO) clippy -- -D warnings

fmt:
	$(CARGO) fmt

fmt-check:
	$(CARGO) fmt --check

# Full pre-commit gate (mirrors CI)
check: fmt-check clippy test

clean:
	$(CARGO) clean
