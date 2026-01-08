---
name: deploy-command
description: Deploy Avero Command (Phoenix app) to e18n.net. Use when deploying the command center dashboard.
---

# /deploy-command

Deploy Avero Command Phoenix app to e18n.net production server.

## Steps

1. Rsync code to server (excluding build artifacts)
2. Docker build new image
3. Stop and remove old container
4. Start new container with env file
5. Verify container is running

## Command

```bash
./scripts/deploy-command.sh
```

## Manual Deploy

```bash
HOST="root@e18n.net"
REMOTE_DIR="/opt/avero/command"

# Sync code
rsync -avz --delete \
    --exclude '_build' \
    --exclude 'deps' \
    --exclude 'node_modules' \
    --exclude '.elixir_ls' \
    command/ "$HOST:$REMOTE_DIR/"

# Build and restart
ssh "$HOST" "cd $REMOTE_DIR && docker build -t avero-command:latest -f Dockerfile.dev . && docker stop avero-command || true && docker rm avero-command || true && docker run -d --name avero-command --restart unless-stopped -p 127.0.0.1:4000:4000 --env-file .env avero-command:latest"
```

## Verify

After deploy, check logs:
```bash
ssh root@e18n.net "docker logs avero-command -f"
```

Check the dashboard:
```
https://command.e18n.net/dashboard
```

## Environment

The container uses `.env` file on the server with:
- DATABASE_URL
- SECRET_KEY_BASE
- PHX_HOST
- MQTT settings
