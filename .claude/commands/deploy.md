---
description: Deploy gateway or command app to production servers
allowed-tools: Bash, Read
argument-hint: <target> (netto|avero|command|tui-netto|tui-avero|grafana)
---

# /deploy

Deploy to production servers. Usage: `/deploy <target>`

## Targets

| Target | Description | Script |
|--------|-------------|--------|
| `netto` | Gateway to Netto (avero@100.80.187.3) | `./scripts/deploy-netto.sh` |
| `command` | Phoenix app (root@e18n.net) | `./scripts/deploy-command.sh` |
| `tui-netto` | TUI to Netto (avero@100.80.187.3) | `./scripts/deploy-tui-netto.sh` |
| `tui-avero` | TUI to Avero (avero@100.80.187.4) | `./scripts/deploy-tui-avero.sh` |
| `grafana` | Grafana dashboards (root@e18n.net) | `./scripts/deploy-grafana.sh` |

## Gateway Deployment

Deploys the main gateway binary to Raspberry Pi hosts.

**Steps**: Run tests, cross-compile with zig for aarch64, copy binary, restart systemd service, verify logs.

```bash
./scripts/deploy-netto.sh
```

## Command Deployment

Deploys the Phoenix web app to e18n.net via Docker.

**Steps**: Rsync code, rebuild Docker container, restart, verify logs.

```bash
./scripts/deploy-command.sh
```

## TUI Deployment

Deploys the terminal UI monitoring tool. TUI is not a service - run interactively when needed.

**Steps**: Sync source to server, build on server, copy binary.

```bash
./scripts/deploy-tui-netto.sh
./scripts/deploy-tui-avero.sh
```

## Grafana Deployment

Deploys Grafana dashboards from `grafana/` directory.

```bash
./scripts/deploy-grafana.sh              # Deploys netto-grandi (default)
./scripts/deploy-grafana.sh [dashboard]  # Deploys specific dashboard
```
