defmodule AveroCommand.Scenarios.GateObstruction do
  @moduledoc """
  Scenario #22: Gate Empty Cycles (Stats)

  Detects when a gate has multiple open/close cycles where nobody
  actually crossed. This is recorded as a statistical observation only.

  Detection: gate.closed events with exit_summary.total_crossings == 0
  Trigger: gate.closed with total_crossings == 0
  Severity: INFO (stats only)
  """
  require Logger

  alias AveroCommand.Store

  # Time window to look for multiple empty cycles (seconds)
  @window_seconds 120

  # Thresholds for empty cycles in window
  @warning_threshold 2
  @high_threshold 4

  @doc """
  Evaluate if this event triggers the gate-obstruction scenario.
  Only triggers on gate.closed events where total_crossings == 0.
  """
  def evaluate(%{event_type: "gates", data: %{"type" => "gate.closed"} = data} = event) do
    gate_id = data["gate_id"] || 0
    total_crossings = total_crossings_from(data)

    # Skip gate_id 0 (invalid) - this filters out malformed events
    if gate_id == 0 do
      :no_match
    else
      # Only consider this if we have explicit crossing counts and nobody crossed
      if total_crossings == 0, do: check_empty_cycle_pattern(event, gate_id), else: :no_match
    end
  end

  def evaluate(_event), do: :no_match

  defp check_empty_cycle_pattern(event, gate_id) do
    site = event.site
    cutoff = DateTime.add(DateTime.utc_now(), -@window_seconds, :second)

    # Count recent empty cycles for this gate
    empty_cycle_count =
      Store.recent_events(100, site)
      |> Enum.filter(fn e ->
        e.event_type == "gates" &&
          e.data["type"] == "gate.closed" &&
          e.data["gate_id"] == gate_id &&
          DateTime.compare(e.time, cutoff) == :gt &&
          total_crossings_from(e.data) == 0
      end)
      |> length()

    # Include current event in count
    empty_cycle_count = empty_cycle_count + 1

    Logger.debug("GateObstruction: gate #{gate_id} has #{empty_cycle_count} empty cycles in #{@window_seconds}s")

    cond do
      empty_cycle_count >= @high_threshold ->
        {:match, build_incident(event, gate_id, empty_cycle_count)}

      empty_cycle_count >= @warning_threshold ->
        {:match, build_incident(event, gate_id, empty_cycle_count)}

      true ->
        :no_match
    end
  rescue
    e ->
      Logger.warning("GateObstruction: error checking pattern: #{inspect(e)}")
      :no_match
  end

  defp build_incident(event, gate_id, empty_cycle_count) do
    %{
      type: "gate_empty_cycles",
      severity: "info",
      category: "operational",
      site: event.site,
      gate_id: gate_id,
      context: %{
        gate_id: gate_id,
        empty_cycle_count: empty_cycle_count,
        window_seconds: @window_seconds,
        message: "Gate #{gate_id} had #{empty_cycle_count} empty cycles (no crossings) in #{@window_seconds}s"
      },
      suggested_actions: build_actions()
    }
  end

  defp build_actions do
    [
      %{"id" => "acknowledge", "label" => "Acknowledge", "auto" => false},
      %{"id" => "dismiss", "label" => "Dismiss", "auto" => false}
    ]
  end

  defp total_crossings_from(data) when is_map(data) do
    exit_summary = data["exit_summary"]

    cond do
      is_map(exit_summary) -> normalize_count(exit_summary["total_crossings"])
      true -> normalize_count(data["crossing_count"])
    end
  end

  defp total_crossings_from(_), do: nil

  defp normalize_count(value) when is_integer(value), do: value
  defp normalize_count(value) when is_float(value), do: trunc(value)
  defp normalize_count(value) when is_binary(value) do
    case Integer.parse(value) do
      {int, ""} -> int
      _ -> nil
    end
  end
  defp normalize_count(_), do: nil
end
