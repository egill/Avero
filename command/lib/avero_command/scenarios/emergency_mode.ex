defmodule AveroCommand.Scenarios.EmergencyMode do
  @moduledoc """
  Scenario #26: Emergency Mode Activated

  Detects when emergency mode is triggered on gates, which could be:
  - Fire alarm activation
  - Manual emergency override
  - Security lockdown
  - Power failure failsafe

  Trigger: gate.emergency event or manual override
  Severity: CRITICAL
  """
  require Logger

  @doc """
  Evaluate if this event triggers the emergency-mode scenario.
  """
  def evaluate(%{event_type: "gates", data: %{"type" => "gate.emergency"} = data} = event) do
    Logger.error("EmergencyMode: gate emergency triggered - #{inspect(data["reason"])}")
    {:match, build_incident(event, data, :emergency)}
  end

  # Also match emergency_open events
  def evaluate(%{event_type: "gates", data: %{"type" => "gate.emergency_open"} = data} = event) do
    Logger.error("EmergencyMode: gate emergency open - #{inspect(data["reason"])}")
    {:match, build_incident(event, data, :emergency_open)}
  end

  # Match manual override events
  def evaluate(%{event_type: "gates", data: %{"type" => "gate.manual_override"} = data} = event) do
    Logger.warning("EmergencyMode: manual override activated on gate #{data["gate_id"]}")
    {:match, build_incident(event, data, :manual_override)}
  end

  # Match system emergency events
  def evaluate(%{event_type: "system", data: %{"type" => "system.emergency"} = data} = event) do
    Logger.error("EmergencyMode: system emergency - #{inspect(data["reason"])}")
    {:match, build_incident(event, data, :system_emergency)}
  end

  # Match fire alarm events
  def evaluate(%{event_type: "system", data: %{"type" => "fire_alarm"} = data} = event) do
    Logger.error("EmergencyMode: FIRE ALARM ACTIVATED")
    {:match, build_incident(event, data, :fire_alarm)}
  end

  # Match security lockdown
  def evaluate(%{event_type: "system", data: %{"type" => "security_lockdown"} = data} = event) do
    Logger.error("EmergencyMode: security lockdown initiated")
    {:match, build_incident(event, data, :security_lockdown)}
  end

  def evaluate(_event), do: :no_match

  defp build_incident(event, data, emergency_type) do
    gate_id = data["gate_id"] || 0
    reason = data["reason"] || data["message"] || "Unknown"
    triggered_by = data["triggered_by"] || data["source"] || "system"

    {severity, category, message} =
      case emergency_type do
        :fire_alarm ->
          {"critical", "safety", "FIRE ALARM - All gates opening for evacuation"}

        :emergency ->
          {"critical", "safety", "Emergency mode activated: #{reason}"}

        :emergency_open ->
          {"critical", "safety", "Emergency gate open: #{reason}"}

        :system_emergency ->
          {"critical", "safety", "System emergency: #{reason}"}

        :security_lockdown ->
          {"high", "security", "Security lockdown initiated: #{reason}"}

        :manual_override ->
          {"medium", "equipment", "Manual override on gate #{gate_id}: #{reason}"}
      end

    %{
      type: "emergency_mode",
      severity: severity,
      category: category,
      site: event.site,
      gate_id: gate_id,
      context: %{
        emergency_type: emergency_type,
        reason: reason,
        triggered_by: triggered_by,
        gate_id: gate_id,
        message: message
      },
      suggested_actions: build_actions(emergency_type)
    }
  end

  defp build_actions(:fire_alarm) do
    [
      %{"id" => "verify_alarm", "label" => "Verify Fire Alarm", "auto" => false},
      %{"id" => "call_emergency", "label" => "Call Emergency Services", "auto" => true},
      %{"id" => "evacuate", "label" => "Initiate Evacuation", "auto" => true},
      %{"id" => "acknowledge", "label" => "Acknowledge", "auto" => false}
    ]
  end

  defp build_actions(:security_lockdown) do
    [
      %{"id" => "verify_threat", "label" => "Verify Threat", "auto" => false},
      %{"id" => "contact_security", "label" => "Contact Security", "auto" => true},
      %{"id" => "release_lockdown", "label" => "Release Lockdown", "auto" => false},
      %{"id" => "acknowledge", "label" => "Acknowledge", "auto" => false}
    ]
  end

  defp build_actions(:manual_override) do
    [
      %{"id" => "verify_override", "label" => "Verify Override Reason", "auto" => false},
      %{"id" => "reset_gate", "label" => "Reset Gate to Normal", "auto" => false},
      %{"id" => "acknowledge", "label" => "Acknowledge", "auto" => false}
    ]
  end

  defp build_actions(_emergency) do
    [
      %{"id" => "assess_situation", "label" => "Assess Situation", "auto" => false},
      %{"id" => "contact_manager", "label" => "Contact Manager", "auto" => true},
      %{"id" => "reset_system", "label" => "Reset to Normal", "auto" => false},
      %{"id" => "acknowledge", "label" => "Acknowledge", "auto" => false}
    ]
  end
end
