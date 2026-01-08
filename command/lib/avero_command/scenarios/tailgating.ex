defmodule AveroCommand.Scenarios.Tailgating do
  @moduledoc """
  Scenario #2: Tailgating

  Detects when an unauthorized person exits during the same
  gate cycle as an authorized person.

  Trigger: exit.confirmed event with authorized=false within
  a short time window of an authorized exit.

  Severity: HIGH
  """
  require Logger

  alias AveroCommand.Store

  # Time window to consider as same gate cycle (ms)
  @tailgate_window_ms 10_000

  @doc """
  Evaluate if this event triggers the tailgating scenario.

  Priority 1: Direct tailgating.detected events from gateway (with full context)
  Priority 2: exit.confirmed event with authorized=false (fallback detection)
  Priority 3: gate.closed events which have tailgating info in exit_summary
  """
  # Handle direct tailgating.detected events from the gateway
  # These include enriched context about both persons, POS visits, groups, etc.
  # NOTE: Gateway publishes tailgating.detected to /tracking topic, so event_type = "tracking"
  def evaluate(%{event_type: "tracking", data: %{"type" => "tailgating.detected"} = data} = event) do
    Logger.info("Tailgating: direct detection from gateway - authorized=#{data["authorized_person_id"]}, unauthorized=#{data["unauthorized_person_id"]}")
    {:match, build_enriched_incident(event, data)}
  end

  def evaluate(%{event_type: "exits", data: %{"type" => "exit.confirmed", "authorized" => false}} = event) do
    Logger.info("Tailgating: checking unauthorized exit for person #{event.person_id}")
    check_for_tailgating(event)
  end

  def evaluate(%{event_type: "exits", data: %{"type" => "exit.confirmed", "tailgated" => true, "authorized" => false}} = event) do
    # This exit was flagged as a tailgate by the gateway - find the authorized person
    # NOTE: Must check authorized=false to avoid false positives on authorized tailgaters
    Logger.info("Tailgating: gateway flagged tailgate for unauthorized person #{event.person_id}")
    # Look for recent authorized exit at same gate
    recent_events = get_recent_exits(event.site, event.gate_id)
    authorized_event = Enum.find(recent_events, fn e ->
      e.authorized == true and within_window?(e.time, event.time)
    end)

    if authorized_event do
      {:match, build_incident(event, authorized_event)}
    else
      # Fallback if we can't find authorized person
      {:match, build_incident_single(event)}
    end
  end

  def evaluate(%{event_type: "gates", data: %{"type" => "gate.closed"} = data} = event) do
    # Check if the gate cycle had tailgating
    exit_summary = data["exit_summary"] || %{}
    tailgating_count = exit_summary["tailgating_count"] || 0

    if tailgating_count > 0 do
      Logger.info("Tailgating: gate.closed with #{tailgating_count} tailgaters")
      {:match, build_gate_cycle_incident(event, exit_summary)}
    else
      :no_match
    end
  end

  def evaluate(_event), do: :no_match

  defp check_for_tailgating(event) do
    site = event.site
    gate_id = event.gate_id

    # Look for a recent authorized exit at the same gate
    recent_events = get_recent_exits(site, gate_id)

    # Find the authorized person in the recent events
    authorized_event =
      Enum.find(recent_events, fn e ->
        e.authorized == true and
          within_window?(e.time, event.time)
      end)

    if authorized_event do
      {:match, build_incident(event, authorized_event)}
    else
      :no_match
    end
  end

  defp get_recent_exits(site, gate_id) do
    # Get exit events from the last 30 seconds
    Store.recent_events(50, site)
    |> Enum.filter(fn e ->
      e.event_type == "exit.confirmed" and e.gate_id == gate_id
    end)
  rescue
    _ -> []
  end

  defp within_window?(time1, time2) do
    diff_ms = abs(DateTime.diff(time1, time2, :millisecond))
    diff_ms <= @tailgate_window_ms
  end

  defp build_incident(unauthorized_event, authorized_event) do
    unauthorized_id = unauthorized_event.person_id
    authorized_id = authorized_event.person_id
    gate_id = unauthorized_event.gate_id || unauthorized_event.data["gate_id"]

    %{
      type: "tailgating_detected",
      severity: "high",
      category: "loss_prevention",
      site: unauthorized_event.site,
      gate_id: gate_id,
      related_person_id: unauthorized_id,
      context: %{
        # Gate opener: single person who triggered the open command
        gate_opener_id: authorized_id,
        gate_opener_method: authorized_event.auth_method,
        # Followers: array of persons who tailgated (can be multiple)
        follower_ids: [unauthorized_id],
        gate_id: gate_id,
        message: "Person #{unauthorized_id} tailgated through gate opened by #{authorized_id}"
      },
      suggested_actions: [
        %{"id" => "notify_security", "label" => "Notify Security", "auto" => true},
        %{"id" => "review_camera", "label" => "Review Camera Footage", "auto" => false},
        %{"id" => "dismiss", "label" => "Dismiss", "auto" => false}
      ]
    }
  end

  # Fallback when we can't find the gate opener
  defp build_incident_single(event) do
    unauthorized_id = event.person_id
    gate_id = event.gate_id || event.data["gate_id"]

    %{
      type: "tailgating_detected",
      severity: "high",
      category: "loss_prevention",
      site: event.site,
      gate_id: gate_id,
      related_person_id: unauthorized_id,
      context: %{
        # Gate opener unknown (couldn't find recent authorized exit)
        gate_opener_id: nil,
        gate_opener_method: nil,
        # Followers: the detected tailgater(s)
        follower_ids: [unauthorized_id],
        gate_id: gate_id,
        message: "Tailgating detected - Person #{unauthorized_id} (gate opener unknown)"
      },
      suggested_actions: [
        %{"id" => "notify_security", "label" => "Notify Security", "auto" => true},
        %{"id" => "review_camera", "label" => "Review Camera Footage", "auto" => false},
        %{"id" => "dismiss", "label" => "Dismiss", "auto" => false}
      ]
    }
  end

  defp build_gate_cycle_incident(event, exit_summary) do
    %{
      type: "tailgating_detected",
      severity: "high",
      category: "loss_prevention",
      site: event.site,
      gate_id: event.data["gate_id"] || 0,
      context: %{
        gate_id: event.data["gate_id"] || 0,
        total_crossings: exit_summary["total_crossings"] || 0,
        authorized_count: exit_summary["authorized_count"] || 0,
        unauthorized_count: exit_summary["unauthorized_count"] || 0,
        tailgating_count: exit_summary["tailgating_count"] || 0,
        message: "#{exit_summary["tailgating_count"]} tailgater(s) detected in gate cycle with #{exit_summary["total_crossings"]} total crossings"
      },
      suggested_actions: [
        %{"id" => "notify_security", "label" => "Notify Security", "auto" => true},
        %{"id" => "review_camera", "label" => "Review Camera Footage", "auto" => false},
        %{"id" => "dismiss", "label" => "Dismiss", "auto" => false}
      ]
    }
  end

  # Build incident from enriched tailgating.detected event from gateway
  # Contains full context about both persons, POS visits, groups, etc.
  defp build_enriched_incident(event, data) do
    # Determine severity based on context
    # Same group = lower severity (tagging issue, not theft)
    # Unauthorized paid = lower severity (likely honest mistake)
    severity = cond do
      data["same_group"] == true -> "medium"
      data["unauthorized_paid"] == true -> "medium"
      true -> "high"
    end

    # Build descriptive message based on context
    message = build_tailgate_message(data)

    # Extract follower IDs - gateway may send single ID or array
    follower_ids = case data["unauthorized_person_ids"] do
      ids when is_list(ids) -> ids
      nil -> if data["unauthorized_person_id"], do: [data["unauthorized_person_id"]], else: []
      _ -> []
    end

    %{
      type: "tailgating_detected",
      severity: severity,
      category: "loss_prevention",
      site: event.site,
      gate_id: data["gate_id"],
      related_person_id: List.first(follower_ids),
      context: %{
        # Gate opener: the authorized person who triggered the open
        gate_opener_id: data["authorized_person_id"],
        gate_opener_method: data["authorized_method"],
        gate_opener_session_id: data["authorized_session_id"],
        gate_opener_last_zone: data["authorized_last_zone"],

        # Followers: array of persons who tailgated
        follower_ids: follower_ids,

        # Additional context for first/primary follower (for backward compat)
        follower_last_zone: data["unauthorized_last_zone"],
        follower_visited_pos: data["unauthorized_visited_pos"] || false,
        follower_last_pos_zone: data["unauthorized_last_pos_zone"],
        follower_paid: data["unauthorized_paid"] || false,
        follower_session_id: data["unauthorized_session_id"],

        gate_id: data["gate_id"],

        # Relationship context
        same_group: data["same_group"] || false,
        group_id: data["group_id"],
        same_pos_zone: data["same_pos_zone"] || false,
        shared_pos_zone: data["shared_pos_zone"],

        # Timing
        gate_open_duration_ms: data["gate_open_duration_ms"],
        trigger_source: data["trigger_source"],

        # Summary message
        message: message
      },
      suggested_actions: build_suggested_actions(data)
    }
  end

  defp build_tailgate_message(data) do
    cond do
      data["same_group"] == true ->
        "Same group exit - Person #{data["unauthorized_person_id"]} exited with group member #{data["authorized_person_id"]} (tagging issue)"

      data["unauthorized_paid"] == true ->
        "Paid customer tailgated - Person #{data["unauthorized_person_id"]} paid at #{data["unauthorized_last_pos_zone"]} but followed #{data["authorized_person_id"]}"

      data["same_pos_zone"] == true ->
        "Co-shoppers - Both visited #{data["shared_pos_zone"]} before exit"

      data["unauthorized_visited_pos"] == true ->
        "Visited POS without payment - Person #{data["unauthorized_person_id"]} was at #{data["unauthorized_last_pos_zone"]} but didn't complete payment"

      true ->
        "Unauthorized exit - Person #{data["unauthorized_person_id"]} followed authorized person #{data["authorized_person_id"]}"
    end
  end

  defp build_suggested_actions(data) do
    base_actions = [
      %{"id" => "review_camera", "label" => "Review Camera Footage", "auto" => false}
    ]

    # Add different actions based on context
    cond do
      data["same_group"] == true ->
        # Same group - likely a tagging issue, not theft
        [
          %{"id" => "flag_tagging_issue", "label" => "Flag Tagging Issue", "auto" => false},
          %{"id" => "dismiss", "label" => "Dismiss (Group Exit)", "auto" => false}
        ] ++ base_actions

      data["unauthorized_paid"] == true ->
        # Person paid but still tailgated - possible tech issue
        [
          %{"id" => "check_payment_system", "label" => "Check Payment Integration", "auto" => false},
          %{"id" => "dismiss", "label" => "Dismiss (Paid Customer)", "auto" => false}
        ] ++ base_actions

      true ->
        # Standard tailgate - potential theft
        [
          %{"id" => "notify_security", "label" => "Notify Security", "auto" => true},
          %{"id" => "dismiss", "label" => "Dismiss", "auto" => false}
        ] ++ base_actions
    end
  end
end
