defmodule AveroCommand.Scenarios.HighTraffic do
  @moduledoc """
  Scenario #6: High Traffic Alert

  Detects when too many people are being tracked simultaneously,
  indicating potential congestion or system capacity concerns.

  Trigger: system.metrics event with high concurrent person count
  Severity: MEDIUM (10-20 people) or HIGH (20+ people)
  """
  require Logger

  # Thresholds for concurrent tracked persons
  @warning_threshold 10
  @elevated_threshold 20

  @doc """
  Evaluate if this event triggers the high-traffic scenario.
  Event comes through as event_type: "system.metrics" with concurrent_persons count.
  """
  def evaluate(%{event_type: "system.metrics", data: data} = event) do
    concurrent = data["concurrent_persons"] || data["tracked_count"] || 0
    check_threshold(event, data, concurrent)
  end

  # Also check gate.closed events which have max_zone_occupancy
  def evaluate(%{event_type: "gates", data: %{"type" => "gate.closed"} = data} = event) do
    max_occupancy = data["max_zone_occupancy"] || 0

    if max_occupancy >= @elevated_threshold do
      {:match, build_incident_from_gate(event, data, max_occupancy)}
    else
      :no_match
    end
  end

  def evaluate(_event), do: :no_match

  defp check_threshold(event, data, concurrent) when concurrent >= @elevated_threshold do
    {:match, build_incident(event, data, concurrent, "high")}
  end

  defp check_threshold(event, data, concurrent) when concurrent >= @warning_threshold do
    {:match, build_incident(event, data, concurrent, "medium")}
  end

  defp check_threshold(_event, _data, _concurrent), do: :no_match

  defp build_incident(event, data, concurrent, severity) do
    %{
      type: "high_traffic",
      severity: severity,
      category: "operational",
      site: event.site,
      gate_id: 0,
      context: %{
        concurrent_persons: concurrent,
        threshold: if(severity == "high", do: @elevated_threshold, else: @warning_threshold),
        queue_depth: data["event_queue_depth"] || 0,
        message: "#{concurrent} people currently tracked (threshold: #{if severity == "high", do: @elevated_threshold, else: @warning_threshold})"
      },
      suggested_actions: [
        %{"id" => "monitor", "label" => "Monitor Closely", "auto" => false},
        %{"id" => "open_lanes", "label" => "Open Additional Lanes", "auto" => false},
        %{"id" => "acknowledge", "label" => "Acknowledge", "auto" => false}
      ]
    }
  end

  defp build_incident_from_gate(event, data, max_occupancy) do
    gate_id = data["gate_id"] || 0

    %{
      type: "high_traffic",
      severity: "high",
      category: "operational",
      site: event.site,
      gate_id: gate_id,
      context: %{
        gate_id: gate_id,
        max_zone_occupancy: max_occupancy,
        total_crossings: get_in(data, ["exit_summary", "total_crossings"]) || 0,
        message: "Gate zone had #{max_occupancy} people during cycle (high congestion)"
      },
      suggested_actions: [
        %{"id" => "monitor", "label" => "Monitor Closely", "auto" => false},
        %{"id" => "open_lanes", "label" => "Open Additional Lanes", "auto" => false},
        %{"id" => "acknowledge", "label" => "Acknowledge", "auto" => false}
      ]
    }
  end
end
