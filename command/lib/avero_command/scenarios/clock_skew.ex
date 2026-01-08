defmodule AveroCommand.Scenarios.ClockSkew do
  @moduledoc """
  Scenario #20: Clock Skew Detected

  Detects when a sensor's timestamp differs significantly from
  the gateway time, which can cause event ordering issues.

  Trigger: sensor.status event with timestamp skew
  Severity: MEDIUM
  """
  require Logger

  # Maximum acceptable skew in seconds
  @max_skew_seconds 5

  @doc """
  Evaluate if this event triggers the clock-skew scenario.
  """
  def evaluate(%{event_type: "sensors", data: %{"type" => "sensor.status"} = data, time: event_time} = event) do
    sensor_timestamp = parse_sensor_timestamp(data)

    if sensor_timestamp do
      skew_seconds = abs(DateTime.diff(event_time, sensor_timestamp, :second))

      if skew_seconds > @max_skew_seconds do
        Logger.warning("ClockSkew: sensor #{data["sensor_id"]} has #{skew_seconds}s clock skew")
        {:match, build_incident(event, data, sensor_timestamp, skew_seconds)}
      else
        :no_match
      end
    else
      :no_match
    end
  end

  def evaluate(_event), do: :no_match

  defp parse_sensor_timestamp(data) do
    timestamp = data["sensor_timestamp"] || data["device_timestamp"]

    case timestamp do
      nil -> nil
      ts when is_binary(ts) ->
        case DateTime.from_iso8601(ts) do
          {:ok, dt, _} -> dt
          _ -> nil
        end
      ts when is_integer(ts) ->
        case DateTime.from_unix(ts) do
          {:ok, dt} -> dt
          _ -> nil
        end
      _ -> nil
    end
  end

  defp build_incident(event, data, sensor_timestamp, skew_seconds) do
    sensor_id = data["sensor_id"] || "unknown"

    %{
      type: "clock_skew",
      severity: "medium",
      category: "equipment",
      site: event.site,
      gate_id: 0,
      context: %{
        sensor_id: sensor_id,
        sensor_timestamp: sensor_timestamp,
        gateway_timestamp: event.time,
        skew_seconds: skew_seconds,
        max_allowed_seconds: @max_skew_seconds,
        message: "Sensor #{sensor_id} clock is #{skew_seconds}s off from gateway (max: #{@max_skew_seconds}s)"
      },
      suggested_actions: [
        %{"id" => "sync_time", "label" => "Sync Sensor Time", "auto" => false},
        %{"id" => "check_ntp", "label" => "Check NTP Config", "auto" => false},
        %{"id" => "acknowledge", "label" => "Acknowledge", "auto" => false}
      ]
    }
  end
end
