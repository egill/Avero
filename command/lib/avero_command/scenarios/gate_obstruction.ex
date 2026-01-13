defmodule AveroCommand.Scenarios.GateObstruction do
  @moduledoc """
  Scenario #22: Rapid Gate Cycling

  Detects when a gate is cycling rapidly (open-close-open-close) with no
  crossings. This indicates a potential sensor issue, obstruction, or malfunction.

  Only triggers on rapid back-to-back empty cycles, not occasional ones.

  Detection: 3+ gate.closed events with 0 crossings within 30 seconds
  Severity: HIGH (indicates malfunction)
  """
  require Logger

  alias AveroCommand.Store

  # Short window - we're looking for rapid back-to-back cycling
  @rapid_window_seconds 30

  # Need 3+ rapid empty cycles to trigger (open-close-open-close-open-close)
  @rapid_cycle_threshold 3

  @doc """
  Evaluate if this event triggers the rapid gate cycling scenario.
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
      if total_crossings == 0, do: check_rapid_cycling(event, gate_id), else: :no_match
    end
  end

  def evaluate(_event), do: :no_match

  defp check_rapid_cycling(event, gate_id) do
    site = event.site
    cutoff = DateTime.add(DateTime.utc_now(), -@rapid_window_seconds, :second)

    # Find recent empty cycles for this gate in the short window
    recent_empty_cycles =
      Store.recent_events(50, site)
      |> Enum.filter(fn e ->
        e.event_type == "gates" &&
          e.data["type"] == "gate.closed" &&
          e.data["gate_id"] == gate_id &&
          DateTime.compare(e.time, cutoff) == :gt &&
          total_crossings_from(e.data) == 0
      end)

    # Include current event in count
    rapid_count = length(recent_empty_cycles) + 1

    # Log for stats (not an incident)
    if rapid_count > 1 do
      Logger.info(
        "GateObstruction: gate #{gate_id} has #{rapid_count} empty cycles in #{@rapid_window_seconds}s"
      )
    end

    # Only alert on rapid back-to-back cycling
    if rapid_count >= @rapid_cycle_threshold do
      {:match, build_incident(event, gate_id, rapid_count)}
    else
      :no_match
    end
  rescue
    e ->
      Logger.warning("GateObstruction: error checking pattern: #{inspect(e)}")
      :no_match
  end

  defp build_incident(event, gate_id, rapid_count) do
    %{
      type: "gate_rapid_cycling",
      severity: "high",
      category: "equipment",
      site: event.site,
      gate_id: gate_id,
      context: %{
        gate_id: gate_id,
        rapid_cycle_count: rapid_count,
        window_seconds: @rapid_window_seconds,
        message:
          "Gate #{gate_id} cycling rapidly - #{rapid_count} empty cycles in #{@rapid_window_seconds}s"
      },
      suggested_actions: build_actions()
    }
  end

  defp build_actions do
    [
      %{"id" => "check_sensor", "label" => "Check Sensor", "auto" => false},
      %{"id" => "acknowledge", "label" => "Acknowledge", "auto" => false}
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
