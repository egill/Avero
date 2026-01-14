#!/bin/bash
set -e

# Configuration
HOST="avero@100.80.187.3"
TARGET="aarch64-unknown-linux-gnu"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BINARY="$SCRIPT_DIR/target/$TARGET/release/gateway"

# Use rustup's cargo and zig 0.14 for cross-compilation
export PATH="$HOME/.cargo/bin:/opt/homebrew/opt/zig@0.14/bin:$PATH"

cd "$SCRIPT_DIR"

echo "Deploying gateway to Netto..."

echo "Running tests..."
cargo test

echo "Building for $TARGET..."
cargo zigbuild --release --target "$TARGET"

if [ ! -f "$BINARY" ]; then
    echo "Error: Binary not found at $BINARY"
    exit 1
fi

echo "Copying binary to server..."
scp "$BINARY" "$HOST:/tmp/gateway"

echo "Syncing config..."
scp "$SCRIPT_DIR/config/netto.toml" "$HOST:/tmp/netto.toml"

echo "Restarting service..."
ssh "$HOST" "\
    sudo systemctl stop gateway && \
    sleep 2 && \
    sudo cp /tmp/gateway /opt/avero/gateway-bin && \
    sudo mkdir -p /opt/avero/config && \
    sudo cp /tmp/netto.toml /opt/avero/config/netto.toml && \
    sudo chown avero:avero /opt/avero/config/netto.toml && \
    sudo systemctl start gateway"

echo "Verifying status..."
ssh "$HOST" "sudo systemctl status gateway --no-pager"

echo "Done."
