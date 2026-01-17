defmodule AveroCommand.Scenarios.GateStuck do
  @moduledoc """
  Scenario #13: Gate Stuck Open

  Detects when a gate has been open for too long without closing.

  Trigger: gate.opened event followed by no gate.closed event
  within the threshold time.

  Severity: HIGH
  """
  require Logger

  alias AveroCommand.Entities.GateRegistry

  # Gate should close within 60 seconds
  @stuck_threshold_ms 60_000

  @doc """
  Evaluate if this event triggers the gate-stuck scenario.
  Events come through as event_type: "gates" with data: %{"type" => "gate.opened", ...}

  Also detects via long gate duration - if open_duration_ms > 60s, gate was stuck.
  """
  def evaluate(%{event_type: "gates", data: %{"type" => "gate.closed"} = data} = event) do
    # Check if the gate was open for too long (indicates it was stuck)
    open_duration_ms = data["open_duration_ms"] || 0

    if open_duration_ms >= @stuck_threshold_ms do
      Logger.info(
        "GateStuck: gate was open for #{open_duration_ms}ms (#{div(open_duration_ms, 1000)}s)"
      )

      {:match, build_incident_from_closed(event, open_duration_ms)}
    else
      :no_match
    end
  end

  def evaluate(%{event_type: "gates", data: %{"type" => "gate.opened"}} = _event) do
    # Gate opened - we could schedule a check but for now we detect on close
    :no_match
  end

  def evaluate(%{event_type: event_type} = event) when event_type in ["sensors", "people"] do
    # Check gate state on other events
    gate_id = event.gate_id || event.data["gate_id"]

    if gate_id do
      check_gate_state(event, gate_id)
    else
      :no_match
    end
  end

  def evaluate(_event), do: :no_match

  defp build_incident_from_closed(event, open_duration_ms) do
    open_duration_seconds = div(open_duration_ms, 1000)
    exit_summary = event.data["exit_summary"] || %{}

    %{
      type: "gate_stuck_open",
      severity: "high",
      category: "equipment",
      site: event.site,
      gate_id: event.data["gate_id"] || 0,
      context: %{
        gate_id: event.data["gate_id"] || 0,
        open_duration_seconds: open_duration_seconds,
        open_duration_ms: open_duration_ms,
        total_crossings: exit_summary["total_crossings"] || 0,
        message: "Gate was stuck open for #{open_duration_seconds} seconds"
      },
      suggested_actions: [
        %{"id" => "check_gate", "label" => "Check Gate Hardware", "auto" => false},
        %{"id" => "notify_maintenance", "label" => "Notify Maintenance", "auto" => true},
        %{"id" => "dismiss", "label" => "Dismiss", "auto" => false}
      ]
    }
  end

  defp check_gate_state(event, gate_id) do
    site = event.site

    case GateRegistry.get(site, gate_id) do
      nil ->
        :no_match

      pid ->
        state = AveroCommand.Entities.Gate.get_state(pid)

        if state && state.state == :open && is_stuck?(state.last_opened_at) do
          {:match, build_incident(event, state)}
        else
          :no_match
        end
    end
  end

  defp is_stuck?(nil), do: false

  defp is_stuck?(opened_at) do
    age_ms = DateTime.diff(DateTime.utc_now(), opened_at, :millisecond)
    age_ms > @stuck_threshold_ms
  end

  defp build_incident(event, gate_state) do
    open_duration = DateTime.diff(DateTime.utc_now(), gate_state.last_opened_at, :second)

    %{
      type: "gate_stuck_open",
      severity: "high",
      category: "equipment",
      site: event.site,
      gate_id: event.gate_id,
      context: %{
        gate_id: event.gate_id,
        open_duration_seconds: open_duration,
        persons_in_zone: gate_state.persons_in_zone,
        message: "Gate has been open for #{open_duration} seconds"
      },
      suggested_actions: [
        %{"id" => "close_gate", "label" => "Force Close Gate", "auto" => false},
        %{"id" => "notify_maintenance", "label" => "Notify Maintenance", "auto" => true},
        %{"id" => "dismiss", "label" => "Dismiss", "auto" => false}
      ]
    }
  end
end
