# This file is responsible for configuring your application
# and its dependencies with the aid of the Config module.
import Config

config :avero_command,
  ecto_repos: [AveroCommand.Repo],
  generators: [timestamp_type: :utc_datetime]

# Configures the endpoint
config :avero_command, AveroCommandWeb.Endpoint,
  url: [host: "localhost"],
  adapter: Bandit.PhoenixAdapter,
  render_errors: [
    formats: [html: AveroCommandWeb.ErrorHTML, json: AveroCommandWeb.ErrorJSON],
    layout: false
  ],
  pubsub_server: AveroCommand.PubSub,
  live_view: [signing_salt: "avero_command_salt"]

# Configure esbuild (the version is required)
config :esbuild,
  version: "0.17.11",
  avero_command: [
    args: ~w(js/app.js --bundle --target=es2017 --outdir=../priv/static/assets --external:/fonts/* --external:/images/*),
    cd: Path.expand("../assets", __DIR__),
    env: %{"NODE_PATH" => Path.expand("../deps", __DIR__)}
  ]

# Configure tailwind (the version is required)
config :tailwind,
  version: "3.4.0",
  avero_command: [
    args: ~w(
      --config=tailwind.config.js
      --input=css/app.css
      --output=../priv/static/assets/app.css
    ),
    cd: Path.expand("../assets", __DIR__)
  ]

# Configures Elixir's Logger
config :logger, :console,
  format: "$time $metadata[$level] $message\n",
  metadata: [:request_id]

# Use Jason for JSON parsing in Phoenix
config :phoenix, :json_library, Jason

# Configure Quantum scheduler for periodic jobs
config :avero_command, AveroCommand.Scheduler,
  jobs: [
    # Site offline check every 5 minutes
    {"*/5 * * * *", {AveroCommand.Scenarios.SiteOffline, :run, []}},
    # Traffic anomaly check hourly
    {"5 * * * *", {AveroCommand.Reports.TrafficAnomaly, :run, []}},
    # Daily summary at midnight
    {"0 0 * * *", {AveroCommand.Reports.DailySummary, :run, []}},
    # Site comparison daily at midnight
    {"0 0 * * *", {AveroCommand.Reports.SiteComparison, :run, []}},
    # Last customer check after midnight (store closed)
    {"15 0 * * *", {AveroCommand.Reports.LastCustomer, :run, []}}
  ]

# Import environment specific config
import_config "#{config_env()}.exs"
