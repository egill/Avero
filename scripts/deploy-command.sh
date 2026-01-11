#!/bin/bash
set -e

# Configuration
HOST="root@e18n.net"
REMOTE_DIR="/opt/avero/command"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
COMMAND_DIR="$SCRIPT_DIR/command"

cd "$COMMAND_DIR"

echo "Deploying Avero Command to e18n.net..."

echo "Syncing code..."
rsync -avz --delete \
    --exclude '_build' \
    --exclude 'deps' \
    --exclude 'node_modules' \
    --exclude '.elixir_ls' \
    --exclude '*.log' \
    --exclude '.env' \
    "$COMMAND_DIR/" "$HOST:$REMOTE_DIR/"

echo "Rebuilding and restarting container..."
ssh "$HOST" "\
    cd $REMOTE_DIR && \
    docker build -t avero-command:latest -f Dockerfile.dev . && \
    docker stop avero-command || true && \
    docker rm avero-command || true && \
    docker run -d \
        --name avero-command \
        --restart unless-stopped \
        --network avero \
        -p 127.0.0.1:4000:4000 \
        --env-file .env \
        avero-command:latest"

echo "Waiting for startup..."
sleep 5

echo "Verifying..."
ssh "$HOST" "docker ps | grep avero-command"
ssh "$HOST" "docker logs avero-command --tail 20"

echo "Done: https://command.e18n.net"
