#!/bin/bash
set -e

HOST="root@e18n.net"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
COMMAND_DIR="$SCRIPT_DIR/command"
REMOTE_DIR="/opt/avero/command"

cd "$COMMAND_DIR"

echo "Deploying Avero Command to e18n.net"

echo "Syncing code to server..."
rsync -avz --delete \
    --exclude '_build' \
    --exclude 'deps' \
    --exclude 'node_modules' \
    --exclude '.elixir_ls' \
    --exclude '*.log' \
    "$COMMAND_DIR/" "$HOST:$REMOTE_DIR/"

echo "Rebuilding and restarting container..."
ssh "$HOST" "cd $REMOTE_DIR && docker build -t avero-command:latest -f Dockerfile.dev . && docker stop avero-command || true && docker rm avero-command || true && docker run -d --name avero-command --restart unless-stopped -p 127.0.0.1:4000:4000 --env-file .env avero-command:latest"

echo "Waiting for startup..."
sleep 5

echo "Checking status..."
ssh "$HOST" "docker ps | grep avero-command"
ssh "$HOST" "docker logs avero-command --tail 20"

echo "Deployed! Access at https://command.e18n.net"
