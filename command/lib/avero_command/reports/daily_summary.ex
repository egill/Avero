defmodule AveroCommand.Reports.DailySummary do
  @moduledoc """
  Report #34: Daily Summary

  Generates a brief, factual daily summary including:
  - Traffic metrics with day-over-day comparison
  - Gate performance with min/max/avg timing
  - Issues to explore (gate faults, long openings, tailgating)
  - Incident summary

  Trigger: End of day (midnight UTC)
  Type: Info incident
  """
  require Logger

  alias AveroCommand.Store
  alias AveroCommand.Journeys
  alias AveroCommand.Incidents
  alias AveroCommand.Incidents.Manager
  alias AveroCommand.Reports.SiteDiscovery

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
    SiteDiscovery.list_recent_sites()
  end

  defp generate_summary(site) do
    # Query yesterday's data since this runs at midnight
    yesterday = Date.utc_today() |> Date.add(-1)
    day_before = Date.utc_today() |> Date.add(-2)

    # Collect data for both days
    today_journeys = Journeys.get_daily_stats(site, yesterday)
    prev_journeys = Journeys.get_daily_stats(site, day_before)

    today_gates = Store.get_gate_timing_stats(site, yesterday)
    prev_gates = Store.get_gate_timing_stats(site, day_before)

    today_incidents = Incidents.get_daily_incident_stats(site, yesterday)

    # Build sections
    traffic = build_traffic_section(today_journeys, prev_journeys)
    gates = build_gates_section(today_gates, prev_gates)
    issues = build_issues_section(today_gates, today_incidents, today_journeys)
    incidents = build_incidents_section(today_incidents)

    # Generate brief message
    message = generate_message(site, yesterday, traffic, gates, issues, incidents)

    # Create incident
    create_incident(site, yesterday, traffic, gates, issues, incidents, message)
  end

  # ============================================
  # Section Builders
  # ============================================

  defp build_traffic_section(today, prev) do
    total = today.total_exits
    yesterday_total = prev.total_exits

    change_pct =
      if yesterday_total > 0 do
        Float.round((total - yesterday_total) / yesterday_total * 100, 1)
      else
        0.0
      end

    paid_pct = if total > 0, do: Float.round(today.paid_exits / total * 100, 1), else: 0.0
    unpaid_pct = if total > 0, do: Float.round(today.unpaid_exits / total * 100, 1), else: 0.0

    %{
      total: total,
      yesterday: yesterday_total,
      change_pct: change_pct,
      paid: today.paid_exits,
      paid_pct: paid_pct,
      unpaid: today.unpaid_exits,
      unpaid_pct: unpaid_pct,
      lost: today.tracking_lost,
      peak_hour: today.peak_hour,
      peak_count: today.peak_count,
      hourly_exits: today.hourly_exits
    }
  end

  defp build_gates_section(today, prev) do
    %{
      total_cycles: today.total_cycles,
      min_ms: today.min_ms,
      max_ms: today.max_ms,
      avg_ms: today.avg_ms,
      yesterday_avg_ms: prev.avg_ms,
      long_openings: today.long_openings,
      by_gate: today.by_gate
    }
  end

  defp build_issues_section(gates, incidents, journeys) do
    # ACC mismatch: journeys with ACC match but we want to track if there's a mismatch
    # For now, we just highlight if there are tailgating incidents
    %{
      gate_faults: incidents.gate_faults,
      long_openings: gates.long_openings,
      tailgating: incidents.tailgating,
      tailgated_journeys: journeys.tailgated_count
    }
  end

  defp build_incidents_section(incidents) do
    %{
      total: incidents.total,
      high: incidents.high,
      medium: incidents.medium,
      info: incidents.info,
      top_types: incidents.top_types,
      by_type: incidents.by_type
    }
  end

  # ============================================
  # Message Generation
  # ============================================

  defp generate_message(site, date, traffic, gates, issues, incidents) do
    date_str = Calendar.strftime(date, "%b %d")

    # Traffic summary
    change_str = format_change(traffic.change_pct)

    traffic_part = "#{traffic.total} exits#{change_str}"

    # Gates summary
    gates_part =
      if gates.total_cycles > 0 do
        ", #{gates.total_cycles} gate cycles (avg #{format_duration(gates.avg_ms)})"
      else
        ""
      end

    # Issues part - only mention if there are issues
    issues_parts = []

    issues_parts =
      if issues.gate_faults > 0 do
        issues_parts ++ ["#{issues.gate_faults} gate fault#{plural(issues.gate_faults)}"]
      else
        issues_parts
      end

    issues_parts =
      if issues.long_openings > 0 do
        issues_parts ++ ["#{issues.long_openings} long opening#{plural(issues.long_openings)}"]
      else
        issues_parts
      end

    issues_parts =
      if issues.tailgating > 0 do
        issues_parts ++ ["#{issues.tailgating} tailgating incident#{plural(issues.tailgating)}"]
      else
        issues_parts
      end

    issues_str =
      if length(issues_parts) > 0 do
        ". " <> Enum.join(issues_parts, ", ")
      else
        ""
      end

    # Incidents summary
    incidents_str =
      if incidents.total > 0 do
        severity_parts =
          [
            if(incidents.high > 0, do: "#{incidents.high} high"),
            if(incidents.medium > 0, do: "#{incidents.medium} medium")
          ]
          |> Enum.reject(&is_nil/1)

        severity_str =
          if length(severity_parts) > 0 do
            " (#{Enum.join(severity_parts, ", ")})"
          else
            ""
          end

        ". #{incidents.total} incident#{plural(incidents.total)}#{severity_str}"
      else
        ""
      end

    "#{format_site(site)} #{date_str}: #{traffic_part}#{gates_part}#{issues_str}#{incidents_str}"
  end

  defp format_change(pct) when pct > 0, do: " (up #{abs(pct)}%)"
  defp format_change(pct) when pct < 0, do: " (down #{abs(pct)}%)"
  defp format_change(_), do: ""

  defp format_duration(ms) when ms >= 1000, do: "#{Float.round(ms / 1000, 1)}s"
  defp format_duration(ms), do: "#{ms}ms"

  defp format_site(site), do: site |> String.split("-") |> List.first() |> String.capitalize()

  defp plural(1), do: ""
  defp plural(_), do: "s"

  # ============================================
  # Incident Creation
  # ============================================

  defp create_incident(site, date, traffic, gates, issues, incidents, message) do
    incident_attrs = %{
      type: "daily_summary",
      severity: "info",
      category: "business_intelligence",
      site: site,
      gate_id: 0,
      context: %{
        date: Date.to_iso8601(date),
        message: message,
        traffic: traffic,
        gates: gates,
        issues: issues,
        incidents: incidents
      },
      suggested_actions: [
        %{"id" => "view_dashboard", "label" => "View Dashboard", "auto" => false},
        %{"id" => "acknowledge", "label" => "Acknowledge", "auto" => true}
      ]
    }

    Manager.create_incident(incident_attrs)
    Logger.info("DailySummary: #{message}")
  end
end
