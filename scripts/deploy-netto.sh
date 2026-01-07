#!/bin/bash
set -e

HOST="avero@100.80.187.3"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

echo "🚀 Deploying gateway-poc to Netto"

echo "📦 Syncing source..."
rsync -avz --exclude target --exclude .git "$SCRIPT_DIR/" "$HOST:~/gateway-poc-new/"

echo "🔨 Building on server..."
ssh "$HOST" "source ~/.cargo/env && cd ~/gateway-poc-new && cargo build --release"

echo "🔄 Deploying..."
ssh "$HOST" "sudo systemctl stop gateway-poc && sleep 2 && cp ~/gateway-poc-new/target/release/gateway-poc /opt/avero/gateway-poc/target/release/ && sudo systemctl start gateway-poc"

echo "✅ Checking status..."
ssh "$HOST" "sudo systemctl status gateway-poc --no-pager"

echo "🎉 Deployed!"
