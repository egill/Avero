---
name: deploy-tui-avero
description: Deploy gateway-tui to Avero server (100.80.187.4). Use when deploying the TUI monitoring dashboard to Avero.
---

# /deploy-tui-avero

Deploy gateway-tui to the Avero server.

## Steps

1. Sync source code to avero@100.80.187.4
2. Build gateway-tui on server
3. Copy binary to /opt/avero/gateway-poc/target/release/

## Command

```bash
./scripts/deploy-tui-avero.sh
```

## Run TUI on Avero

```bash
ssh avero@100.80.187.4 '/opt/avero/gateway-poc/target/release/gateway-tui --config /opt/avero/gateway-poc/config/avero.toml'
```

## Note

The TUI is not a service - it's a manual monitoring tool. Run it interactively when needed.
