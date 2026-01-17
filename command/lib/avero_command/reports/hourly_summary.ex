defmodule AveroCommand.Reports.HourlySummary do
  @moduledoc """
  Report #33: Hourly Summary

  Generates an hourly summary of activity including:
  - Total exits
  - Gate utilization
  - Incidents generated
  - Average dwell time

  Trigger: Top of every hour
  Type: Info incident
  """
  require Logger

  alias AveroCommand.Store
  alias AveroCommand.Incidents.Manager
  alias AveroCommand.Reports.SiteDiscovery

  @doc """
  Scheduled job to generate hourly summary.
  Called by Quantum scheduler at the top of each hour.
  """
  def run do
    Logger.info("HourlySummary: generating hourly summaries")

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
    # Get events from the last hour
    one_hour_ago = DateTime.add(DateTime.utc_now(), -3600, :second)
    events = get_events_since(site, one_hour_ago)

    # Calculate metrics
    stats = calculate_stats(events)

    # Create info incident with summary
    create_incident(site, stats)
  end

  defp get_events_since(site, since) do
    Store.recent_events(1000, site)
    |> Enum.filter(fn e ->
      DateTime.compare(e.time, since) == :gt
    end)
  rescue
    _ -> []
  end

  defp calculate_stats(events) do
    exits =
      Enum.count(events, fn e ->
        e.event_type == "exits" && e.data["type"] == "exit.confirmed"
      end)

    gate_opens =
      Enum.count(events, fn e ->
        e.event_type == "gates" && e.data["type"] == "gate.opened"
      end)

    payments = Enum.count(events, &payment_event?/1)

    barcodes =
      Enum.count(events, fn e ->
        e.event_type == "barcodes" && e.data["type"] == "barcode.validated"
      end)

    # Get gate-specific stats
    gate_stats =
      events
      |> Enum.filter(fn e -> e.event_type == "gates" && e.data["type"] == "gate.closed" end)
      |> Enum.group_by(fn e -> e.data["gate_id"] end)
      |> Enum.map(fn {gate_id, gate_events} ->
        total_exits =
          gate_events
          |> Enum.map(fn e -> e.data["exit_summary"]["total_crossings"] || 0 end)
          |> Enum.sum()

        avg_duration =
          gate_events
          |> Enum.map(fn e -> e.data["open_duration_ms"] || 0 end)
          |> then(fn durations ->
            if length(durations) > 0, do: Enum.sum(durations) / length(durations), else: 0
          end)

        {gate_id,
         %{cycles: length(gate_events), exits: total_exits, avg_duration_ms: round(avg_duration)}}
      end)
      |> Map.new()

    %{
      total_exits: exits,
      total_gate_opens: gate_opens,
      total_payments: payments,
      total_barcodes: barcodes,
      gate_stats: gate_stats,
      event_count: length(events)
    }
  end

  defp payment_event?(event) do
    (event.event_type == "payments" && event.data["type"] == "payment.received") ||
      (event.event_type == "people" && event.data["type"] == "person.payment.received")
  end

  defp create_incident(site, stats) do
    hour = DateTime.utc_now() |> Map.put(:minute, 0) |> Map.put(:second, 0)

    incident_attrs = %{
      type: "hourly_summary",
      severity: "info",
      category: "business_intelligence",
      site: site,
      gate_id: 0,
      context: %{
        hour: hour,
        total_exits: stats.total_exits,
        total_gate_opens: stats.total_gate_opens,
        total_payments: stats.total_payments,
        total_barcodes: stats.total_barcodes,
        gate_stats: stats.gate_stats,
        event_count: stats.event_count,
        message:
          "Hourly summary: #{stats.total_exits} exits, #{stats.total_payments} payments, #{stats.total_gate_opens} gate cycles"
      },
      suggested_actions: [
        %{"id" => "view_dashboard", "label" => "View Dashboard", "auto" => false},
        %{"id" => "acknowledge", "label" => "Acknowledge", "auto" => true}
      ]
    }

    Manager.create_incident(incident_attrs)

    Logger.info(
      "HourlySummary: #{site} - #{stats.total_exits} exits, #{stats.total_payments} payments"
    )
  end
end
