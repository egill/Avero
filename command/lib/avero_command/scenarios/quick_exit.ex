defmodule AveroCommand.Scenarios.QuickExit do
  @moduledoc """
  Scenario #6: Quick Exit / Payment Zone Skip

  Detects when a person reaches the gate very quickly (< 30s track duration)
  or without entering any POS zone, indicating they may have skipped payment.

  Trigger: person.state.changed to at_gate
  Severity: INFO (configurable enforcement level)
  """
  require Logger

  alias AveroCommand.Entities.PersonRegistry

  # Minimum expected journey time in milliseconds
  @min_journey_time_ms 30_000

  @doc """
  Evaluate if this event triggers the quick-exit scenario.
  """
  def evaluate(%{event_type: "people", data: %{"type" => "person.state.changed"} = data} = event) do
    to_state = data["to_state"] || data["to"] || data["state"]

    if to_state == "at_gate" do
      check_journey(event, data)
    else
      :no_match
    end
  end

  def evaluate(_event), do: :no_match

  defp check_journey(event, data) do
    site = event.site
    person_id = event.person_id || data["person_id"]

    case PersonRegistry.get(site, person_id) do
      nil ->
        # Can't verify - check from event data
        check_from_event(event, data, person_id)

      pid ->
        state = AveroCommand.Entities.Person.get_state(pid)
        check_from_state(event, data, person_id, state)
    end
  end

  defp check_from_state(_event, _data, person_id, state) do
    cond do
      state == nil ->
        :no_match

      # Already has payment - not suspicious
      state.has_payment ->
        :no_match

      # Dwelled at POS - they visited checkout, different scenario handles no-payment
      state.dwelled_at_pos ->
        :no_match

      # Check if they skipped POS entirely
      # NOTE: Disabled individual incident creation - quick exits are tracked for stats via logs only
      not visited_pos_zone?(state.zones_visited) ->
        Logger.info("QuickExit: person #{person_id} skipped POS zone entirely")
        :no_match

      # Check if journey was too fast
      # NOTE: Disabled individual incident creation - quick exits are tracked for stats via logs only
      journey_too_fast?(state.started_at) ->
        journey_ms = DateTime.diff(DateTime.utc_now(), state.started_at, :millisecond)
        Logger.info("QuickExit: person #{person_id} journey only #{journey_ms}ms")
        :no_match

      true ->
        :no_match
    end
  end

  defp check_from_event(_event, data, person_id) do
    # Fallback: check journey duration from event data
    # NOTE: Disabled individual incident creation - quick exits are tracked for stats via logs only
    journey_ms = data["journey_duration_ms"] || data["track_duration_ms"]

    if journey_ms && journey_ms < @min_journey_time_ms do
      Logger.info("QuickExit: person #{person_id} journey only #{journey_ms}ms (from event data)")
      :no_match
    else
      :no_match
    end
  end

  defp visited_pos_zone?(zones_visited) when is_list(zones_visited) do
    Enum.any?(zones_visited, fn zone_visit ->
      zone = zone_visit[:zone] || zone_visit["zone"] || ""
      String.starts_with?(zone, "POS")
    end)
  end

  defp visited_pos_zone?(_), do: false

  defp journey_too_fast?(nil), do: false

  defp journey_too_fast?(started_at) do
    journey_ms = DateTime.diff(DateTime.utc_now(), started_at, :millisecond)
    journey_ms < @min_journey_time_ms
  end
end
