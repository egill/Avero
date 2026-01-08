import Config

# Configure your database (TimescaleDB)
# Local: localhost:5433, Docker: timescaledb:5432
config :avero_command, AveroCommand.Repo,
  username: "avero",
  password: "avero_dev",
  hostname: System.get_env("DATABASE_HOST", "localhost"),
  port: String.to_integer(System.get_env("DATABASE_PORT", "5433")),
  database: "avero_command_dev",
  stacktrace: true,
  show_sensitive_data_on_connection_error: true,
  pool_size: 10

# For development, we disable any cache and enable
# debugging and code reloading.
config :avero_command, AveroCommandWeb.Endpoint,
  http: [ip: {0, 0, 0, 0}, port: 4000],
  check_origin: false,
  code_reloader: true,
  debug_errors: true,
  secret_key_base: "dev_secret_key_base_at_least_64_chars_long_for_development_only_do_not_use_in_production",
  watchers: [
    esbuild: {Esbuild, :install_and_run, [:avero_command, ~w(--sourcemap=inline --watch)]},
    tailwind: {Tailwind, :install_and_run, [:avero_command, ~w(--watch)]}
  ]

# Watch static and templates for browser reloading.
config :avero_command, AveroCommandWeb.Endpoint,
  live_reload: [
    patterns: [
      ~r"priv/static/(?!uploads/).*(js|css|png|jpeg|jpg|gif|svg)$",
      ~r"lib/avero_command_web/(controllers|live|components)/.*(ex|heex)$"
    ]
  ]

# Enable dev routes for dashboard and mailbox
config :avero_command, dev_routes: true

# Do not include metadata nor timestamps in development logs
config :logger, :console, format: "[$level] $message\n"

# Set a higher stacktrace during development
config :phoenix, :stacktrace_depth, 20

# Initialize plugs at runtime for faster development compilation
config :phoenix, :plug_init_mode, :runtime

config :phoenix_live_view,
  debug_heex_annotations: true,
  enable_expensive_runtime_checks: true

# MQTT configuration for development
config :avero_command, :mqtt,
  host: System.get_env("MQTT_HOST", "localhost"),
  port: String.to_integer(System.get_env("MQTT_PORT", "1883")),
  client_id: "avero_command_dev",
  topics: ["avero/events/#"]
