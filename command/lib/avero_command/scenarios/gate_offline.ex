defmodule AveroCommand.Scenarios.GateOffline do
  @moduledoc """
  Scenario #3: Gate Offline

  Detects when a gate controller becomes unreachable.

  Trigger: gate.offline event from gate controller communication failure
  Severity: CRITICAL (gate cannot be controlled)
  """
  require Logger

  @doc """
  Evaluate if this event triggers the gate-offline scenario.
  Event comes through as event_type: "gates" with data: %{"type" => "gate.offline", ...}
  """
  def evaluate(%{event_type: "gates", data: %{"type" => "gate.offline"} = data} = event) do
    Logger.warning("GateOffline: gate #{data["gate_id"]} went offline",
      error: data["error"],
      source: data["source"]
    )

    {:match, build_incident(event, data)}
  end

  def evaluate(_event), do: :no_match

  defp build_incident(event, data) do
    gate_id = data["gate_id"] || 0

    source = data["source"] || "rs485"
    error = data["error"]

    message =
      if is_binary(error) and error != "" do
        "Gate #{gate_id} is offline (#{String.upcase(source)}): #{error}"
      else
        "Gate #{gate_id} is offline - #{String.upcase(source)} communication lost"
      end

    %{
      type: "gate_offline",
      severity: "critical",
      category: "equipment",
      site: event.site,
      gate_id: gate_id,
      context: %{
        gate_id: gate_id,
        source: source,
        error: error,
        message: message
      },
      suggested_actions: [
        %{"id" => "check_cable", "label" => "Check RS485 Cable", "auto" => false},
        %{"id" => "check_power", "label" => "Check Gate Power", "auto" => false},
        %{"id" => "restart_gateway", "label" => "Restart Gateway", "auto" => false},
        %{"id" => "notify_maintenance", "label" => "Notify Maintenance", "auto" => true}
      ]
    }
  end
end
