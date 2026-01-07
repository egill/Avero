# Ralph Agent Instructions - Gate Latency Benchmark

## Critical Success Factors

### 1. Small Stories
Must fit in one context window.
```
❌ Too big:
> "Build entire benchmark suite"

✅ Right size:
> "Implement RS485 polling"
> "Add CloudPlus open command"
> "Add latency statistics"
```

### 2. Feedback Loops
Ralph needs fast feedback:
```bash
./build.sh pi
ssh avero@100.65.110.63 "./gate-bench/gate-bench-rust --trials 1"
```
Without these, broken code compounds.

### 3. Explicit Criteria
```
❌ Vague:
> "RS485 works"

✅ Explicit:
> - Status byte from frame[4]
> - Checksum: sum + 1 == 0
> - Returns Moving when door moves
> - Verify on Pi with --trials 1
```

### 4. Learnings Compound
By story 10, Ralph knows patterns from stories 1-9.

Two places for learnings:
- `progress.txt` — session memory for Ralph iterations
- `CLAUDE.md` — permanent docs for humans and future agents

Before committing, update CLAUDE.md if you discovered reusable patterns.

### 5. CLAUDE.md Updates
Update when you learn something worth preserving:
```
✅ Good additions:
- "RS485 status byte is at position 4, not 10"
- "Gate closes idle TCP - reconnect per trial"
- "Door 0x02 = resting = closed"

❌ Don't add:
- Story-specific details
- Temporary notes
- Info already in progress.txt
```

---

## Your Task

1. Read `bench/prd.json`
2. Read `bench/progress.txt`
   (check Codebase Patterns first)
3. Check you're on the correct branch
4. Pick highest priority story
   where `passes: false`
5. Implement that ONE story
6. Test on Pi: `ssh avero@100.65.110.63`
7. Update CLAUDE.md files with learnings
8. Commit: `feat: [ID] - [Title]`
9. Update prd-test.json: `passes: true`
10. Append learnings to progress.txt

## Reference Code

**CRITICAL**: Use production code as reference:
- RS485: `src/io/rs485.rs`
- CloudPlus: `src/io/cloudplus.rs`
- Working test: `src/bin/gate_test.rs`

Copy implementations exactly. Do not improvise.

## Hardware Config

- Pi: `avero@100.65.110.63`
- RS485: `/dev/ttyAMA4` @ 19200 baud
- Gate: `192.168.0.245:8000`
- Poll interval: 250ms

## RS485 Protocol (from rs485.rs)

- Command: 8 bytes, starts 0x7E
- Response: 18 bytes, starts 0x7F
- Door status: byte 4 (NOT byte 10)
- Checksum: sum all bytes + 1 == 0
- Status codes: 0x00/0x02=closed, 0x03=moving

## CloudPlus Protocol (from cloudplus.rs)

- Frame: STX(0x02), rand, cmd, addr(0xFF), door(0x01), len_lo, len_hi, checksum, ETX(0x03)
- Open command: 0x2C
- Checksum: XOR all bytes before checksum

## Progress Format

APPEND to progress.txt:

```
## [Date] - [Story ID]
- What was implemented
- Files changed
- **Learnings:**
  - Patterns discovered
  - Gotchas encountered
---
```

## Codebase Patterns

Add reusable patterns to the TOP
of progress.txt:

```
## Codebase Patterns
- RS485 status byte is at position 4, not 10
- Door 0x02 (right open) = resting = CLOSED
- Gate closes idle TCP connections - reconnect per trial
- Fresh TCP connection before each command
```

## Build & Deploy

```bash
cd bench
./build.sh pi
scp bin/aarch64-linux/gate-bench-rust avero@100.65.110.63:~/gate-bench/
ssh avero@100.65.110.63 "./gate-bench/gate-bench-rust --rs485-device /dev/ttyAMA4 --trials 5 --delay 11"
```

## Stop Condition

If ALL stories pass, reply:
<promise>COMPLETE</promise>

Otherwise end normally.
