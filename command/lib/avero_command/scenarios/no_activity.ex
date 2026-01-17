defmodule AveroCommand.Scenarios.NoActivity do
  @moduledoc """
  Scenario #10: No Activity Alert

  Detects when a site has no events for an extended period,
  indicating potential system issues or complete shutdown.

  Trigger: system.metrics event when event rate is zero
  Severity: HIGH after 5 minutes, CRITICAL after 15 minutes
  """
  require Logger

  alias AveroCommand.Store

  # Thresholds in seconds
  # 5 minutes
  @warning_threshold_seconds 300
  # 15 minutes
  @critical_threshold_seconds 900

  @doc """
  Evaluate if this event triggers the no-activity scenario.
  We check system.metrics events which include event counts.
  """
  def evaluate(%{event_type: "system.metrics", data: data, site: site} = event) do
    # Check if event rate is zero or very low
    events_processed = data["events_processed"] || data["event_count"] || 0
    events_per_second = data["events_per_second"] || 0

    if events_per_second == 0 && events_processed == 0 do
      check_inactivity(event, site)
    else
      :no_match
    end
  end

  def evaluate(_event), do: :no_match

  defp check_inactivity(event, site) do
    # Get last non-metrics event
    last_activity = get_last_activity(site)

    case last_activity do
      nil ->
        # No activity ever recorded
        :no_match

      activity_time ->
        inactivity_seconds = DateTime.diff(DateTime.utc_now(), activity_time, :second)

        cond do
          inactivity_seconds >= @critical_threshold_seconds ->
            {:match, build_incident(event, site, inactivity_seconds, "critical")}

          inactivity_seconds >= @warning_threshold_seconds ->
            {:match, build_incident(event, site, inactivity_seconds, "high")}

          true ->
            :no_match
        end
    end
  end

  defp get_last_activity(site) do
    # Get recent events excluding system.metrics
    Store.recent_events(50, site)
    |> Enum.filter(fn e ->
      e.event_type not in ["system.metrics", "sensors"]
    end)
    |> Enum.sort_by(& &1.time, {:desc, DateTime})
    |> List.first()
    |> case do
      nil -> nil
      e -> e.time
    end
  rescue
    _ -> nil
  end

  defp build_incident(_event, site, inactivity_seconds, severity) do
    inactivity_minutes = div(inactivity_seconds, 60)

    %{
      type: "no_activity",
      severity: severity,
      category: "operational",
      site: site,
      gate_id: 0,
      context: %{
        inactivity_seconds: inactivity_seconds,
        inactivity_minutes: inactivity_minutes,
        threshold_minutes:
          div(
            if(severity == "critical",
              do: @critical_threshold_seconds,
              else: @warning_threshold_seconds
            ),
            60
          ),
        message: "No activity for #{inactivity_minutes} minutes"
      },
      suggested_actions: [
        %{"id" => "check_sensors", "label" => "Check Sensor Connections", "auto" => false},
        %{"id" => "check_gateway", "label" => "Check Gateway Status", "auto" => false},
        %{"id" => "verify_store_status", "label" => "Verify Store Status", "auto" => false},
        %{"id" => "acknowledge", "label" => "Acknowledge (Store Closed)", "auto" => false}
      ]
    }
  end
end
