# /deploy-command

Deploy the Phoenix/Elixir command application to command.e18n.net.

## Architecture

- Server: root@e18n.net
- Container: avero-command (Docker)
- Source mount: /opt/avero/command-src -> /app
- App runs at: https://command.e18n.net

## Process

1. **Compile locally** - Run `mix compile` to check for errors
2. **Sync source** - rsync the `command/` folder to `/opt/avero/command-src`
3. **Restart container** - Docker restart to pick up changes and run migrations

## Commands

```bash
# Compile locally to check for errors
cd command
mix compile

# Sync source to server (exclude build artifacts)
rsync -avz --delete \
  --exclude '_build' \
  --exclude 'deps' \
  --exclude '.elixir_ls' \
  --exclude 'node_modules' \
  --exclude '*.beam' \
  --exclude 'erl_crash.dump' \
  command/ root@e18n.net:/opt/avero/command-src/

# Restart the container (will run deps.get, migrations, and start server)
ssh root@e18n.net "docker restart avero-command"

# Wait for startup and check logs
sleep 5
ssh root@e18n.net "docker logs avero-command --tail 20"
```

## Verification

After deployment:
1. Check docker logs for successful startup
2. Verify the site is accessible at https://command.e18n.net
3. Check for any migration output in logs

## Notes

- The container mounts source directly, so changes are picked up on restart
- Container runs: `mix deps.get && mix ecto.create && mix phx.server`
- Migrations run automatically on container start
