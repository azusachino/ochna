# ochna Makefile
# Provides industry-standard targets for building, testing, checking quality, and installing ochna.

.PHONY: all build test fmt fmt-fix lint check validate setup install report clean

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

# Alias for check; run before opening a PR.
validate: check

setup:
	@echo "Initializing Git submodules..."
	git submodule update --init --recursive
	@echo "Initializing python virtual environment via uv..."
	uv venv --python 3.14
	@echo "Building ochna binary..."
	cargo build --release
	@echo "Done. Run 'make report' to index the test giants and emit BENCHMARK.md."

install:
	cargo install --path . --root $(HOME)/.cargo

# Index every checked-out test giant and write BENCHMARK.md. Reproducible
# quality gate: counts are stable per pinned submodule commit, so a parser
# regression shows up as a count delta. Use REINDEX=1 to force a clean re-index.
report: build
	./scripts/report.sh

clean:
	cargo clean
