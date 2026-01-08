defmodule AveroCommand.Scenarios.GateFault do
  @moduledoc """
  Scenario #4: Gate Fault

  Detects when a gate reports a hardware fault condition.

  Trigger: gate.fault event from gate controller
  Severity: HIGH (gate may not operate correctly)
  """
  require Logger

  @doc """
  Evaluate if this event triggers the gate-fault scenario.
  Event comes through as event_type: "gates" with data: %{"type" => "gate.fault", ...}
  """
  def evaluate(%{event_type: "gates", data: %{"type" => "gate.fault"} = data} = event) do
    Logger.warning("GateFault: gate #{data["gate_id"]} reported fault: #{data["message"]}")
    {:match, build_incident(event, data)}
  end

  def evaluate(_event), do: :no_match

  defp build_incident(event, data) do
    gate_id = data["gate_id"] || 0
    fault_code = data["code"] || 0
    fault_message = data["message"] || "Unknown fault"

    %{
      type: "gate_fault",
      severity: severity_for_code(fault_code),
      category: "equipment",
      site: event.site,
      gate_id: gate_id,
      context: %{
        gate_id: gate_id,
        fault_code: fault_code,
        fault_message: fault_message,
        message: "Gate #{gate_id} fault: #{fault_message} (code: #{fault_code})"
      },
      suggested_actions: [
        %{"id" => "check_gate", "label" => "Inspect Gate Hardware", "auto" => false},
        %{"id" => "clear_fault", "label" => "Clear Fault", "auto" => false},
        %{"id" => "notify_maintenance", "label" => "Notify Maintenance", "auto" => true}
      ]
    }
  end

  # Map fault codes to severity levels
  defp severity_for_code(code) when code >= 100, do: "critical"
  defp severity_for_code(code) when code >= 50, do: "high"
  defp severity_for_code(_code), do: "medium"
end
