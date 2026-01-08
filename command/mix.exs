defmodule AveroCommand.MixProject do
  use Mix.Project

  def project do
    [
      app: :avero_command,
      version: "0.1.0",
      elixir: "~> 1.17",
      elixirc_paths: elixirc_paths(Mix.env()),
      start_permanent: Mix.env() == :prod,
      aliases: aliases(),
      deps: deps()
    ]
  end

  def application do
    [
      mod: {AveroCommand.Application, []},
      extra_applications: [:logger, :runtime_tools]
    ]
  end

  defp elixirc_paths(:test), do: ["lib", "test/support"]
  defp elixirc_paths(_), do: ["lib"]

  defp deps do
    [
      # Phoenix framework
      {:phoenix, "~> 1.8.3"},
      {:phoenix_html, "~> 4.2"},
      {:phoenix_html_helpers, "~> 1.0"},
      {:phoenix_live_reload, "~> 1.6", only: :dev},
      {:phoenix_live_view, "~> 1.0"},
      {:phoenix_live_dashboard, "~> 0.8.7"},

      # Database
      {:ecto_sql, "~> 3.10"},
      {:postgrex, ">= 0.0.0"},

      # MQTT
      {:tortoise311, "~> 0.12"},

      # JSON
      {:jason, "~> 1.4"},

      # Telemetry & metrics
      {:telemetry_metrics, "~> 0.6"},
      {:telemetry_poller, "~> 1.0"},
      {:phoenix_ecto, "~> 4.4"},

      # HTTP server
      {:plug_cowboy, "~> 2.6"},
      {:bandit, "~> 1.0"},

      # Utilities
      {:esbuild, "~> 0.8", runtime: Mix.env() == :dev},
      {:tailwind, "~> 0.2", runtime: Mix.env() == :dev},
      {:heroicons, "~> 0.5"},
      {:floki, ">= 0.30.0", only: :test},

      # UUID generation
      {:elixir_uuid, "~> 1.2"},

      # Scheduler for periodic jobs
      {:quantum, "~> 3.5"},

      # Prometheus metrics
      {:prometheus_ex, "~> 3.1"},
      {:prometheus_plugs, "~> 1.1"}
    ]
  end

  defp aliases do
    [
      setup: ["deps.get", "ecto.setup", "assets.setup", "assets.build"],
      "ecto.setup": ["ecto.create", "ecto.migrate", "run priv/repo/seeds.exs"],
      "ecto.reset": ["ecto.drop", "ecto.setup"],
      test: ["ecto.create --quiet", "ecto.migrate --quiet", "test"],
      "assets.setup": ["tailwind.install --if-missing", "esbuild.install --if-missing"],
      "assets.build": ["tailwind avero_command", "esbuild avero_command"],
      "assets.deploy": [
        "tailwind avero_command --minify",
        "esbuild avero_command --minify",
        "phx.digest"
      ]
    ]
  end
end
