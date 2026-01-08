defmodule AveroCommand.Scenarios.LongGateDuration do
  @moduledoc """
  Scenario: Long Gate Duration

  Detects when a gate was open for longer than expected.
  This is less severe than gate_stuck (which is 60s+) but still
  indicates potential issues like:
  - Slow customer movement
  - Multiple people exiting on one cycle
  - Partial sensor obstruction

  Trigger: gate.closed event with open_duration_ms > threshold
  Severity: MEDIUM (12-30s) or HIGH (30-60s)
  """
  require Logger

  # Base thresholds in milliseconds (for 1 person)
  # Add ~10 seconds per additional crossing
  @base_warning_ms 30_000        # 30 seconds for 1 person
  @base_elevated_ms 60_000       # 60 seconds (gate stuck)
  @per_crossing_allowance_ms 10_000  # 10 seconds per additional crossing

  @doc """
  Evaluate if this event triggers the long-gate-duration scenario.
  Event comes through with event_type: "gates" and data: %{"type" => "gate.closed", ...}
  """
  def evaluate(%{event_type: "gates", data: %{"type" => "gate.closed"} = data} = event) do
    Logger.info("LongGateDuration: evaluating gate.closed event, duration=#{data["open_duration_ms"]}")
    check_duration(data, event)
  end

  # NOTE: Removed dead code path - event_type is always derived from topic (e.g., "gates")
  # not from data["type"] (e.g., "gate.closed"). The pattern above handles all cases.

  def evaluate(%{event_type: _event_type} = _event) do
    :no_match
  end

  def evaluate(_event), do: :no_match

  defp check_duration(%{"open_duration_ms" => duration_ms} = data, event) when is_integer(duration_ms) do
    gate_id = Map.get(data, "gate_id", 0)

    # Skip gate_id 0 (invalid)
    if gate_id == 0 do
      :no_match
    else
      exit_summary = Map.get(data, "exit_summary", %{})
      total_crossings = Map.get(exit_summary, "total_crossings", 0)

      # Adjust thresholds based on number of crossings
      # More crossings = more time is acceptable
      extra_allowance = max(0, total_crossings - 1) * @per_crossing_allowance_ms
      warning_threshold = @base_warning_ms + extra_allowance
      elevated_threshold = @base_elevated_ms + extra_allowance

      # Only flag if duration exceeds threshold AND there are issues
      # (unauthorized exits or unusually long even with crossings)
      unauthorized_count = Map.get(exit_summary, "unauthorized_count", 0)

      cond do
        # Gate stuck level - always flag
        duration_ms >= elevated_threshold ->
          {:match, build_incident(event, data, duration_ms, "high")}

        # Warning level - only if there are unauthorized or no crossings
        duration_ms >= warning_threshold && (unauthorized_count > 0 || total_crossings == 0) ->
          {:match, build_incident(event, data, duration_ms, "medium")}

        # All authorized, reasonable time per person - not an incident
        true ->
          :no_match
      end
    end
  end

  defp check_duration(_, _), do: :no_match

  defp build_incident(event, data, duration_ms, severity) do
    duration_seconds = duration_ms / 1000
    gate_id = Map.get(data, "gate_id", 0)

    exit_summary = Map.get(data, "exit_summary", %{})
    total_crossings = Map.get(exit_summary, "total_crossings", 0)
    authorized_count = Map.get(exit_summary, "authorized_count", 0)
    unauthorized_count = Map.get(exit_summary, "unauthorized_count", 0)
    tailgating_count = Map.get(exit_summary, "tailgating_count", 0)

    # Auth method breakdown (payment, barcode, etc.)
    auth_breakdown = Map.get(data, "auth_method_breakdown", %{})

    # Build message with payment info
    payment_info = build_payment_summary(authorized_count, unauthorized_count, auth_breakdown)

    %{
      type: "long_gate_duration",
      severity: severity,
      category: "operational",
      site: event.site,
      gate_id: gate_id,
      context: %{
        gate_id: gate_id,
        duration_seconds: Float.round(duration_seconds, 1),
        duration_ms: duration_ms,
        total_crossings: total_crossings,
        authorized_count: authorized_count,
        unauthorized_count: unauthorized_count,
        tailgating_count: tailgating_count,
        auth_method_breakdown: auth_breakdown,
        message: "Gate #{gate_id} open for #{Float.round(duration_seconds, 1)}s - #{payment_info}"
      },
      suggested_actions: build_actions(severity, tailgating_count, unauthorized_count)
    }
  end

  defp build_payment_summary(authorized, unauthorized, breakdown) do
    parts = ["#{authorized + unauthorized} crossings"]

    parts =
      if authorized > 0 do
        methods =
          breakdown
          |> Enum.filter(fn {_k, v} -> v > 0 end)
          |> Enum.map(fn {k, v} -> "#{v} #{k}" end)
          |> Enum.join(", ")

        if methods != "" do
          parts ++ ["#{authorized} paid (#{methods})"]
        else
          parts ++ ["#{authorized} paid"]
        end
      else
        parts
      end

    parts =
      if unauthorized > 0 do
        parts ++ ["#{unauthorized} unpaid"]
      else
        parts
      end

    Enum.join(parts, ", ")
  end

  defp build_actions("high", tailgating_count, _unauthorized) when tailgating_count > 0 do
    [
      %{"id" => "review_footage", "label" => "Review Camera Footage", "auto" => false},
      %{"id" => "check_sensor", "label" => "Check Sensor Alignment", "auto" => false},
      %{"id" => "acknowledge", "label" => "Acknowledge", "auto" => false}
    ]
  end

  defp build_actions("high", _, _) do
    [
      %{"id" => "check_sensor", "label" => "Check Sensor Alignment", "auto" => false},
      %{"id" => "acknowledge", "label" => "Acknowledge", "auto" => false}
    ]
  end

  defp build_actions(_, _, unauthorized) when unauthorized > 0 do
    [
      %{"id" => "review_footage", "label" => "Review Camera Footage", "auto" => false},
      %{"id" => "acknowledge", "label" => "Acknowledge", "auto" => false}
    ]
  end

  defp build_actions(_, _, _) do
    [
      %{"id" => "acknowledge", "label" => "Acknowledge", "auto" => false},
      %{"id" => "dismiss", "label" => "Dismiss", "auto" => false}
    ]
  end
end
