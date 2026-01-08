---
name: deploy-netto
description: Deploy gateway-poc to Netto production server (100.80.187.3). Use when deploying the main gateway service to Netto production.
---

# /deploy-netto

Deploy gateway-poc to the Netto production server.

## Steps

1. Run tests locally (cargo test)
2. Cross-compile with zig for aarch64-unknown-linux-gnu
3. Copy binary to server
4. Stop gateway-poc service
5. Install new binary
6. Start gateway-poc service
7. Verify service is running

## Command

```bash
./scripts/deploy-netto.sh
```

## Manual Deploy

```bash
# Use rustup's cargo (not Homebrew) and zig 0.14
export PATH="$HOME/.cargo/bin:/opt/homebrew/opt/zig@0.14/bin:$PATH"

HOST="avero@100.80.187.3"
TARGET="aarch64-unknown-linux-gnu"

cargo test
cargo zigbuild --release --target "$TARGET"
scp "target/$TARGET/release/gateway-poc" "$HOST:/tmp/gateway-poc"
ssh $HOST "sudo systemctl stop gateway-poc && sleep 2 && sudo cp /tmp/gateway-poc /opt/avero/gateway-poc/target/release/ && sudo systemctl start gateway-poc"
ssh $HOST "sudo systemctl status gateway-poc"
```

## Verify

After deploy, check logs:
```bash
ssh avero@100.80.187.3 "sudo journalctl -u gateway-poc -f"
```
