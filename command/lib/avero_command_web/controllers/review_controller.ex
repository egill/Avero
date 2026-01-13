defmodule AveroCommandWeb.ReviewController do
  @moduledoc """
  API endpoint for automated review of journeys and incidents.
  Used by Claude headless mode for anomaly detection.
  """
  use AveroCommandWeb, :controller

  import Ecto.Query
  alias AveroCommand.Repo
  alias AveroCommand.Journeys.Journey
  alias AveroCommand.Incidents.Incident

  @default_limit 20

  @doc """
  GET /api/review

  Returns recent journeys and incidents for automated review.

  Query params:
    - since: ISO8601 timestamp (optional, defaults to last hour)
    - limit: max journeys to return (optional, defaults to 20)

  Returns journeys that need review:
    - Unpaid exits
    - Journeys with potential data issues (missing gate_opened_by, payment_zone when paid, etc.)
  """
  def index(conn, params) do
    since = parse_since(params["since"])
    limit = parse_limit(params["limit"])

    journeys = fetch_journeys_for_review(since, limit)
    incidents = fetch_incidents_since(since)

    json(conn, %{
      query: %{
        since: DateTime.to_iso8601(since),
        limit: limit,
        fetched_at: DateTime.to_iso8601(DateTime.utc_now())
      },
      journeys: Enum.map(journeys, &format_journey/1),
      incidents: Enum.map(incidents, &format_incident/1),
      summary: %{
        journey_count: length(journeys),
        incident_count: length(incidents),
        unpaid_count: Enum.count(journeys, &(not &1.authorized)),
        missing_data_count: Enum.count(journeys, &has_missing_data?/1)
      }
    })
  end

  # Parse since parameter, default to 1 hour ago
  defp parse_since(nil), do: DateTime.add(DateTime.utc_now(), -3600, :second)

  defp parse_since(since_str) do
    case DateTime.from_iso8601(since_str) do
      {:ok, dt, _} -> dt
      _ -> DateTime.add(DateTime.utc_now(), -3600, :second)
    end
  end

  defp parse_limit(nil), do: @default_limit

  defp parse_limit(limit_str) do
    case Integer.parse(limit_str) do
      {n, ""} when n > 0 and n <= 500 -> n
      _ -> @default_limit
    end
  end

  # Fetch journeys that need review (unpaid or with potential issues)
  defp fetch_journeys_for_review(since, limit) do
    from(j in Journey,
      where: j.time >= ^since,
      where: j.exit_type in ["exit_confirmed", "tracking_lost_authorized"],
      order_by: [desc: j.time],
      limit: ^limit
    )
    |> Repo.all()
  rescue
    _ -> []
  end

  # Fetch active incidents since timestamp
  defp fetch_incidents_since(since) do
    from(i in Incident,
      where: i.created_at >= ^since,
      where: i.status in ["new", "acknowledged", "in_progress"],
      order_by: [desc: i.created_at]
    )
    |> Repo.all()
  rescue
    _ -> []
  end

  # Check if journey has missing data that should be present
  defp has_missing_data?(journey) do
    cond do
      # Paid but no payment zone recorded
      journey.authorized and is_nil(journey.payment_zone) ->
        true

      # Exited but no gate_opened_by
      journey.exit_type == "exit_confirmed" and is_nil(journey.gate_opened_by) ->
        true

      # Had POS dwell but no payment zone
      # This is expected for unpaid
      (journey.total_pos_dwell_ms && journey.total_pos_dwell_ms >= 7000) and
        is_nil(journey.payment_zone) and not journey.authorized ->
        false

      true ->
        false
    end
  end

  # Format journey for JSON output (exclude heavy events array by default)
  defp format_journey(journey) do
    # Extract key timing info from events
    events = journey.events || []
    gate_open_requested = find_event(events, "gate_open_requested")
    gate_opened = find_event(events, "gate_opened")
    last_pos_exit = find_last_pos_exit(events)
    exit_event = find_event(events, "exit")

    %{
      id: journey.id,
      person_id: journey.person_id,
      site: journey.site,
      time: journey.time && DateTime.to_iso8601(journey.time),
      duration_ms: journey.duration_ms,
      # Outcome
      outcome: journey.outcome,
      exit_type: journey.exit_type,
      authorized: journey.authorized,
      auth_method: journey.auth_method,
      # Gate
      gate_opened_by: journey.gate_opened_by,
      tailgated: journey.tailgated,
      # Payment
      payment_zone: journey.payment_zone,
      total_pos_dwell_ms: journey.total_pos_dwell_ms,
      dwell_threshold_met: journey.dwell_threshold_met,
      # Group
      is_group: journey.is_group,
      member_count: journey.member_count,
      # Derived timing (for review)
      # Gate Open: prefer gate_opened (RS485), fall back to gate_open_requested
      timing: %{
        gate_cmd_at: get_event_ts(gate_open_requested),
        gate_opened_at: get_event_ts(gate_opened) || get_event_ts(gate_open_requested),
        pos_exit_at: get_event_ts(last_pos_exit),
        exit_at: get_event_ts(exit_event)
      },
      # Data quality flags
      issues: detect_issues(journey)
    }
  end

  # Detect potential issues with the journey data
  defp detect_issues(journey) do
    issues = []

    issues = if not journey.authorized, do: ["unpaid_exit" | issues], else: issues

    issues =
      if journey.authorized and is_nil(journey.payment_zone),
        do: ["paid_no_payment_zone" | issues],
        else: issues

    issues =
      if journey.exit_type == "exit_confirmed" and is_nil(journey.gate_opened_by),
        do: ["exit_no_gate_opened_by" | issues],
        else: issues

    issues = if journey.tailgated, do: ["tailgated" | issues], else: issues

    issues =
      if journey.exit_type == "tracking_lost_authorized",
        do: ["paid_but_tracking_lost" | issues],
        else: issues

    # Check for missing gate timing in events
    events = journey.events || []
    has_gate_cmd = Enum.any?(events, &(&1["type"] == "gate_open_requested"))
    has_gate_opened = Enum.any?(events, &(&1["type"] == "gate_opened"))

    issues =
      if journey.exit_type == "exit_confirmed" and not has_gate_cmd and not journey.tailgated,
        do: ["missing_gate_cmd_event" | issues],
        else: issues

    issues =
      if journey.exit_type == "exit_confirmed" and not has_gate_opened and not journey.tailgated,
        do: ["missing_gate_opened_event" | issues],
        else: issues

    Enum.reverse(issues)
  end

  defp find_event(events, type) do
    Enum.find(events, &(&1["type"] == type))
  end

  defp find_last_pos_exit(events) do
    # Try zone_exit from POS zone first
    pos_exit =
      events
      |> Enum.filter(fn e ->
        e["type"] == "zone_exit" and
          is_binary(get_in(e, ["data", "zone"])) and
          String.starts_with?(get_in(e, ["data", "zone"]), "POS")
      end)
      |> List.last()

    # Fall back to acc_payment if no POS zone exit
    pos_exit || find_event(events, "acc_payment")
  end

  defp get_event_ts(nil), do: nil
  defp get_event_ts(%{"ts" => ts}), do: ts
  defp get_event_ts(_), do: nil

  # Format incident for JSON output
  defp format_incident(incident) do
    %{
      id: incident.id,
      type: incident.type,
      severity: incident.severity,
      category: incident.category,
      site: incident.site,
      status: incident.status,
      created_at: incident.created_at && DateTime.to_iso8601(incident.created_at),
      related_person_id: incident.related_person_id,
      context: incident.context
    }
  end
end
