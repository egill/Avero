defmodule AveroCommand.Scenarios.GateAlarm do
  @moduledoc """
  Scenario #5: Gate Alarm

  Detects when a gate triggers an alarm condition (e.g., forced entry,
  obstruction, emergency stop).

  Trigger: gate.alarm event from gate controller
  Severity: HIGH to CRITICAL depending on alarm type
  """
  require Logger

  @doc """
  Evaluate if this event triggers the gate-alarm scenario.
  Event comes through as event_type: "gates" with data: %{"type" => "gate.alarm", ...}
  """
  def evaluate(%{event_type: "gates", data: %{"type" => "gate.alarm"} = data} = event) do
    Logger.warning("GateAlarm: gate #{data["gate_id"]} triggered alarm: #{data["message"]}")
    {:match, build_incident(event, data)}
  end

  def evaluate(_event), do: :no_match

  defp build_incident(event, data) do
    gate_id = data["gate_id"] || 0
    alarm_code = data["code"] || 0
    alarm_message = data["message"] || "Unknown alarm"
    {severity, category} = classify_alarm(alarm_code, alarm_message)

    %{
      type: "gate_alarm",
      severity: severity,
      category: category,
      site: event.site,
      gate_id: gate_id,
      context: %{
        gate_id: gate_id,
        alarm_code: alarm_code,
        alarm_message: alarm_message,
        message: "Gate #{gate_id} alarm: #{alarm_message} (code: #{alarm_code})"
      },
      suggested_actions: build_actions(alarm_code)
    }
  end

  # Classify alarm by code/message
  defp classify_alarm(code, message) do
    message_lower = String.downcase(message || "")

    cond do
      String.contains?(message_lower, "forced") ->
        {"critical", "security"}

      String.contains?(message_lower, "emergency") ->
        {"critical", "safety"}

      String.contains?(message_lower, "obstruction") ->
        {"high", "operational"}

      code >= 100 ->
        {"critical", "security"}

      code >= 50 ->
        {"high", "operational"}

      true ->
        {"medium", "operational"}
    end
  end

  defp build_actions(code) when code >= 100 do
    [
      %{"id" => "dispatch_security", "label" => "Dispatch Security", "auto" => true},
      %{"id" => "review_camera", "label" => "Review Camera Footage", "auto" => false},
      %{"id" => "check_gate", "label" => "Inspect Gate", "auto" => false},
      %{"id" => "acknowledge", "label" => "Acknowledge", "auto" => false}
    ]
  end

  defp build_actions(_code) do
    [
      %{"id" => "check_gate", "label" => "Inspect Gate", "auto" => false},
      %{"id" => "clear_alarm", "label" => "Clear Alarm", "auto" => false},
      %{"id" => "acknowledge", "label" => "Acknowledge", "auto" => false}
    ]
  end
end
