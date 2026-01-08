import Config

# Production configuration
# Most settings come from runtime.exs via environment variables

config :avero_command, AveroCommandWeb.Endpoint,
  cache_static_manifest: "priv/static/cache_manifest.json",
  server: true

# Do not print debug messages in production
config :logger, level: :info

# Runtime production config is in runtime.exs
