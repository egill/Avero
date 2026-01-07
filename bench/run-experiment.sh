#!/bin/bash
# Run gate latency experiment comparing Rust vs Go
#
# Usage: ./run-experiment.sh [options]
#   --trials N      Number of trials per language (default: 30)
#   --delay N       Seconds between trials (default: 5)
#   --gate ADDR     Gate TCP address (default: 192.168.0.245:8000)
#   --rs485 DEV     RS485 device (default: /dev/ttyUSB0)
#   --output DIR    Output directory (default: ./results)

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

# Default parameters
TRIALS=30
DELAY=5
GATE_ADDR="192.168.0.245:8000"
RS485_DEV="/dev/ttyUSB0"
OUTPUT_DIR="$SCRIPT_DIR/results"

# Parse arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        --trials) TRIALS="$2"; shift 2 ;;
        --delay) DELAY="$2"; shift 2 ;;
        --gate) GATE_ADDR="$2"; shift 2 ;;
        --rs485) RS485_DEV="$2"; shift 2 ;;
        --output) OUTPUT_DIR="$2"; shift 2 ;;
        *) echo "Unknown option: $1"; exit 1 ;;
    esac
done

# Detect architecture
ARCH=$(uname -m)
if [ "$ARCH" = "aarch64" ] || [ "$ARCH" = "arm64" ]; then
    BIN_DIR="$SCRIPT_DIR/bin/aarch64-linux"
else
    BIN_DIR="$SCRIPT_DIR/bin/native"
fi

RUST_BIN="$BIN_DIR/gate-bench-rust"
GO_BIN="$BIN_DIR/gate-bench-go"

# Check binaries exist
if [ ! -f "$RUST_BIN" ] || [ ! -f "$GO_BIN" ]; then
    echo "Error: Binaries not found in $BIN_DIR"
    echo "Run ./build.sh first"
    exit 1
fi

# Create output directory
TIMESTAMP=$(date +%Y%m%d_%H%M%S)
RESULT_DIR="$OUTPUT_DIR/$TIMESTAMP"
mkdir -p "$RESULT_DIR"

echo "Gate Latency Experiment"
echo "======================="
echo "Timestamp: $TIMESTAMP"
echo "Trials: $TRIALS"
echo "Delay: ${DELAY}s between trials"
echo "Gate: $GATE_ADDR"
echo "RS485: $RS485_DEV"
echo "Output: $RESULT_DIR"
echo

# System info
echo "=== System Info ===" | tee "$RESULT_DIR/system-info.txt"
uname -a | tee -a "$RESULT_DIR/system-info.txt"
echo "Rust version:" | tee -a "$RESULT_DIR/system-info.txt"
rustc --version 2>/dev/null | tee -a "$RESULT_DIR/system-info.txt" || echo "N/A"
echo "Go version:" | tee -a "$RESULT_DIR/system-info.txt"
go version 2>/dev/null | tee -a "$RESULT_DIR/system-info.txt" || echo "N/A"
echo | tee -a "$RESULT_DIR/system-info.txt"

# Run Rust benchmark
echo "=== Running Rust benchmark ===" | tee "$RESULT_DIR/rust-output.txt"
"$RUST_BIN" \
    --gate-addr "$GATE_ADDR" \
    --rs485-device "$RS485_DEV" \
    --trials "$TRIALS" \
    --delay "$DELAY" \
    2>&1 | tee -a "$RESULT_DIR/rust-output.txt"

echo
echo "Waiting 30 seconds before Go benchmark..."
sleep 30

# Run Go benchmark
echo "=== Running Go benchmark ===" | tee "$RESULT_DIR/go-output.txt"
"$GO_BIN" \
    -gate-addr "$GATE_ADDR" \
    -rs485-device "$RS485_DEV" \
    -trials "$TRIALS" \
    -delay "$DELAY" \
    2>&1 | tee -a "$RESULT_DIR/go-output.txt"

echo
echo "=== Experiment Complete ==="
echo "Results saved to: $RESULT_DIR"
echo
echo "Summary:"
echo "--------"
echo "Rust:"
grep -A6 "Results:" "$RESULT_DIR/rust-output.txt" | tail -6
echo
echo "Go:"
grep -A6 "Results:" "$RESULT_DIR/go-output.txt" | tail -6
