defmodule AveroCommand.Metrics do
  @moduledoc """
  Prometheus metrics for the Command application.

  Metrics exposed:
  - avero_command_events_total - Events received by topic and type
  - avero_command_scenarios_evaluated_total - Scenario evaluation results
  - avero_command_incidents_created_total - Incidents created by type/severity
  - avero_command_incidents_active - Gauge of currently active incidents
  """
  use Prometheus.Metric
  require Logger

  @doc """
  Initialize all Prometheus metrics.
  Must be called before any metrics are recorded (typically in Application.start).
  """
  def setup do
    # Event processing counters
    Counter.declare(
      name: :avero_command_events_total,
      help: "Total events received from MQTT",
      labels: [:topic, :event_type, :site]
    )

    # Scenario evaluation counters
    Counter.declare(
      name: :avero_command_scenarios_evaluated_total,
      help: "Scenario evaluation results (match/no_match/error)",
      labels: [:scenario, :result]
    )

    Histogram.declare(
      name: :avero_command_scenarios_evaluation_duration_seconds,
      help: "Scenario evaluation duration in seconds",
      labels: [:scenario],
      buckets: [0.0001, 0.0005, 0.001, 0.005, 0.01, 0.05, 0.1, 0.5, 1.0]
    )

    # Incident counters
    Counter.declare(
      name: :avero_command_incidents_created_total,
      help: "Incidents created",
      labels: [:type, :severity, :category, :site]
    )

    Counter.declare(
      name: :avero_command_incidents_duplicates_total,
      help: "Duplicate incidents skipped (deduplication)",
      labels: [:type, :site]
    )

    Gauge.declare(
      name: :avero_command_incidents_active,
      help: "Currently active incidents (new/acknowledged/in_progress)"
    )

    Logger.info("Metrics: Prometheus metrics initialized")
    :ok
  rescue
    e ->
      Logger.warning("Metrics: Failed to initialize - #{inspect(e)}")
      :error
  end

  # =============================================================================
  # Event Metrics
  # =============================================================================

  @doc "Increment event received counter"
  def inc_event_received(topic, event_type, site) do
    Counter.inc(
      name: :avero_command_events_total,
      labels: [topic, event_type, site || "unknown"]
    )
  rescue
    _ -> :ok
  end

  # =============================================================================
  # Scenario Metrics
  # =============================================================================

  @doc "Increment scenario evaluation result counter"
  def inc_scenario_result(scenario, result) when result in [:match, :no_match, :error] do
    Counter.inc(
      name: :avero_command_scenarios_evaluated_total,
      labels: [scenario, Atom.to_string(result)]
    )
  rescue
    _ -> :ok
  end

  @doc "Record scenario evaluation duration"
  def observe_scenario_duration(scenario, duration_seconds) when is_number(duration_seconds) do
    Histogram.observe(
      [name: :avero_command_scenarios_evaluation_duration_seconds, labels: [scenario]],
      duration_seconds
    )
  rescue
    _ -> :ok
  end

  # =============================================================================
  # Incident Metrics
  # =============================================================================

  @doc "Increment incident created counter"
  def inc_incident_created(type, severity, category, site) do
    Counter.inc(
      name: :avero_command_incidents_created_total,
      labels: [type || "unknown", severity || "unknown", category || "unknown", site || "unknown"]
    )
  rescue
    _ -> :ok
  end

  @doc "Increment duplicate incident counter"
  def inc_incident_duplicate(type, site) do
    Counter.inc(
      name: :avero_command_incidents_duplicates_total,
      labels: [type || "unknown", site || "unknown"]
    )
  rescue
    _ -> :ok
  end

  @doc "Set active incidents gauge"
  def set_active_incidents(count) when is_integer(count) do
    Gauge.set([name: :avero_command_incidents_active], count)
  rescue
    _ -> :ok
  end
end
