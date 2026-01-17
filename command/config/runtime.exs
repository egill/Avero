import Config

# config/runtime.exs is executed for all environments, including
# during releases. It is executed after compilation and before the
# having system starts, so it is typically used to load production
# configuration and secrets from environment variables or elsewhere.

if config_env() == :prod do
  database_url =
    System.get_env("DATABASE_URL") ||
      raise """
      environment variable DATABASE_URL is missing.
      For example: ecto://USER:PASS@HOST/DATABASE
      """

  config :avero_command, AveroCommand.Repo,
    url: database_url,
    pool_size: String.to_integer(System.get_env("POOL_SIZE") || "10")

  secret_key_base =
    System.get_env("SECRET_KEY_BASE") ||
      raise """
      environment variable SECRET_KEY_BASE is missing.
      You can generate one by calling: mix phx.gen.secret
      """

  host = System.get_env("PHX_HOST") || "localhost"
  port = String.to_integer(System.get_env("PORT") || "4000")

  config :avero_command, AveroCommandWeb.Endpoint,
    url: [host: host, port: 443, scheme: "https"],
    http: [ip: {0, 0, 0, 0}, port: port],
    secret_key_base: secret_key_base,
    check_origin: [
      "https://command.e18n.net",
      "https://dashboard.avero.is",
      "//command.e18n.net",
      "//dashboard.avero.is"
    ]
end

# MQTT configuration (all environments)
config :avero_command, :mqtt,
  host: System.get_env("MQTT_HOST", "localhost"),
  port: String.to_integer(System.get_env("MQTT_PORT", "1883")),
  username: System.get_env("MQTT_USERNAME"),
  password: System.get_env("MQTT_PASSWORD"),
  client_id: System.get_env("MQTT_CLIENT_ID", "avero_command_#{System.get_env("MIX_ENV", "dev")}"),
  topics: String.split(System.get_env("MQTT_TOPICS", "gateway/journeys,gateway/events,gateway/gate,gateway/acc,gateway/metrics,gateway/positions,xovis/sensor"), ",")

# Gateway-PoC configuration
# GATEWAY_SITE: Default site ID for journeys from gateway-poc (if not included in JSON)
config :avero_command, :default_gateway_site, System.get_env("GATEWAY_SITE", "netto")
