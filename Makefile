# ochna Makefile
# Provides targets for building, testing, installing, and cleaning the project.

.PHONY: all build test install clean

all: build

build:
	cargo build --release

test:
	cargo test

install:
	cargo install --path . --root $(HOME)/.cargo

clean:
	cargo clean
