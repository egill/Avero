# /deploy-tui-netto

Deploy gateway-tui to the Netto server.

## Steps

1. Sync source code to avero@100.80.187.3
2. Build gateway-tui on server
3. Copy binary to /opt/avero/gateway-poc/target/release/

## Command

```bash
./scripts/deploy-tui-netto.sh
```

## Run TUI on Netto

```bash
ssh avero@100.80.187.3 '/opt/avero/gateway-poc/target/release/gateway-tui --config /opt/avero/gateway-poc/config/netto.toml'
```

## Note

The TUI is not a service - it's a manual monitoring tool. Run it interactively when needed.
