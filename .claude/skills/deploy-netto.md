# /deploy-netto

Deploy gateway-poc to the Netto production server.

## Steps

1. Sync source code to avero@100.80.187.3
2. Build on server (cargo build --release)
3. Stop gateway-poc service
4. Copy new binary
5. Start gateway-poc service
6. Verify service is running

## Command

```bash
./scripts/deploy-netto.sh
```

## Manual Deploy

```bash
HOST="avero@100.80.187.3"
rsync -avz --exclude target --exclude .git ./ $HOST:~/gateway-poc-new/
ssh $HOST "source ~/.cargo/env && cd ~/gateway-poc-new && cargo build --release"
ssh $HOST "sudo systemctl stop gateway-poc && sleep 2 && cp ~/gateway-poc-new/target/release/gateway-poc /opt/avero/gateway-poc/target/release/ && sudo systemctl start gateway-poc"
ssh $HOST "sudo systemctl status gateway-poc"
```

## Verify

After deploy, check logs:
```bash
ssh avero@100.80.187.3 "sudo journalctl -u gateway-poc -f"
```
