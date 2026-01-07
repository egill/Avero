# Gate Latency Benchmark

Baseline comparison of Rust vs Go for gate control latency.

## What It Measures

**Command → Door Moving**: Time from TCP command sent to RS485 detecting door movement.

This isolates the gate control path without MQTT/Xovis dependencies:
- TCP command transmission to CloudPlus controller
- CloudPlus processing
- Physical gate motor activation
- RS485 polling detecting "moving" state

## Hardware Requirements

- Raspberry Pi (or similar) with:
  - Network access to CloudPlus gate controller (TCP port 8000)
  - RS485 serial connection to door status monitor (/dev/ttyUSB0)

## Build

```bash
# Build for current machine
./build.sh

# Build for Raspberry Pi (cross-compile)
./build.sh pi
```

### Build Optimizations

**Rust** (release profile):
- `opt-level = 3` - Maximum optimization
- `lto = "fat"` - Full link-time optimization
- `codegen-units = 1` - Single codegen unit for better optimization
- `panic = "abort"` - No unwinding overhead
- `strip = true` - Remove symbols

**Go**:
- `-ldflags="-s -w"` - Strip debug info and symbols
- `CGO_ENABLED=0` - Static binary (for cross-compile)

## Run Experiment

```bash
# On the target device (e.g., Avero HQ Pi)
./run-experiment.sh --trials 30 --delay 5

# Custom parameters
./run-experiment.sh \
    --trials 50 \
    --delay 3 \
    --gate 192.168.0.245:8000 \
    --rs485 /dev/ttyUSB0 \
    --output ./my-results
```

## Expected Results

Based on production metrics (Netto), expect:
- **Gate response**: 500-1000ms (avg ~773ms)
- This includes:
  - ~125ms average RS485 polling delay (250ms refresh)
  - TCP/gate controller latency
  - Physical motor activation

## Output

Results are saved to `./results/<timestamp>/`:
- `system-info.txt` - OS and toolchain versions
- `rust-output.txt` - Rust benchmark output
- `go-output.txt` - Go benchmark output

Each output includes:
- Per-trial latency in milliseconds
- Statistics: min, max, avg, P50, P95
