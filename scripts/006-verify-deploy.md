# Task: Verify and Deploy

## Summary
Run quality checks and deploy to Netto for validation.

## Steps

### 1. Quality Checks
```bash
cargo test
cargo clippy -- -D warnings
cargo build --release
```

### 2. Deploy to Netto
```bash
scp target/release/gateway-poc avero@100.80.187.3:/home/avero/gateway-poc-bin-new
ssh avero@100.80.187.3 "sudo systemctl stop gateway-poc && sudo cp /home/avero/gateway-poc-bin-new /usr/local/bin/gateway-poc-bin && sudo systemctl start gateway-poc"
```

### 3. Verify
```bash
# Check service started
ssh avero@100.80.187.3 "sudo systemctl status gateway-poc"

# Monitor ACC matching for 30 minutes
ssh avero@100.80.187.3 "journalctl -u gateway-poc -f | grep -E 'acc_matched|acc_unmatched|acc_buffered'"
```

### 4. Success Criteria
- 100% ACC match rate maintained (or improved)
- No new errors in logs
- Group sizes logged correctly
- Buffered ACC events matched when person arrives

## Definition of Done
- [ ] `cargo test` passes
- [ ] `cargo clippy` clean
- [ ] Deployed to Netto
- [ ] 30 minutes observation with no issues
- [ ] ACC match rate >= 100% (compared to before)
