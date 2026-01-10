defmodule AveroCommand.Reports.ShiftChange do
  @moduledoc """
  Report #36: Shift Change Summary

  Generates a summary at shift boundaries including:
  - Exits during the shift
  - Incidents that need handoff
  - Gate performance stats
  - Any ongoing issues

  Trigger: Configurable shift times
  Type: Info incident
  """
  require Logger

  alias AveroCommand.Store
  alias AveroCommand.Incidents
  alias AveroCommand.Incidents.Manager
  alias AveroCommand.Reports.SiteDiscovery

  # Default shift times (24h format, UTC)
  @default_shifts [
    %{name: "morning", start: 6, end: 14},
    %{name: "afternoon", start: 14, end: 22},
    %{name: "night", start: 22, end: 6}
  ]

  @doc """
  Scheduled job to generate shift change summary.
  Should be scheduled at each shift boundary.
  """
  def run do
    run(@default_shifts)
  end

  def run(shifts) do
    current_hour = DateTime.utc_now().hour

    # Find the shift that just ended
    ending_shift = Enum.find(shifts, fn shift ->
      shift.end == current_hour
    end)

    if ending_shift do
      Logger.info("ShiftChange: generating summary for #{ending_shift.name} shift")

      sites = get_active_sites()

      Enum.each(sites, fn site ->
        generate_shift_summary(site, ending_shift)
      end)
    end

    :ok
  end

  defp get_active_sites do
    SiteDiscovery.list_recent_sites()
  end

  defp generate_shift_summary(site, shift) do
    # Calculate shift duration in hours
    duration_hours =
      if shift.end > shift.start do
        shift.end - shift.start
      else
        24 - shift.start + shift.end
      end

    shift_start = DateTime.add(DateTime.utc_now(), -duration_hours * 3600, :second)

    # Get events during the shift
    events = get_events_since(site, shift_start)

    # Calculate stats
    stats = calculate_shift_stats(events)

    # Get open incidents that need handoff
    open_incidents = get_open_incidents(site)

    # Create shift summary incident
    create_incident(site, shift, stats, open_incidents)
  end

  defp get_events_since(site, since) do
    Store.recent_events(2000, site)
    |> Enum.filter(fn e ->
      DateTime.compare(e.time, since) == :gt
    end)
  rescue
    _ -> []
  end

  defp calculate_shift_stats(events) do
    exits = Enum.count(events, fn e ->
      e.event_type == "exits" && e.data["type"] == "exit.confirmed"
    end)

    gate_cycles = Enum.count(events, fn e ->
      e.event_type == "gates" && e.data["type"] == "gate.closed"
    end)

    payments = Enum.count(events, &payment_event?/1)

    %{
      total_exits: exits,
      total_gate_cycles: gate_cycles,
      total_payments: payments,
      event_count: length(events)
    }
  end

  defp payment_event?(event) do
    (event.event_type == "payments" && event.data["type"] == "payment.received") ||
      (event.event_type == "people" && event.data["type"] == "person.payment.received")
  end

  defp get_open_incidents(site) do
    Incidents.list_active()
    |> Enum.filter(fn inc -> inc.site == site end)
    |> Enum.map(fn inc ->
      %{
        id: inc.id,
        type: inc.type,
        severity: inc.severity,
        message: inc.context["message"] || "#{inc.type}"
      }
    end)
  rescue
    _ -> []
  end

  defp create_incident(site, shift, stats, open_incidents) do
    incident_attrs = %{
      type: "shift_change",
      severity: "info",
      category: "business_intelligence",
      site: site,
      gate_id: 0,
      context: %{
        shift_name: shift.name,
        shift_start: shift.start,
        shift_end: shift.end,
        total_exits: stats.total_exits,
        total_gate_cycles: stats.total_gate_cycles,
        total_payments: stats.total_payments,
        event_count: stats.event_count,
        open_incidents: open_incidents,
        open_incident_count: length(open_incidents),
        message: "#{String.capitalize(shift.name)} shift ended: #{stats.total_exits} exits, #{stats.total_payments} payments, #{length(open_incidents)} open incidents"
      },
      suggested_actions: [
        %{"id" => "review_incidents", "label" => "Review Open Incidents", "auto" => false},
        %{"id" => "acknowledge", "label" => "Acknowledge Handoff", "auto" => false}
      ]
    }

    Manager.create_incident(incident_attrs)
    Logger.info("ShiftChange: #{site} #{shift.name} shift - #{stats.total_exits} exits, #{length(open_incidents)} open incidents")
  end
end
