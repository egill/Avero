---
name: build
description: Build gateway-poc for different targets (macOS, Linux aarch64, Raspberry Pi). Use when building the Rust application for local development or cross-compilation.
---

# /build

Build gateway-poc for different targets.

## Local (macOS)

```bash
cargo build --release
```

## Cross-compile for Linux aarch64 (Netto server)

### Option 1: cargo-zigbuild (Recommended)

Install once:
```bash
cargo install cargo-zigbuild
brew install zig
```

Build:
```bash
cargo zigbuild --release --target aarch64-unknown-linux-gnu
```

Binary at: `target/aarch64-unknown-linux-gnu/release/gateway-poc`

### Option 2: Remote build

Build directly on the target server:
```bash
rsync -avz --exclude target --exclude .git ./ avero@HOST:~/gateway-poc-new/
ssh avero@HOST "source ~/.cargo/env && cd ~/gateway-poc-new && cargo build --release"
```

## Targets

| Target | Architecture | OS |
|--------|--------------|-----|
| default | arm64 | macOS |
| aarch64-unknown-linux-gnu | aarch64 | Linux |
| x86_64-unknown-linux-gnu | x86_64 | Linux |

## Test before deploy

```bash
cargo test
cargo clippy
```
