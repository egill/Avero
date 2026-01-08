defmodule AveroCommand.Scenarios.GroupSplit do
  @moduledoc """
  Scenario #7: Group Split

  Detects when multiple people (3+) exit in the same gate cycle but
  with a low payment ratio, indicating potential group-based evasion.

  Trigger: gate.closed event with exit_summary
  Severity: INFO (log for analysis)
  """
  require Logger

  # Minimum crossings to consider as a group
  @group_threshold 3

  @doc """
  Evaluate if this event triggers the group-split scenario.
  """
  def evaluate(%{event_type: "gates", data: %{"type" => "gate.closed"} = data} = event) do
    exit_summary = data["exit_summary"] || %{}
    total_crossings = exit_summary["total_crossings"] || 0
    authorized_count = exit_summary["authorized_count"] || 0

    if total_crossings >= @group_threshold do
      check_payment_ratio(event, data, exit_summary, total_crossings, authorized_count)
    else
      :no_match
    end
  end

  def evaluate(_event), do: :no_match

  defp check_payment_ratio(event, data, exit_summary, total_crossings, authorized_count) do
    # Calculate payment ratio
    payment_ratio = authorized_count / total_crossings
    unauthorized_count = exit_summary["unauthorized_count"] || 0

    # If more than half are unauthorized, flag it
    if payment_ratio < 0.5 do
      Logger.info("GroupSplit: #{total_crossings} people exited, only #{authorized_count} authorized (#{Float.round(payment_ratio * 100, 1)}%)")
      {:match, build_incident(event, data, exit_summary, total_crossings, authorized_count, unauthorized_count, payment_ratio)}
    else
      :no_match
    end
  end

  defp build_incident(event, data, exit_summary, total_crossings, authorized_count, unauthorized_count, payment_ratio) do
    gate_id = data["gate_id"] || 0
    open_duration_ms = data["open_duration_ms"] || 0

    %{
      type: "group_split",
      severity: "info",
      category: "loss_prevention",
      site: event.site,
      gate_id: gate_id,
      context: %{
        gate_id: gate_id,
        total_crossings: total_crossings,
        authorized_count: authorized_count,
        unauthorized_count: unauthorized_count,
        payment_ratio: Float.round(payment_ratio * 100, 1),
        open_duration_ms: open_duration_ms,
        tailgating_count: exit_summary["tailgating_count"] || 0,
        message: "Group of #{total_crossings} exited with #{authorized_count} payments (#{Float.round(payment_ratio * 100, 1)}% paid)"
      },
      suggested_actions: [
        %{"id" => "review_camera", "label" => "Review Camera Footage", "auto" => false},
        %{"id" => "log_analysis", "label" => "Log for Analysis", "auto" => true},
        %{"id" => "dismiss", "label" => "Dismiss", "auto" => false}
      ]
    }
  end
end
