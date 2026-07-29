# ochna Makefile
# Provides industry-standard targets for building, testing, checking quality, and installing ochna.

.DEFAULT_GOAL := help

.PHONY: help all build test fmt fmt-fix lint check validate verify-clis verify_clis setup install report case-sim clean

# `make` runs each recipe line in its own fresh, non-interactive shell, which
# never sources shell rc files -- so a `mise activate` in .zshrc/.bashrc does
# not carry into Make at all, regardless of the invoking shell. `mise x --`
# resolves the .mise.toml-pinned tool directly per invocation instead of
# depending on shell activation having happened.
MISE := mise x --

help:
	@echo "Usage: make <target>"
	@echo ""
	@echo "Targets:"
	@echo "  build        Build the release binary"
	@echo "  test         Run the test suite"
	@echo "  check        Check formatting and run Clippy"
	@echo "  validate     Run checks and CLI smoke tests"
	@echo "  setup        Shallow-clone submodules and build ochna"
	@echo "  install      Install ochna to ~/.cargo/bin"
	@echo "  report       Index test giants and write BENCHMARK.md"
	@echo "  case-sim     Check determined vs actual state against known real-world cases"
	@echo "  clean        Remove Cargo build artifacts"

all: build

build:
	$(MISE) cargo build --release

test:
	$(MISE) cargo test

fmt:
	$(MISE) cargo fmt --all -- --check

fmt-fix:
	$(MISE) cargo fmt --all

lint:
	$(MISE) cargo clippy --all-targets -- -D warnings

check: fmt lint

# Run before opening a PR: static checks plus CLI smoke tests.
validate: check verify-clis

verify-clis: build
	UV_CACHE_DIR=.uv-cache $(MISE) uv run python scripts/verify_clis.py

verify_clis: verify-clis

setup:
	@echo "Initializing Git submodules..."
	git submodule update --init --recursive --depth 1
	@echo "Installing mise-managed toolchain (rust, uv)..."
	mise install
	@echo "Initializing python virtual environment via uv..."
	$(MISE) uv venv --python 3.14
	@echo "Building ochna binary..."
	$(MISE) cargo build --release
	@echo "Done. Run 'make report' to index the test giants and emit BENCHMARK.md."

install:
	$(MISE) cargo install --path . --root $(HOME)/.cargo

# Index every checked-out test giant and write BENCHMARK.md. Reproducible
# quality gate: counts are stable per pinned submodule commit, so a parser
# regression shows up as a count delta. Use REINDEX=1 to force a clean re-index.
report: build
	$(MISE) uv run python scripts/benchmark_report.py

# Determined-state (ochna's output) vs actual-state (documented ground truth
# from real historical PRs) checks against the test giants. Catches parser/
# resolution regressions that synthetic fixtures wouldn't.
case-sim: build
	$(MISE) uv run python scripts/case_simulation.py

clean:
	$(MISE) cargo clean
