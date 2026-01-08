defmodule AveroCommand.Scenarios.BackwardEntry do
  @moduledoc """
  Scenario #10: Backward Entry

  Detects when someone crosses the exit line in the wrong direction
  (backward/entering through exit), which could indicate:
  - Someone trying to enter through exit
  - Confusion about store layout
  - Potential bypass attempt

  Trigger: xovis.line.cross with direction=backward on exit line
  Severity: INFO (can trigger alarm/light if configured)
  """
  require Logger

  @doc """
  Evaluate if this event triggers the backward-entry scenario.
  """
  def evaluate(%{event_type: "sensors", data: %{"type" => "xovis.line.cross"} = data} = event) do
    line = data["line"] || ""
    direction = data["direction"] || ""

    if exit_line?(line) && direction == "backward" do
      Logger.info("BackwardEntry: backward crossing on exit line #{line}")
      {:match, build_incident(event, data, line)}
    else
      :no_match
    end
  end

  def evaluate(_event), do: :no_match

  defp exit_line?(line) do
    line_upper = String.upcase(line)
    String.contains?(line_upper, "EXIT")
  end

  defp build_incident(event, data, line) do
    person_id = event.person_id || data["person_id"] || 0
    gate_id = data["gate_id"] || 0

    %{
      type: "backward_entry",
      severity: "info",
      category: "loss_prevention",
      site: event.site,
      gate_id: gate_id,
      related_person_id: person_id,
      context: %{
        person_id: person_id,
        gate_id: gate_id,
        line: line,
        direction: "backward",
        message: "Person crossed #{line} in wrong direction (entering through exit)"
      },
      suggested_actions: [
        %{"id" => "sound_alert", "label" => "Sound Alert", "auto" => false},
        %{"id" => "notify_staff", "label" => "Notify Staff", "auto" => false},
        %{"id" => "dismiss", "label" => "Dismiss", "auto" => false}
      ]
    }
  end
end
