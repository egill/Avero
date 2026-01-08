defmodule AveroCommand.Scenarios.ExitLoitering do
  @moduledoc """
  Scenario #9: Exit Line Loitering

  Detects when a person stays in the exit/gate zone for an extended
  period without crossing the exit line, indicating possible confusion,
  obstruction, or suspicious behavior.

  Trigger: xovis.zone.exit from EXIT/GATE zone with long dwell time
  Severity: INFO
  """
  require Logger

  # Loitering threshold in milliseconds (60 seconds)
  @loiter_threshold_ms 60_000

  @doc """
  Evaluate if this event triggers the exit-loitering scenario.
  """
  def evaluate(%{event_type: "sensors", data: %{"type" => "xovis.zone.exit"} = data} = event) do
    zone = data["zone"] || ""
    time_in_zone_ms = data["time_in_zone_ms"] || data["dwell_ms"] || 0

    if exit_zone?(zone) && time_in_zone_ms > @loiter_threshold_ms do
      Logger.info("ExitLoitering: person spent #{time_in_zone_ms}ms in #{zone}")
      {:match, build_incident(event, data, zone, time_in_zone_ms)}
    else
      :no_match
    end
  end

  def evaluate(_event), do: :no_match

  defp exit_zone?(zone) do
    zone_upper = String.upcase(zone)
    String.contains?(zone_upper, "EXIT") or String.contains?(zone_upper, "GATE")
  end

  defp build_incident(event, data, zone, time_in_zone_ms) do
    person_id = event.person_id || data["person_id"] || 0
    gate_id = data["gate_id"] || 0
    time_seconds = div(time_in_zone_ms, 1000)

    %{
      type: "exit_loitering",
      severity: "info",
      category: "loss_prevention",
      site: event.site,
      gate_id: gate_id,
      related_person_id: person_id,
      context: %{
        person_id: person_id,
        gate_id: gate_id,
        zone: zone,
        time_in_zone_ms: time_in_zone_ms,
        time_in_zone_seconds: time_seconds,
        threshold_seconds: div(@loiter_threshold_ms, 1000),
        message: "Person loitered in #{zone} for #{time_seconds}s without exiting"
      },
      suggested_actions: [
        %{"id" => "check_customer", "label" => "Check on Customer", "auto" => false},
        %{"id" => "review_camera", "label" => "Review Camera", "auto" => false},
        %{"id" => "dismiss", "label" => "Dismiss", "auto" => false}
      ]
    }
  end
end
