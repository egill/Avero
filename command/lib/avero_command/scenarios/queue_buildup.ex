defmodule AveroCommand.Scenarios.QueueBuildup do
  @moduledoc """
  Scenario #29: Queue Buildup at Gate

  Detects when multiple people are waiting at the gate zone
  while the gate is not cycling fast enough, indicating:
  - Gate processing too slow
  - Payment system delays
  - Equipment issue causing customer backup

  Trigger: Zone occupancy count or periodic check
  Severity: MEDIUM
  """
  require Logger

  alias AveroCommand.Store

  # Number of people to consider a "queue"
  @queue_threshold 3
  # Minimum gate cycles per minute expected when there's a queue
  @min_cycles_per_minute 2
  # Time window to check gate cycles (seconds)
  @cycle_check_window_seconds 60

  @doc """
  Evaluate if this event triggers the queue-buildup scenario.
  """
  def evaluate(%{event_type: "sensors", data: %{"type" => "xovis.zone.count"} = data} = event) do
    zone = data["zone"] || ""
    count = data["count"] || data["occupancy"] || 0

    if gate_zone?(zone) && count >= @queue_threshold do
      check_queue_buildup(event, data, zone, count)
    else
      :no_match
    end
  end

  # Also check on gate.closed to see if queue is backing up
  def evaluate(%{event_type: "gates", data: %{"type" => "gate.closed"} = data} = event) do
    gate_id = data["gate_id"]
    site = event.site

    # Get current zone occupancy if available
    zone_count = get_gate_zone_occupancy(site, gate_id)

    if zone_count >= @queue_threshold do
      check_queue_buildup(event, data, "gate_#{gate_id}", zone_count)
    else
      :no_match
    end
  end

  def evaluate(_event), do: :no_match

  defp gate_zone?(zone) do
    zone_upper = String.upcase(zone)
    String.contains?(zone_upper, "GATE") || String.contains?(zone_upper, "EXIT")
  end

  defp check_queue_buildup(event, data, zone, queue_size) do
    site = event.site
    gate_id = data["gate_id"] || extract_gate_from_zone(zone)
    since = DateTime.add(DateTime.utc_now(), -@cycle_check_window_seconds, :second)

    # Count gate cycles in the window
    gate_cycles = count_gate_cycles(site, gate_id, since)

    # If queue is building but gates aren't cycling fast enough
    if gate_cycles < @min_cycles_per_minute do
      Logger.warning("QueueBuildup: #{queue_size} people queued at #{zone}, only #{gate_cycles} gate cycles in last minute")
      {:match, build_incident(event, data, zone, queue_size, gate_cycles, gate_id)}
    else
      :no_match
    end
  end

  defp get_gate_zone_occupancy(site, gate_id) do
    # Look for recent zone count events for this gate's zone
    Store.recent_events(50, site)
    |> Enum.find_value(0, fn e ->
      if e.event_type == "sensors" &&
           e.data["type"] == "xovis.zone.count" &&
           gate_matches?(e.data["zone"], gate_id) do
        e.data["count"] || e.data["occupancy"] || 0
      end
    end)
  rescue
    _ -> 0
  end

  defp gate_matches?(zone, gate_id) when is_binary(zone) and is_integer(gate_id) do
    zone_upper = String.upcase(zone)
    String.contains?(zone_upper, "GATE") &&
      String.contains?(zone_upper, Integer.to_string(gate_id))
  end

  defp gate_matches?(zone, _gate_id) when is_binary(zone) do
    String.contains?(String.upcase(zone), "GATE")
  end

  defp gate_matches?(_, _), do: false

  defp extract_gate_from_zone(zone) do
    case Regex.run(~r/(\d+)/, zone) do
      [_, num] -> String.to_integer(num)
      _ -> 0
    end
  end

  defp count_gate_cycles(site, gate_id, since) do
    Store.recent_events(100, site)
    |> Enum.count(fn e ->
      e.event_type == "gates" &&
        e.data["type"] == "gate.closed" &&
        (gate_id == 0 || e.data["gate_id"] == gate_id) &&
        DateTime.compare(e.time, since) == :gt
    end)
  rescue
    _ -> 0
  end

  defp build_incident(event, _data, zone, queue_size, gate_cycles, gate_id) do
    %{
      type: "queue_buildup",
      severity: "medium",
      category: "customer_experience",
      site: event.site,
      gate_id: gate_id,
      context: %{
        zone: zone,
        queue_size: queue_size,
        queue_threshold: @queue_threshold,
        gate_cycles: gate_cycles,
        expected_min_cycles: @min_cycles_per_minute,
        check_window_seconds: @cycle_check_window_seconds,
        message: "#{queue_size} people queued at #{zone}, only #{gate_cycles} gate cycles in last #{@cycle_check_window_seconds}s"
      },
      suggested_actions: [
        %{"id" => "open_additional_gate", "label" => "Open Additional Gate", "auto" => false},
        %{"id" => "check_gate", "label" => "Check Gate Status", "auto" => false},
        %{"id" => "send_staff", "label" => "Send Staff to Assist", "auto" => false},
        %{"id" => "acknowledge", "label" => "Acknowledge", "auto" => false}
      ]
    }
  end
end
