defmodule AveroCommand.Scenarios.SuspiciousReturn do
  @moduledoc """
  Scenario #7: Checkout Return

  Detects when a person crosses back into the store after entering
  the checkout area. This is normal behavior - people may:
  - Forget an item and go back
  - Change their mind about a purchase
  - Go to a different checkout lane

  Trigger: person.returned_to_store event
  Severity: INFO (informational tracking only)
  """
  require Logger

  @doc """
  Evaluate if this event triggers the checkout-return scenario.
  Event comes through as event_type: "people" with data: %{"type" => "person.returned_to_store", ...}
  """
  def evaluate(%{event_type: "people", data: %{"type" => "person.returned_to_store"} = data} = event) do
    Logger.debug("CheckoutReturn: person #{event.person_id || data["person_id"]} returned to store")
    {:match, build_incident(event, data)}
  end

  def evaluate(_event), do: :no_match

  defp build_incident(event, data) do
    person_id = event.person_id || data["person_id"] || 0
    was_authorized = data["was_authorized"] || false
    last_zone = data["last_zone"] || "unknown"

    %{
      type: "checkout_return",
      severity: "info",
      category: "tracking",
      site: event.site,
      gate_id: data["gate_id"] || 0,
      related_person_id: person_id,
      context: %{
        person_id: person_id,
        was_authorized: was_authorized,
        last_zone: last_zone,
        message: "Person #{person_id} returned to store from checkout area"
      },
      suggested_actions: [
        %{"id" => "dismiss", "label" => "Dismiss", "auto" => true}
      ]
    }
  end
end
