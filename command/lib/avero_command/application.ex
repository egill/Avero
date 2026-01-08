defmodule AveroCommand.Application do
  @moduledoc false

  use Application
  require Logger

  @impl true
  def start(_type, _args) do
    # Initialize Prometheus metrics before starting supervision tree
    AveroCommand.Metrics.setup()

    # Initialize the scenario evaluator warmup timer
    AveroCommand.Scenarios.Evaluator.init()
    Logger.info("Application: Metrics and Evaluator initialized")

    children = [
      # Database
      AveroCommand.Repo,

      # PubSub for LiveView
      {Phoenix.PubSub, name: AveroCommand.PubSub},

      # Entity Supervisors (dynamic supervisors for Person/Gate GenServers)
      {DynamicSupervisor, name: AveroCommand.PersonSupervisor, strategy: :one_for_one},
      {DynamicSupervisor, name: AveroCommand.GateSupervisor, strategy: :one_for_one},

      # Entity Registry (track active persons/gates by ID)
      {Registry, keys: :unique, name: AveroCommand.EntityRegistry},

      # MQTT Client
      AveroCommand.MQTT.Client,

      # Incident Manager
      AveroCommand.Incidents.Manager,

      # Scheduler for periodic jobs
      AveroCommand.Scheduler,

      # Phoenix Endpoint (must be last)
      AveroCommandWeb.Endpoint
    ]

    opts = [strategy: :one_for_one, name: AveroCommand.Supervisor]
    Supervisor.start_link(children, opts)
  end

  @impl true
  def config_change(changed, _new, removed) do
    AveroCommandWeb.Endpoint.config_change(changed, removed)
    :ok
  end
end
