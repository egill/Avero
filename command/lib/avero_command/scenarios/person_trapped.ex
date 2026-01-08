defmodule AveroCommand.Scenarios.PersonTrapped do
  @moduledoc """
  Scenario #23: Person Trapped in Gate Area

  Detects when a paid/authorized person has been in the gate zone
  for an extended time without the gate opening, indicating:
  - Gate malfunction
  - Authorization not reaching gate
  - Person unable to trigger exit

  Trigger: Periodic check or zone events
  Severity: HIGH (safety issue)
  """
  require Logger

  alias AveroCommand.Store

  # Time threshold for being "trapped" (seconds)
  @trapped_threshold_seconds 30

  @doc """
  Evaluate if this event triggers the person-trapped scenario.
  Check when someone enters the gate zone with authorization.
  """
  def evaluate(%{event_type: "sensors", data: %{"type" => "xovis.zone.entry"} = data} = event) do
    zone = data["zone"] || ""

    if gate_zone?(zone) do
      check_for_trapped_person(event, data, zone)
    else
      :no_match
    end
  end

  # Also check on zone dwell updates
  def evaluate(%{event_type: "sensors", data: %{"type" => "xovis.zone.dwell"} = data} = event) do
    zone = data["zone"] || ""
    dwell_time_ms = data["dwell_time_ms"] || data["time_in_zone_ms"] || 0

    if gate_zone?(zone) && dwell_time_ms >= @trapped_threshold_seconds * 1000 do
      check_for_trapped_person(event, data, zone)
    else
      :no_match
    end
  end

  # Check when person state indicates authorized but still at gate
  # NOTE: Person state events come from "people" topic, not "tracker"
  def evaluate(%{event_type: "people", data: %{"type" => "person.state.changed"} = data} = event) do
    current_state = data["current_state"] || data["state"]
    person_id = data["person_id"]
    authorized = data["authorized"] || false

    # If person is at_gate with authorization for too long
    if current_state == "at_gate" && authorized && person_id do
      check_authorized_person_stuck(event, data, person_id)
    else
      :no_match
    end
  end

  def evaluate(_event), do: :no_match

  defp gate_zone?(zone) do
    zone_upper = String.upcase(zone)
    String.contains?(zone_upper, "GATE") || String.contains?(zone_upper, "EXIT")
  end

  defp check_for_trapped_person(event, data, zone) do
    person_id = data["person_id"]
    site = event.site

    if person_id do
      # Check if this person has authorization
      if person_is_authorized?(site, person_id) do
        # Check if gate has opened recently for this person
        unless gate_opened_for_person?(site, person_id) do
          dwell_time_ms = data["dwell_time_ms"] || data["time_in_zone_ms"] || 0

          if dwell_time_ms >= @trapped_threshold_seconds * 1000 do
            Logger.error("PersonTrapped: person #{person_id} authorized but trapped in #{zone} for #{div(dwell_time_ms, 1000)}s")
            {:match, build_incident(event, data, zone, person_id, dwell_time_ms)}
          else
            :no_match
          end
        else
          :no_match
        end
      else
        :no_match
      end
    else
      :no_match
    end
  end

  defp check_authorized_person_stuck(event, data, person_id) do
    site = event.site
    since = DateTime.add(DateTime.utc_now(), -@trapped_threshold_seconds, :second)

    # Check when they became authorized/at_gate
    state_time = get_at_gate_time(site, person_id)

    if state_time && DateTime.compare(state_time, since) == :lt do
      # They've been at_gate for longer than threshold
      unless gate_opened_for_person?(site, person_id) do
        elapsed_seconds = DateTime.diff(DateTime.utc_now(), state_time, :second)
        Logger.error("PersonTrapped: authorized person #{person_id} stuck at gate for #{elapsed_seconds}s")
        {:match, build_incident(event, data, "gate_zone", person_id, elapsed_seconds * 1000)}
      else
        :no_match
      end
    else
      :no_match
    end
  end

  defp person_is_authorized?(site, person_id) do
    Store.recent_events(50, site)
    |> Enum.any?(fn e ->
      (e.event_type == "tracker" &&
         e.data["type"] == "person.authorized" &&
         e.data["person_id"] == person_id) ||
        (e.event_type == "payments" &&
           e.data["person_id"] == person_id)
    end)
  rescue
    _ -> false
  end

  defp gate_opened_for_person?(site, person_id) do
    since = DateTime.add(DateTime.utc_now(), -@trapped_threshold_seconds, :second)

    Store.recent_events(50, site)
    |> Enum.any?(fn e ->
      e.event_type == "gates" &&
        e.data["type"] == "gate.opened" &&
        e.data["person_id"] == person_id &&
        DateTime.compare(e.time, since) == :gt
    end)
  rescue
    _ -> false
  end

  defp get_at_gate_time(site, person_id) do
    Store.recent_events(100, site)
    |> Enum.find_value(fn e ->
      if e.event_type == "tracker" &&
           e.data["type"] == "person.state.changed" &&
           e.data["person_id"] == person_id &&
           e.data["current_state"] == "at_gate" do
        e.time
      end
    end)
  rescue
    _ -> nil
  end

  defp build_incident(event, data, zone, person_id, dwell_time_ms) do
    gate_id = data["gate_id"] || 0
    dwell_seconds = div(dwell_time_ms, 1000)

    %{
      type: "person_trapped",
      severity: "high",
      category: "safety",
      site: event.site,
      gate_id: gate_id,
      context: %{
        person_id: person_id,
        zone: zone,
        dwell_time_seconds: dwell_seconds,
        threshold_seconds: @trapped_threshold_seconds,
        message: "Authorized person #{person_id} trapped in #{zone} for #{dwell_seconds}s - gate not opening"
      },
      suggested_actions: [
        %{"id" => "open_gate", "label" => "Open Gate Manually", "auto" => false},
        %{"id" => "check_gate", "label" => "Check Gate Hardware", "auto" => false},
        %{"id" => "page_staff", "label" => "Page Staff", "auto" => true},
        %{"id" => "acknowledge", "label" => "Acknowledge", "auto" => false}
      ]
    }
  end
end
