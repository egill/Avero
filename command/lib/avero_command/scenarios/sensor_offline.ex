defmodule AveroCommand.Scenarios.SensorOffline do
  @moduledoc """
  Scenario #9: Sensor Offline

  Detects when a Xovis sensor stops sending status updates,
  indicating network or hardware issues.

  Trigger: sensor.status events stop arriving for threshold time
  Severity: CRITICAL (no tracking capability)

  This scenario is evaluated by tracking the last sensor.status event
  and checking if it exceeds the offline threshold.
  """
  require Logger

  alias AveroCommand.Store

  # Time threshold to consider sensor offline (in seconds)
  # 2 minutes without status
  @offline_threshold_seconds 120

  @doc """
  Evaluate if this event triggers the sensor-offline scenario.
  Event comes through as event_type: "sensors" with data: %{"type" => "sensor.status", ...}

  This scenario works differently - it checks if we haven't seen a
  sensor status recently when we receive ANY event from that site.
  """
  def evaluate(%{event_type: "sensors", data: %{"type" => "sensor.status"}} = _event) do
    # Sensor sent status - it's online, no incident
    :no_match
  end

  def evaluate(%{site: site} = _event) when is_binary(site) do
    # On any event, check if sensors are still reporting
    check_sensor_status(site)
  end

  def evaluate(_event), do: :no_match

  defp check_sensor_status(site) do
    threshold = DateTime.add(DateTime.utc_now(), -@offline_threshold_seconds, :second)

    # Get recent sensor status events
    recent_status = get_recent_sensor_status(site, threshold)

    if Enum.empty?(recent_status) do
      # Check if we've EVER seen a sensor status to avoid false positives on startup
      all_status =
        get_recent_sensor_status(site, DateTime.add(DateTime.utc_now(), -3600, :second))

      if Enum.empty?(all_status) do
        # Never seen sensor status - don't alert (might be startup)
        :no_match
      else
        # Had sensor status before but not recently - sensor is offline
        last_seen = List.first(all_status)
        {:match, build_incident(site, last_seen)}
      end
    else
      :no_match
    end
  end

  defp get_recent_sensor_status(site, since) do
    Store.recent_events(100, site)
    |> Enum.filter(fn e ->
      e.event_type == "sensors" &&
        e.data["type"] == "sensor.status" &&
        DateTime.compare(e.time, since) == :gt
    end)
    |> Enum.sort_by(& &1.time, {:desc, DateTime})
  rescue
    _ -> []
  end

  defp build_incident(site, last_status_event) do
    sensor_id = last_status_event.data["sensor_id"] || "unknown"
    last_seen = last_status_event.time
    offline_duration = DateTime.diff(DateTime.utc_now(), last_seen, :second)

    %{
      type: "sensor_offline",
      severity: "critical",
      category: "equipment",
      site: site,
      gate_id: 0,
      context: %{
        sensor_id: sensor_id,
        last_seen: last_seen,
        offline_duration_seconds: offline_duration,
        threshold_seconds: @offline_threshold_seconds,
        message: "Sensor #{sensor_id} offline for #{offline_duration}s (last seen: #{last_seen})"
      },
      suggested_actions: [
        %{"id" => "check_sensor_power", "label" => "Check Sensor Power", "auto" => false},
        %{"id" => "check_network", "label" => "Check Network Connection", "auto" => false},
        %{"id" => "restart_sensor", "label" => "Restart Sensor", "auto" => false},
        %{"id" => "notify_maintenance", "label" => "Notify Maintenance", "auto" => true}
      ]
    }
  end
end
