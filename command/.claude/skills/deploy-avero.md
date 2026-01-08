# /deploy-avero

Deploy gateway-poc to Avero (100.65.110.63).

## Process

1. **Run tests** - Ensure all tests pass before deploying
2. **Check binary age** - If `target/aarch64-unknown-linux-gnu/release/gateway-poc` is less than 2 minutes old, skip rebuild
3. **Build if needed** - Cross-compile with `cargo zigbuild --release --target aarch64-unknown-linux-gnu`
4. **Deploy** - Copy binary to server and restart service

## Commands

```bash
# Run tests
cargo test

# Check binary age (skip rebuild if < 2 minutes old)
BINARY="target/aarch64-unknown-linux-gnu/release/gateway-poc"
if [ -f "$BINARY" ]; then
  AGE=$(($(date +%s) - $(stat -f %m "$BINARY" 2>/dev/null || stat -c %Y "$BINARY")))
  if [ $AGE -lt 120 ]; then
    echo "Binary is ${AGE}s old (< 2 min), skipping rebuild"
    SKIP_BUILD=1
  fi
fi

# Build if needed
if [ -z "$SKIP_BUILD" ]; then
  cargo zigbuild --release --target aarch64-unknown-linux-gnu
fi

# Deploy
scp target/aarch64-unknown-linux-gnu/release/gateway-poc avero@100.65.110.63:/tmp/
ssh avero@100.65.110.63 "sudo systemctl stop gateway-poc && cp /tmp/gateway-poc /opt/avero/gateway-poc-bin && sudo systemctl start gateway-poc"

# Verify
ssh avero@100.65.110.63 "sudo journalctl -u gateway-poc --since '10 seconds ago' --no-pager | grep gateway_poc_starting"
```

## Verification

After deployment, check for the startup log with version:
```
INFO gateway_poc_starting version="X.X.X" git_hash="XXXXXXX"
```
