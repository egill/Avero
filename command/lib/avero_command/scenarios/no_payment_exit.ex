defmodule AveroCommand.Scenarios.NoPaymentExit do
  @moduledoc """
  Scenario #1: No Payment + Exit Attempt

  Detects when a person:
  1. Dwelled at POS zone (>30s)
  2. Did not receive payment
  3. Is now attempting to exit (at gate)

  Severity: HIGH
  Actions: Keep gate closed, notify staff
  """
  require Logger

  alias AveroCommand.Entities.PersonRegistry

  @doc """
  Evaluate if this event triggers the no-payment-exit scenario.
  Event comes through as event_type: "people" with data: %{"type" => "person.state.changed", ...}
  """
  def evaluate(%{event_type: "people", data: %{"type" => "person.state.changed"} = data} = event) do
    # Check if person is transitioning to "at_gate"
    to_state = data["to_state"] || data["to"] || data["state"]

    if to_state == "at_gate" do
      Logger.info("NoPaymentExit: checking person #{event.person_id} transitioning to at_gate")
      check_person(event)
    else
      :no_match
    end
  end

  def evaluate(_event), do: :no_match

  defp check_person(event) do
    site = event.site
    person_id = event.person_id

    case PersonRegistry.get(site, person_id) do
      nil ->
        :no_match

      pid ->
        state = AveroCommand.Entities.Person.get_state(pid)

        cond do
          state == nil ->
            :no_match

          not state.dwelled_at_pos ->
            # Didn't spend time at POS, different scenario
            :no_match

          state.has_payment ->
            # Has payment, all good
            :no_match

          true ->
            # Dwelled at POS, no payment, now at gate
            {:match, build_incident(event, state)}
        end
    end
  end

  defp build_incident(event, person_state) do
    %{
      type: "no_payment_exit_attempt",
      severity: "high",
      category: "loss_prevention",
      site: event.site,
      gate_id: event.gate_id || event.data["gate_id"],
      related_person_id: event.person_id,
      context: %{
        person_id: event.person_id,
        gate_id: event.gate_id || event.data["gate_id"],
        dwelled_at_pos: person_state.dwelled_at_pos,
        zones_visited: person_state.zones_visited,
        message: "Person dwelled at POS but has no payment, attempting exit"
      },
      suggested_actions: [
        %{"id" => "notify_staff", "label" => "Notify Staff", "auto" => true},
        %{"id" => "open_gate", "label" => "Open Gate (Override)", "auto" => false},
        %{"id" => "dismiss", "label" => "Dismiss", "auto" => false}
      ]
    }
  end
end
