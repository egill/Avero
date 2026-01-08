defmodule AveroCommand.Reports.TrafficAnomaly do
  @moduledoc """
  Report #35: Traffic Anomaly Detection

  Compares current traffic to historical averages and detects
  significant deviations that may indicate:
  - Unusual high traffic (event, promotion)
  - Unusual low traffic (weather, incident nearby)
  - System issues causing missed counts

  Trigger: Hourly comparison
  Type: Medium/Low severity based on deviation
  """
  require Logger

  alias AveroCommand.Store
  alias AveroCommand.Incidents.Manager

  # Deviation threshold to trigger alert (percentage)
  @high_deviation_threshold 50
  @low_deviation_threshold 30

  @doc """
  Scheduled job to check for traffic anomalies.
  Called by Quantum scheduler hourly.
  """
  def run do
    Logger.info("TrafficAnomaly: checking for traffic anomalies")

    sites = get_active_sites()

    Enum.each(sites, fn site ->
      check_anomaly(site)
    end)

    :ok
  end

  defp get_active_sites do
    Store.recent_events(500, nil)
    |> Enum.map(& &1.site)
    |> Enum.uniq()
    |> Enum.reject(&is_nil/1)
  rescue
    _ -> []
  end

  defp check_anomaly(site) do
    current_hour = DateTime.utc_now().hour
    current_dow = Date.utc_today() |> Date.day_of_week()

    # Get current hour's traffic
    current_traffic = get_hourly_traffic(site, 0)

    # Get historical average for this hour and day of week
    historical_avg = get_historical_average(site, current_hour, current_dow)

    if historical_avg > 0 do
      deviation = ((current_traffic - historical_avg) / historical_avg) * 100

      cond do
        deviation >= @high_deviation_threshold ->
          create_incident(site, :high_traffic, current_traffic, historical_avg, deviation)

        deviation <= -@low_deviation_threshold ->
          create_incident(site, :low_traffic, current_traffic, historical_avg, deviation)

        true ->
          :ok
      end
    end
  end

  defp get_hourly_traffic(site, hours_ago) do
    now = DateTime.utc_now()
    hour_start = now |> DateTime.add(-hours_ago * 3600, :second) |> Map.put(:minute, 0) |> Map.put(:second, 0)
    hour_end = DateTime.add(hour_start, 3600, :second)

    Store.recent_events(1000, site)
    |> Enum.count(fn e ->
      e.event_type == "exits" &&
        e.data["type"] == "exit.confirmed" &&
        DateTime.compare(e.time, hour_start) in [:gt, :eq] &&
        DateTime.compare(e.time, hour_end) == :lt
    end)
  rescue
    _ -> 0
  end

  defp get_historical_average(site, _hour, _dow) do
    # For now, use a simple average from the last few hours of the same day
    # In production, this would query historical data from the database
    recent_hours = for h <- 1..4, do: get_hourly_traffic(site, h * 24)

    if length(recent_hours) > 0 do
      Enum.sum(recent_hours) / length(recent_hours)
    else
      # Fallback to a reasonable default if no historical data
      10.0
    end
  end

  defp create_incident(site, anomaly_type, current, historical, deviation) do
    {severity, message} =
      case anomaly_type do
        :high_traffic ->
          {"low", "Traffic #{round(deviation)}% above normal: #{current} exits vs #{round(historical)} average"}

        :low_traffic ->
          {"medium", "Traffic #{round(abs(deviation))}% below normal: #{current} exits vs #{round(historical)} average"}
      end

    incident_attrs = %{
      type: "traffic_anomaly",
      severity: severity,
      category: "business_intelligence",
      site: site,
      gate_id: 0,
      context: %{
        anomaly_type: anomaly_type,
        current_traffic: current,
        historical_average: round(historical),
        deviation_percent: round(deviation),
        hour: DateTime.utc_now().hour,
        day_of_week: Date.utc_today() |> Date.day_of_week(),
        message: message
      },
      suggested_actions: [
        %{"id" => "investigate", "label" => "Investigate Cause", "auto" => false},
        %{"id" => "check_systems", "label" => "Check Systems", "auto" => false},
        %{"id" => "acknowledge", "label" => "Acknowledge", "auto" => false}
      ]
    }

    Manager.create_incident(incident_attrs)
    Logger.info("TrafficAnomaly: #{site} - #{anomaly_type} - #{round(deviation)}% deviation")
  end
end
