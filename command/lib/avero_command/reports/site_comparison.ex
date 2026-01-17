defmodule AveroCommand.Reports.SiteComparison do
  @moduledoc """
  Report #41: Site Comparison Report

  Generates a daily comparison of activity across all sites:
  - Exit counts per site
  - Incidents per site
  - Relative performance
  - Anomalies compared to peer sites

  Trigger: Daily (end of day)
  Type: Info incident
  """
  require Logger

  alias AveroCommand.Store
  alias AveroCommand.Incidents
  alias AveroCommand.Incidents.Manager
  alias AveroCommand.Reports.SiteDiscovery

  @doc """
  Scheduled job to generate site comparison report.
  Called by Quantum scheduler daily.
  """
  def run do
    Logger.info("SiteComparison: generating cross-site comparison report")

    sites = get_all_sites()

    site_stats =
      sites
      |> Enum.map(&get_site_stats/1)
      |> Enum.reject(&(&1.event_count == 0))

    if length(site_stats) > 1 do
      create_comparison_report(site_stats)
    end

    :ok
  end

  defp get_all_sites do
    SiteDiscovery.list_recent_sites()
  end

  defp get_site_stats(site) do
    today_start = Date.utc_today() |> DateTime.new!(~T[00:00:00], "Etc/UTC")

    events =
      Store.recent_events(2000, site)
      |> Enum.filter(fn e ->
        DateTime.compare(e.time, today_start) == :gt
      end)

    exits =
      Enum.count(events, fn e ->
        e.event_type == "exits" && e.data["type"] == "exit.confirmed"
      end)

    gate_cycles =
      Enum.count(events, fn e ->
        e.event_type == "gates" && e.data["type"] == "gate.closed"
      end)

    payments = Enum.count(events, &payment_event?/1)

    incidents =
      Incidents.list_active()
      |> Enum.filter(fn inc ->
        inc.site == site &&
          DateTime.to_date(inc.created_at) == Date.utc_today()
      end)

    incident_count = length(incidents)
    high_severity = Enum.count(incidents, &(&1.severity in ["high", "critical"]))

    %{
      site: site,
      exits: exits,
      gate_cycles: gate_cycles,
      payments: payments,
      incidents: incident_count,
      high_severity_incidents: high_severity,
      event_count: length(events)
    }
  rescue
    _ ->
      %{
        site: site,
        exits: 0,
        gate_cycles: 0,
        payments: 0,
        incidents: 0,
        high_severity_incidents: 0,
        event_count: 0
      }
  end

  defp payment_event?(event) do
    (event.event_type == "payments" && event.data["type"] == "payment.received") ||
      (event.event_type == "people" && event.data["type"] == "person.payment.received")
  end

  defp create_comparison_report(site_stats) do
    # Calculate averages
    total_exits = Enum.sum(Enum.map(site_stats, & &1.exits))
    avg_exits = if length(site_stats) > 0, do: total_exits / length(site_stats), else: 0

    # Find best and worst performers
    sorted_by_exits = Enum.sort_by(site_stats, & &1.exits, :desc)
    top_site = List.first(sorted_by_exits)
    bottom_site = List.last(sorted_by_exits)

    # Find sites with high incident rates
    sites_with_issues = Enum.filter(site_stats, &(&1.high_severity_incidents > 0))

    # Create a summary incident for each site
    Enum.each(site_stats, fn stats ->
      create_site_incident(stats, avg_exits, length(site_stats))
    end)

    # Create overall comparison incident
    create_overall_incident(site_stats, avg_exits, top_site, bottom_site, sites_with_issues)
  end

  defp create_site_incident(stats, avg_exits, site_count) do
    performance =
      cond do
        stats.exits > avg_exits * 1.2 -> "above_average"
        stats.exits < avg_exits * 0.8 -> "below_average"
        true -> "average"
      end

    incident_attrs = %{
      type: "site_daily_stats",
      severity: "info",
      category: "business_intelligence",
      site: stats.site,
      gate_id: 0,
      context: %{
        site: stats.site,
        exits: stats.exits,
        gate_cycles: stats.gate_cycles,
        payments: stats.payments,
        incidents: stats.incidents,
        performance: performance,
        average_exits: round(avg_exits),
        sites_compared: site_count,
        message:
          "Site #{stats.site}: #{stats.exits} exits (#{performance} vs #{round(avg_exits)} avg), #{stats.incidents} incidents"
      },
      suggested_actions: [
        %{"id" => "view_details", "label" => "View Details", "auto" => false},
        %{"id" => "acknowledge", "label" => "Acknowledge", "auto" => true}
      ]
    }

    Manager.create_incident(incident_attrs)
  end

  defp create_overall_incident(site_stats, avg_exits, top_site, bottom_site, sites_with_issues) do
    incident_attrs = %{
      type: "site_comparison",
      severity: "info",
      category: "business_intelligence",
      site: "all",
      gate_id: 0,
      context: %{
        date: Date.utc_today(),
        site_count: length(site_stats),
        total_exits: Enum.sum(Enum.map(site_stats, & &1.exits)),
        average_exits: round(avg_exits),
        top_site: top_site && top_site.site,
        top_site_exits: top_site && top_site.exits,
        bottom_site: bottom_site && bottom_site.site,
        bottom_site_exits: bottom_site && bottom_site.exits,
        sites_with_issues: Enum.map(sites_with_issues, & &1.site),
        site_stats:
          Enum.map(site_stats, fn s -> %{site: s.site, exits: s.exits, incidents: s.incidents} end),
        message:
          "Cross-site comparison: #{length(site_stats)} sites, avg #{round(avg_exits)} exits. Top: #{top_site && top_site.site} (#{top_site && top_site.exits}), #{length(sites_with_issues)} sites with critical incidents"
      },
      suggested_actions: [
        %{"id" => "view_dashboard", "label" => "View Dashboard", "auto" => false},
        %{"id" => "export_report", "label" => "Export Report", "auto" => false},
        %{"id" => "acknowledge", "label" => "Acknowledge", "auto" => true}
      ]
    }

    Manager.create_incident(incident_attrs)
    Logger.info("SiteComparison: generated comparison for #{length(site_stats)} sites")
  end
end
