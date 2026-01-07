# Gateway Deployment Guide

## Targets

| Name | Host | Service | Config |
|------|------|---------|--------|
| netto | avero@100.80.187.3 | gateway-poc | netto.toml |

## Build Options

### 1. Remote Build (Current)
Build directly on the target server:
```bash
rsync -avz --exclude target --exclude .git ./ avero@HOST:~/gateway-poc-new/
ssh avero@HOST "source ~/.cargo/env && cd ~/gateway-poc-new && cargo build --release"
```

### 2. Cross-compile with Zig (Recommended)
Install cargo-zigbuild for fast local cross-compilation:
```bash
# Install once
cargo install cargo-zigbuild
brew install zig  # or apt install zig

# Build for Linux aarch64
cargo zigbuild --release --target aarch64-unknown-linux-gnu
```

### 3. Cross-compile with GNU toolchain
```bash
# Install toolchain (macOS)
brew install aarch64-unknown-linux-gnu

# Build
cargo build --release --target aarch64-unknown-linux-gnu
```

## Deploy Steps

1. **Stop service**: `ssh avero@HOST "sudo systemctl stop gateway-poc"`
2. **Copy binary**: `scp target/*/release/gateway-poc avero@HOST:/opt/avero/gateway-poc/target/release/`
3. **Start service**: `ssh avero@HOST "sudo systemctl start gateway-poc"`
4. **Verify**: `ssh avero@HOST "sudo systemctl status gateway-poc"`

## Quick Deploy Commands

### Netto
```bash
# Full deploy (build + deploy)
./scripts/deploy-netto.sh

# Or manually
rsync -avz --exclude target --exclude .git ./ avero@100.80.187.3:~/gateway-poc-new/
ssh avero@100.80.187.3 "source ~/.cargo/env && cd ~/gateway-poc-new && cargo build --release"
ssh avero@100.80.187.3 "sudo systemctl stop gateway-poc && sleep 2 && cp ~/gateway-poc-new/target/release/gateway-poc /opt/avero/gateway-poc/target/release/ && sudo systemctl start gateway-poc"
```

## TUI Deployment

The gateway-tui is a monitoring tool, not a service.

```bash
# Deploy to Netto
./scripts/deploy-tui-netto.sh

# Deploy to Avero
./scripts/deploy-tui-avero.sh

# Run on server
ssh avero@HOST '/opt/avero/gateway-poc/target/release/gateway-tui --config /opt/avero/gateway-poc/config/netto.toml'
```

## Monitoring

```bash
# Logs
ssh avero@HOST "sudo journalctl -u gateway-poc -f"

# Status
ssh avero@HOST "sudo systemctl status gateway-poc"
```

## Rollback

```bash
# Keep previous binary as .bak before deploying
ssh avero@HOST "cp /opt/avero/gateway-poc/target/release/gateway-poc /opt/avero/gateway-poc/target/release/gateway-poc.bak"

# Rollback
ssh avero@HOST "sudo systemctl stop gateway-poc && cp /opt/avero/gateway-poc/target/release/gateway-poc.bak /opt/avero/gateway-poc/target/release/gateway-poc && sudo systemctl start gateway-poc"
```
