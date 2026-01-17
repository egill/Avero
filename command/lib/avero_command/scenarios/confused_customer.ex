defmodule AveroCommand.Scenarios.ConfusedCustomer do
  @moduledoc """
  Scenario #28: Confused Customer

  Detects when a customer cycles in and out of the gate zone
  multiple times without successfully exiting, indicating:
  - Customer confusion about exit process
  - Payment/barcode issue
  - Gate not responding properly

  Trigger: Multiple zone entry/exit events without gate exit
  Severity: LOW
  """
  require Logger

  alias AveroCommand.Store

  # Number of zone cycles to trigger confusion alert
  @cycle_threshold 3
  # Time window to check (seconds)
  @time_window_seconds 120

  @doc """
  Evaluate if this event triggers the confused-customer scenario.
  """
  def evaluate(%{event_type: "sensors", data: %{"type" => type} = data} = event)
      when type in ["xovis.zone.entry", "xovis.zone.exit"] do
    zone = data["zone"] || ""
    person_id = data["person_id"]

    if gate_zone?(zone) && person_id do
      check_confusion_pattern(event, data, person_id)
    else
      :no_match
    end
  end

  def evaluate(_event), do: :no_match

  defp gate_zone?(zone) do
    zone_upper = String.upcase(zone)
    String.contains?(zone_upper, "GATE") || String.contains?(zone_upper, "EXIT")
  end

  defp check_confusion_pattern(event, data, person_id) do
    site = event.site
    since = DateTime.add(DateTime.utc_now(), -@time_window_seconds, :second)

    # Count zone entry/exit cycles for this person in gate area
    zone_events = get_gate_zone_events(site, person_id, since)
    cycle_count = count_cycles(zone_events)

    # Check if they've successfully exited
    has_exited = has_successful_exit?(site, person_id, since)

    if cycle_count >= @cycle_threshold && not has_exited do
      Logger.info(
        "ConfusedCustomer: person #{person_id} has #{cycle_count} gate zone cycles without exit"
      )

      {:match, build_incident(event, data, person_id, cycle_count)}
    else
      :no_match
    end
  end

  defp get_gate_zone_events(site, person_id, since) do
    Store.recent_events(200, site)
    |> Enum.filter(fn e ->
      e.event_type == "sensors" &&
        e.data["type"] in ["xovis.zone.entry", "xovis.zone.exit"] &&
        e.data["person_id"] == person_id &&
        gate_zone?(e.data["zone"] || "") &&
        DateTime.compare(e.time, since) == :gt
    end)
    |> Enum.sort_by(& &1.time, DateTime)
  rescue
    _ -> []
  end

  defp count_cycles(zone_events) do
    # Count entry/exit pairs as cycles
    entry_count =
      Enum.count(zone_events, &(&1.data["type"] == "xovis.zone.entry"))

    exit_count =
      Enum.count(zone_events, &(&1.data["type"] == "xovis.zone.exit"))

    min(entry_count, exit_count)
  end

  defp has_successful_exit?(site, person_id, since) do
    Store.recent_events(100, site)
    |> Enum.any?(fn e ->
      e.event_type == "exits" &&
        e.data["type"] == "exit.confirmed" &&
        e.data["person_id"] == person_id &&
        DateTime.compare(e.time, since) == :gt
    end)
  rescue
    _ -> false
  end

  defp build_incident(event, data, person_id, cycle_count) do
    gate_id = data["gate_id"] || 0
    zone = data["zone"] || "gate_zone"

    %{
      type: "confused_customer",
      severity: "low",
      category: "customer_experience",
      site: event.site,
      gate_id: gate_id,
      context: %{
        person_id: person_id,
        zone: zone,
        cycle_count: cycle_count,
        cycle_threshold: @cycle_threshold,
        time_window_seconds: @time_window_seconds,
        message:
          "Customer #{person_id} has cycled #{cycle_count} times in gate area without exiting"
      },
      suggested_actions: [
        %{"id" => "assist_customer", "label" => "Send Staff to Assist", "auto" => false},
        %{"id" => "check_payment", "label" => "Check Payment Status", "auto" => false},
        %{"id" => "open_gate", "label" => "Open Gate Manually", "auto" => false},
        %{"id" => "dismiss", "label" => "Dismiss", "auto" => false}
      ]
    }
  end
end
