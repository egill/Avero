#!/bin/bash
# Build optimized binaries for gate latency benchmark
#
# Usage: ./build.sh [target]
#   target: native (default), pi (build on remote Pi via SSH)
#
# For Pi builds, set PI_HOST environment variable:
#   PI_HOST=avero@100.80.187.3 ./build.sh pi

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
TARGET="${1:-native}"
PI_HOST="${PI_HOST:-avero@100.80.187.3}"

echo "Building gate-bench binaries..."
echo "Target: $TARGET"
echo

if [ "$TARGET" = "pi" ]; then
    # Cross-compile for Pi
    RUST_TARGET="aarch64-unknown-linux-gnu"
    OUTPUT_DIR="$SCRIPT_DIR/bin/aarch64-linux"
    mkdir -p "$OUTPUT_DIR"

    # Build Rust with zigbuild
    echo "=== Building Rust benchmark (cross-compile) ==="
    cd "$SCRIPT_DIR/rust"
    cargo zigbuild --release --target "$RUST_TARGET"
    cp "target/$RUST_TARGET/release/gate-bench" "$OUTPUT_DIR/gate-bench-rust"
    echo "Rust binary: $OUTPUT_DIR/gate-bench-rust"
    ls -lh "$OUTPUT_DIR/gate-bench-rust"
    echo

    # Build Go with cross-compile
    echo "=== Building Go benchmark (cross-compile) ==="
    cd "$SCRIPT_DIR/go"
    GOOS=linux GOARCH=arm64 CGO_ENABLED=0 go build -ldflags="-s -w" -o "$OUTPUT_DIR/gate-bench-go" .
    echo "Go binary: $OUTPUT_DIR/gate-bench-go"
    ls -lh "$OUTPUT_DIR/gate-bench-go"
    echo

    echo "=== Build complete ==="
    echo "Binaries in: $OUTPUT_DIR"
    ls -lh "$OUTPUT_DIR/"
else
    # Native build
    OUTPUT_DIR="$SCRIPT_DIR/bin/native"
    mkdir -p "$OUTPUT_DIR"

    # Build Rust
    echo "=== Building Rust benchmark ==="
    cd "$SCRIPT_DIR/rust"
    cargo build --release
    cp target/release/gate-bench "$OUTPUT_DIR/gate-bench-rust"
    echo "Rust binary: $OUTPUT_DIR/gate-bench-rust"
    ls -lh "$OUTPUT_DIR/gate-bench-rust"
    echo

    # Build Go
    echo "=== Building Go benchmark ==="
    cd "$SCRIPT_DIR/go"
    go build -ldflags="-s -w" -o "$OUTPUT_DIR/gate-bench-go" .
    echo "Go binary: $OUTPUT_DIR/gate-bench-go"
    ls -lh "$OUTPUT_DIR/gate-bench-go"
    echo

    echo "=== Build complete ==="
    echo "Binaries in: $OUTPUT_DIR"
    ls -lh "$OUTPUT_DIR/"
fi
