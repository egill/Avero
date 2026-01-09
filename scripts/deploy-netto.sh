#!/bin/bash
set -e

# Use rustup's cargo (not Homebrew) and zig 0.14 for cross-compilation
export PATH="$HOME/.cargo/bin:/opt/homebrew/opt/zig@0.14/bin:$PATH"

HOST="avero@100.80.187.3"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGET="aarch64-unknown-linux-gnu"
BINARY="$SCRIPT_DIR/target/$TARGET/release/gateway-poc"

cd "$SCRIPT_DIR"

echo "🚀 Deploying gateway-poc to Netto"

echo "🧪 Running tests..."
cargo test

echo "🔨 Building with zig for $TARGET..."
cargo zigbuild --release --target "$TARGET"

if [ ! -f "$BINARY" ]; then
    echo "❌ Binary not found at $BINARY"
    exit 1
fi

echo "📦 Copying binary to server..."
scp "$BINARY" "$HOST:/tmp/gateway-poc"

echo "🔄 Deploying..."
ssh "$HOST" "sudo systemctl stop gateway-poc && sleep 2 && sudo cp /tmp/gateway-poc /opt/avero/gateway-poc-bin && sudo systemctl start gateway-poc"

echo "✅ Checking status..."
ssh "$HOST" "sudo systemctl status gateway-poc --no-pager"

echo "🎉 Deployed!"
