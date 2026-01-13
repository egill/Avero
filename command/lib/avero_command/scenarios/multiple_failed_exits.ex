defmodule AveroCommand.Scenarios.MultipleFailedExits do
  @moduledoc """
  Scenario #11: Multiple Failed Exit Attempts

  Detects when a person makes multiple attempts to exit through
  the gate but is never authorized, indicating potential issues
  like lost receipt, technical problems, or evasion attempts.

  Trigger: Track gate attempts per person, alert on 3+ with 0 authorizations
  Severity: MEDIUM
  """
  require Logger

  alias AveroCommand.Store

  # Number of failed attempts before alerting
  @failed_attempts_threshold 3
  # Time window to check for attempts (seconds)
  @time_window_seconds 300

  @doc """
  Evaluate if this event triggers the multiple-failed-exits scenario.
  Check on person state changes to at_gate.
  """
  def evaluate(%{event_type: "people", data: %{"type" => "person.state.changed"} = data} = event) do
    to_state = data["to_state"] || data["to"] || data["state"]

    if to_state == "at_gate" do
      check_exit_attempts(event, data)
    else
      :no_match
    end
  end

  # Also check on exit.confirmed with authorized=false
  def evaluate(
        %{event_type: "exits", data: %{"type" => "exit.confirmed", "authorized" => false} = data} =
          event
      ) do
    check_exit_attempts(event, data)
  end

  def evaluate(_event), do: :no_match

  defp check_exit_attempts(event, data) do
    site = event.site
    person_id = event.person_id || data["person_id"]

    if person_id do
      since = DateTime.add(DateTime.utc_now(), -@time_window_seconds, :second)
      attempts = get_exit_attempts(site, person_id, since)

      # Count unauthorized attempts
      unauthorized_attempts =
        Enum.count(attempts, fn e ->
          e.data["authorized"] == false || e.data["authorized"] == "false"
        end)

      # Count any authorizations
      any_authorized =
        Enum.any?(attempts, fn e ->
          e.data["authorized"] == true || e.data["authorized"] == "true"
        end)

      if unauthorized_attempts >= @failed_attempts_threshold && not any_authorized do
        Logger.info(
          "MultipleFailedExits: person #{person_id} has #{unauthorized_attempts} failed attempts"
        )

        {:match, build_incident(event, data, person_id, unauthorized_attempts)}
      else
        :no_match
      end
    else
      :no_match
    end
  end

  defp get_exit_attempts(site, person_id, since) do
    Store.recent_events(100, site)
    |> Enum.filter(fn e ->
      e.event_type == "exits" && e.data["type"] == "exit.confirmed" &&
        e.person_id == person_id &&
        DateTime.compare(e.time, since) == :gt
    end)
  rescue
    _ -> []
  end

  defp build_incident(event, data, person_id, attempt_count) do
    gate_id = event.gate_id || data["gate_id"] || 0

    %{
      type: "multiple_failed_exits",
      severity: "medium",
      category: "loss_prevention",
      site: event.site,
      gate_id: gate_id,
      related_person_id: person_id,
      context: %{
        person_id: person_id,
        gate_id: gate_id,
        attempt_count: attempt_count,
        threshold: @failed_attempts_threshold,
        time_window_minutes: div(@time_window_seconds, 60),
        message:
          "Person has #{attempt_count} failed exit attempts in #{div(@time_window_seconds, 60)} minutes"
      },
      suggested_actions: [
        %{"id" => "assist_customer", "label" => "Assist Customer", "auto" => false},
        %{"id" => "notify_staff", "label" => "Notify Staff", "auto" => true},
        %{"id" => "review_camera", "label" => "Review Camera", "auto" => false},
        %{"id" => "dismiss", "label" => "Dismiss", "auto" => false}
      ]
    }
  end
end
