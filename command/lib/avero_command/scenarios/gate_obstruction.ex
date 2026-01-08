defmodule AveroCommand.Scenarios.GateObstruction do
  @moduledoc """
  Scenario #22: Gate Obstruction / False Triggers

  Detects when a gate has multiple open/close cycles where nobody
  actually crossed. This indicates:
  - Sensor misalignment or debris triggering false positives
  - Gate hardware issues causing spurious opens
  - Objects obstructing the sensor beam

  Detection: gate.closed events with exit_summary.total_crossings == 0
  If we see multiple such "empty cycles" in a short window, that's concerning.

  Trigger: gate.closed with total_crossings == 0
  Severity: MEDIUM (2-3 empty cycles) or HIGH (4+ empty cycles)
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
    exit_summary = data["exit_summary"] || %{}
    total_crossings = exit_summary["total_crossings"] || 0

    # Skip gate_id 0 (invalid) - this filters out malformed events
    if gate_id == 0 do
      :no_match
    else
      # Only consider this if nobody crossed (empty cycle)
      if total_crossings == 0 do
        check_empty_cycle_pattern(event, gate_id)
      else
        :no_match
      end
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
          (e.data["exit_summary"]["total_crossings"] || 0) == 0
      end)
      |> length()

    # Include current event in count
    empty_cycle_count = empty_cycle_count + 1

    Logger.debug("GateObstruction: gate #{gate_id} has #{empty_cycle_count} empty cycles in #{@window_seconds}s")

    cond do
      empty_cycle_count >= @high_threshold ->
        {:match, build_incident(event, gate_id, empty_cycle_count, "high")}

      empty_cycle_count >= @warning_threshold ->
        {:match, build_incident(event, gate_id, empty_cycle_count, "medium")}

      true ->
        :no_match
    end
  rescue
    e ->
      Logger.warning("GateObstruction: error checking pattern: #{inspect(e)}")
      :no_match
  end

  defp build_incident(event, gate_id, empty_cycle_count, severity) do
    %{
      type: "gate_obstruction",
      severity: severity,
      category: "equipment",
      site: event.site,
      gate_id: gate_id,
      context: %{
        gate_id: gate_id,
        empty_cycle_count: empty_cycle_count,
        window_seconds: @window_seconds,
        message: "Gate #{gate_id} had #{empty_cycle_count} empty cycles (no crossings) in #{@window_seconds}s - possible sensor issue or obstruction"
      },
      suggested_actions: build_actions(severity)
    }
  end

  defp build_actions("high") do
    [
      %{"id" => "check_sensor", "label" => "Check Sensor Alignment", "auto" => false},
      %{"id" => "check_gate_area", "label" => "Inspect Gate Area", "auto" => false},
      %{"id" => "notify_maintenance", "label" => "Notify Maintenance", "auto" => true},
      %{"id" => "acknowledge", "label" => "Acknowledge", "auto" => false}
    ]
  end

  defp build_actions(_) do
    [
      %{"id" => "check_sensor", "label" => "Check Sensor Alignment", "auto" => false},
      %{"id" => "acknowledge", "label" => "Acknowledge", "auto" => false},
      %{"id" => "dismiss", "label" => "Dismiss", "auto" => false}
    ]
  end
end
