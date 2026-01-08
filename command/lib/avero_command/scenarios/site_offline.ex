defmodule AveroCommand.Scenarios.SiteOffline do
  @moduledoc """
  Scenario #39: Site Offline Detection

  Detects when no events have been received from a site
  for an extended period, indicating:
  - Network connectivity issues
  - Gateway offline
  - Complete system failure

  Trigger: Periodic check (every 5 minutes)
  Severity: CRITICAL
  """
  require Logger

  alias AveroCommand.Store
  alias AveroCommand.Incidents
  alias AveroCommand.Incidents.Manager

  # Time without events to consider site offline (minutes)
  @offline_threshold_minutes 10

  @doc """
  Scheduled job to check for offline sites.
  Called by Quantum scheduler periodically.
  """
  def run do
    Logger.debug("SiteOffline: checking for offline sites")

    # Get all known sites from today's events
    known_sites = get_known_sites()

    Enum.each(known_sites, fn site ->
      check_site_status(site)
    end)

    :ok
  end

  defp get_known_sites do
    today_start = Date.utc_today() |> DateTime.new!(~T[00:00:00], "Etc/UTC")

    Store.recent_events(2000, nil)
    |> Enum.filter(fn e ->
      DateTime.compare(e.time, today_start) == :gt
    end)
    |> Enum.map(& &1.site)
    |> Enum.uniq()
    |> Enum.reject(&is_nil/1)
  rescue
    _ -> []
  end

  defp check_site_status(site) do
    last_event_time = get_last_event_time(site)

    if last_event_time do
      offline_minutes = DateTime.diff(DateTime.utc_now(), last_event_time, :second) / 60

      if offline_minutes >= @offline_threshold_minutes do
        maybe_create_incident(site, offline_minutes, last_event_time)
      end
    end
  end

  defp get_last_event_time(site) do
    Store.recent_events(100, site)
    |> Enum.max_by(& &1.time, DateTime, fn -> nil end)
    |> case do
      nil -> nil
      event -> event.time
    end
  rescue
    _ -> nil
  end

  defp maybe_create_incident(site, offline_minutes, last_event_time) do
    # Check if we already have an active offline incident for this site
    if has_active_offline_incident?(site) do
      :ok
    else
      create_incident(site, offline_minutes, last_event_time)
    end
  end

  defp has_active_offline_incident?(site) do
    Incidents.list_active()
    |> Enum.any?(fn inc ->
      inc.type == "site_offline" && inc.site == site
    end)
  rescue
    _ -> false
  end

  defp create_incident(site, offline_minutes, last_event_time) do
    incident_attrs = %{
      type: "site_offline",
      severity: "critical",
      category: "equipment",
      site: site,
      gate_id: 0,
      context: %{
        offline_minutes: round(offline_minutes),
        last_event_time: last_event_time,
        threshold_minutes: @offline_threshold_minutes,
        message: "Site #{site} has been offline for #{round(offline_minutes)} minutes (last event: #{Calendar.strftime(last_event_time, "%H:%M:%S")})"
      },
      suggested_actions: [
        %{"id" => "check_network", "label" => "Check Network", "auto" => false},
        %{"id" => "check_gateway", "label" => "Check Gateway", "auto" => false},
        %{"id" => "contact_site", "label" => "Contact Site", "auto" => true},
        %{"id" => "acknowledge", "label" => "Acknowledge", "auto" => false}
      ]
    }

    Manager.create_incident(incident_attrs)
    Logger.error("SiteOffline: #{site} has been offline for #{round(offline_minutes)} minutes")
  end
end
