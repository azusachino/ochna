# ochna Makefile
# Provides industry-standard targets for building, testing, checking quality, and installing ochna.

.PHONY: all build test fmt fmt-fix lint check install clean

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

install:
	cargo install --path . --root $(HOME)/.cargo

clean:
	cargo clean
