# ochna Makefile
# Provides industry-standard targets for building, testing, checking quality, and installing ochna.

.PHONY: all build test fmt fmt-fix lint check setup install clean

all: build

build:
	cargo build --release

test:
	cargo test

fmt:
	cargo fmt --all -- --check

fmt-fix:
	cargo fmt --all

lint:
	cargo clippy --all-targets -- -D warnings

check: fmt lint

setup:
	@echo "Initializing Git submodules..."
	git submodule update --init --recursive
	@echo "Initializing python virtual environment via uv..."
	uv venv --python 3.14
	@echo "Building ochna binary..."
	cargo build --release
	@echo "Indexing development submodules..."
	cd clones/tokio && ../../target/release/ochna init || true
	cd clones/netty && ../../target/release/ochna init || true
	cd clones/kubernetes && ../../target/release/ochna init || true
	cd clones/linux && ../../target/release/ochna init || true
	cd clones/zig && ../../target/release/ochna init || true

install:
	cargo install --path . --root $(HOME)/.cargo

clean:
	cargo clean
