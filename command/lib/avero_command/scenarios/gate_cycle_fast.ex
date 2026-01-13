defmodule AveroCommand.Scenarios.GateCycleFast do
  @moduledoc """
  Scenario #21: Gate Cycle Too Fast

  Detects when a gate opens and closes in less than 1 second,
  which could indicate:
  - Faulty sensor signal
  - Gate hardware issue
  - False trigger

  Trigger: gate.closed event with very short open_duration
  Severity: MEDIUM
  """
  require Logger

  # Minimum expected gate cycle time in milliseconds
  @min_cycle_ms 1000

  @doc """
  Evaluate if this event triggers the gate-cycle-fast scenario.
  """
  def evaluate(%{event_type: "gates", data: %{"type" => "gate.closed"} = data} = event) do
    gate_id = data["gate_id"] || 0
    open_duration_ms = data["open_duration_ms"] || 0

    # Skip gate_id 0 (invalid) and require valid duration
    if gate_id > 0 && open_duration_ms > 0 && open_duration_ms < @min_cycle_ms do
      Logger.warning("GateCycleFast: gate closed after only #{open_duration_ms}ms")
      {:match, build_incident(event, data, open_duration_ms)}
    else
      :no_match
    end
  end

  def evaluate(_event), do: :no_match

  defp build_incident(event, data, open_duration_ms) do
    gate_id = data["gate_id"] || 0
    exit_summary = data["exit_summary"] || %{}
    total_crossings = exit_summary["total_crossings"] || 0

    %{
      type: "gate_cycle_fast",
      severity: "medium",
      category: "equipment",
      site: event.site,
      gate_id: gate_id,
      context: %{
        gate_id: gate_id,
        open_duration_ms: open_duration_ms,
        min_expected_ms: @min_cycle_ms,
        total_crossings: total_crossings,
        message:
          "Gate #{gate_id} cycled in #{open_duration_ms}ms (min expected: #{@min_cycle_ms}ms, crossings: #{total_crossings})"
      },
      suggested_actions: [
        %{"id" => "check_sensor", "label" => "Check Sensor Signal", "auto" => false},
        %{"id" => "check_gate", "label" => "Inspect Gate Hardware", "auto" => false},
        %{"id" => "acknowledge", "label" => "Acknowledge", "auto" => false}
      ]
    }
  end
end
