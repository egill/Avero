defmodule AveroCommand.Reports.DailySummary do
  @moduledoc """
  Report #34: Daily Summary

  Generates a comprehensive daily summary including:
  - Total exits and revenue
  - Peak hours
  - Gate utilization
  - Incident summary
  - Comparison to previous days

  Trigger: End of day (configurable)
  Type: Info incident
  """
  require Logger

  alias AveroCommand.Store
  alias AveroCommand.Incidents
  alias AveroCommand.Incidents.Manager

  @doc """
  Scheduled job to generate daily summary.
  Called by Quantum scheduler at end of day.
  """
  def run do
    Logger.info("DailySummary: generating daily summaries")

    sites = get_active_sites()

    Enum.each(sites, fn site ->
      generate_summary(site)
    end)

    :ok
  end

  defp get_active_sites do
    Store.recent_events(1000, nil)
    |> Enum.map(& &1.site)
    |> Enum.uniq()
    |> Enum.reject(&is_nil/1)
  rescue
    _ -> []
  end

  defp generate_summary(site) do
    # Query yesterday's data since this runs at midnight
    yesterday = Date.utc_today() |> Date.add(-1)
    yesterday_start = DateTime.new!(yesterday, ~T[00:00:00], "Etc/UTC")
    yesterday_end = DateTime.new!(Date.utc_today(), ~T[00:00:00], "Etc/UTC")
    events = get_events_in_range(site, yesterday_start, yesterday_end)

    # Calculate daily metrics
    stats = calculate_daily_stats(events)

    # Get incident counts
    incident_stats = get_incident_stats(site)

    # Create daily summary incident
    create_incident(site, stats, incident_stats)
  end

  defp get_events_in_range(site, from_time, to_time) do
    Store.get_events_in_range(site, from_time, to_time, 5000)
  rescue
    _ -> []
  end

  defp calculate_daily_stats(events) do
    exits = Enum.filter(events, fn e ->
      e.event_type == "exits" && e.data["type"] == "exit.confirmed"
    end)

    # Hourly breakdown
    hourly_exits =
      exits
      |> Enum.group_by(fn e -> e.time.hour end)
      |> Enum.map(fn {hour, events} -> {hour, length(events)} end)
      |> Map.new()

    peak_hour =
      hourly_exits
      |> Enum.max_by(fn {_, count} -> count end, fn -> {0, 0} end)
      |> elem(0)

    # Gate stats
    gate_cycles = Enum.count(events, fn e ->
      e.event_type == "gates" && e.data["type"] == "gate.closed"
    end)

    payments = Enum.count(events, fn e ->
      e.event_type == "payments" && e.data["type"] == "payment.received"
    end)

    barcodes = Enum.count(events, fn e ->
      e.event_type == "barcodes" && e.data["type"] == "barcode.validated"
    end)

    %{
      total_exits: length(exits),
      total_gate_cycles: gate_cycles,
      total_payments: payments,
      total_barcodes: barcodes,
      hourly_exits: hourly_exits,
      peak_hour: peak_hour,
      peak_exits: Map.get(hourly_exits, peak_hour, 0),
      event_count: length(events)
    }
  end

  defp get_incident_stats(site) do
    yesterday = Date.utc_today() |> Date.add(-1)

    incidents = Incidents.list_active()
    |> Enum.filter(fn inc ->
      inc.site == site &&
        DateTime.to_date(inc.created_at) == yesterday
    end)

    by_severity =
      incidents
      |> Enum.group_by(& &1.severity)
      |> Enum.map(fn {sev, incs} -> {sev, length(incs)} end)
      |> Map.new()

    by_type =
      incidents
      |> Enum.group_by(& &1.type)
      |> Enum.map(fn {type, incs} -> {type, length(incs)} end)
      |> Map.new()

    %{
      total: length(incidents),
      by_severity: by_severity,
      by_type: by_type,
      critical: Map.get(by_severity, "critical", 0),
      high: Map.get(by_severity, "high", 0),
      medium: Map.get(by_severity, "medium", 0),
      low: Map.get(by_severity, "low", 0)
    }
  rescue
    _ -> %{total: 0, by_severity: %{}, by_type: %{}, critical: 0, high: 0, medium: 0, low: 0}
  end

  defp create_incident(site, stats, incident_stats) do
    yesterday = Date.utc_today() |> Date.add(-1)

    incident_attrs = %{
      type: "daily_summary",
      severity: "info",
      category: "business_intelligence",
      site: site,
      gate_id: 0,
      context: %{
        date: yesterday,
        total_exits: stats.total_exits,
        total_gate_cycles: stats.total_gate_cycles,
        total_payments: stats.total_payments,
        total_barcodes: stats.total_barcodes,
        peak_hour: stats.peak_hour,
        peak_exits: stats.peak_exits,
        hourly_exits: stats.hourly_exits,
        event_count: stats.event_count,
        incidents: incident_stats,
        message: "Daily summary: #{stats.total_exits} exits, #{stats.total_payments} payments, peak at #{stats.peak_hour}:00 (#{stats.peak_exits} exits), #{incident_stats.total} incidents"
      },
      suggested_actions: [
        %{"id" => "view_dashboard", "label" => "View Dashboard", "auto" => false},
        %{"id" => "export_report", "label" => "Export Report", "auto" => false},
        %{"id" => "acknowledge", "label" => "Acknowledge", "auto" => true}
      ]
    }

    Manager.create_incident(incident_attrs)
    Logger.info("DailySummary: #{site} - #{stats.total_exits} exits, #{incident_stats.total} incidents")
  end
end
