#!/usr/bin/env bash
# ochna CLI performance benchmark script

set -euo pipefail

# Ensure hyperfine is installed
if ! command -v hyperfine &> /dev/null; then
    echo "Error: hyperfine is not installed. Install it via 'brew install hyperfine' or use nix dev shell."
    exit 1
fi

# Ensure ochna is built
echo "Building ochna in release mode..."
cargo build --release

OCHNA="./target/release/ochna"

# Select target project to benchmark
TARGET_DIR="clones/tokio"
if [ ! -d "$TARGET_DIR" ]; then
    echo "Warning: $TARGET_DIR not found. Benchmarking in the current repository instead."
    TARGET_DIR="."
fi

echo "=================================================="
echo "Running benchmarks in: $TARGET_DIR"
echo "=================================================="
echo

# 1. Cold Start Indexing
echo "--- 1. Cold Start Indexing (Full parse & write) ---"
hyperfine --prepare "rm -rf $TARGET_DIR/.codegraph" --runs 3 \
    "$OCHNA init" --directory "$TARGET_DIR"
echo

# 2. Warm Start Indexing (Incremental update checks)
echo "--- 2. Warm Start Indexing (Incremental / Hash check) ---"
hyperfine --runs 5 \
    "$OCHNA init" --directory "$TARGET_DIR"
echo

# 3. Query Benchmark: Symbol Search
echo "--- 3. Query Latency: Symbol Search ---"
hyperfine --runs 10 \
    "$OCHNA search Builder" --directory "$TARGET_DIR"
echo

# 4. Query Benchmark: Callers Trace
echo "--- 4. Query Latency: Callers Trace ---"
hyperfine --runs 10 \
    "$OCHNA callers new_multi_thread" --directory "$TARGET_DIR"
echo

echo "Benchmarks completed."
