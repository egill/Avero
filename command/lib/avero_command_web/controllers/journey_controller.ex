defmodule AveroCommandWeb.JourneyController do
  @moduledoc """
  API endpoint for querying customer journeys.
  Used by the chat-journeys CLI tool for natural language journey analysis.
  """
  use AveroCommandWeb, :controller

  alias AveroCommand.Journeys

  @default_limit 50
  @max_limit 500

  @doc """
  GET /api/journeys

  Query journeys with comprehensive filtering.

  Query params:
    - site: Filter by site (netto, grandi)
    - exit_type: Filter by exit type (exit_confirmed, tracking_lost, returned_to_store)
                 Or aliases: exits, lost, returns
    - since: ISO8601 timestamp or date (YYYY-MM-DD) for start of range
    - until: ISO8601 timestamp or date for end of range
    - person_id: Filter by person/track ID
    - min_dwell: Minimum POS dwell in milliseconds
    - has_acc: Filter by ACC match status (true/false)
    - pos_filter: all | with_pos | without_pos | unpaid_with_pos
    - limit: Max results (default 50, max 500)
    - cursor: ISO8601 timestamp for pagination

  Returns:
    - journeys: Array of journey objects
    - pagination: Cursor info for next/prev pages
    - query: Echo of applied filters
  """
  def index(conn, params) do
    opts = build_query_opts(params)
    limit = parse_limit(params["limit"])

    journeys = Journeys.list_filtered(Keyword.put(opts, :limit, limit))

    # Check if there are more results (we fetch limit+1)
    {journeys, has_more} =
      if length(journeys) > limit do
        {Enum.take(journeys, limit), true}
      else
        {journeys, false}
      end

    # Build pagination cursors
    pagination = build_pagination(journeys, has_more, params["cursor"])

    json(conn, %{
      journeys: Enum.map(journeys, &format_journey/1),
      pagination: pagination,
      query: %{
        filters: summarize_filters(params),
        limit: limit,
        fetched_at: DateTime.to_iso8601(DateTime.utc_now())
      },
      count: length(journeys)
    })
  end

  @doc """
  GET /api/journeys/:id

  Get a single journey by ID with full event details.
  """
  def show(conn, %{"id" => id}) do
    case Journeys.get_journey(id) do
      nil ->
        conn
        |> put_status(:not_found)
        |> json(%{error: "Journey not found", id: id})

      journey ->
        json(conn, %{journey: format_journey_full(journey)})
    end
  end

  @doc """
  GET /api/journeys/by-session/:session_id

  Get a journey by session/journey ID (jid from gateway-poc).
  """
  def by_session(conn, %{"session_id" => session_id}) do
    case Journeys.get_by_session(session_id) do
      nil ->
        conn
        |> put_status(:not_found)
        |> json(%{error: "Journey not found", session_id: session_id})

      journey ->
        json(conn, %{journey: format_journey_full(journey)})
    end
  end

  @doc """
  GET /api/journeys/stats

  Get aggregate statistics for journeys matching filters.
  """
  def stats(conn, params) do
    opts = build_query_opts(params)
    # For stats, get more journeys to compute aggregates
    journeys = Journeys.list_filtered(Keyword.put(opts, :limit, 1000))

    stats = compute_stats(journeys)

    json(conn, %{
      stats: stats,
      query: %{
        filters: summarize_filters(params),
        sample_size: length(journeys),
        fetched_at: DateTime.to_iso8601(DateTime.utc_now())
      }
    })
  end

  # ============================================
  # Query Building
  # ============================================

  defp build_query_opts(params) do
    []
    |> maybe_add_site(params["site"])
    |> maybe_add_exit_type(params["exit_type"])
    |> maybe_add_person_id(params["person_id"])
    |> maybe_add_date_range(params["since"], params["until"])
    |> maybe_add_pos_filter(params["pos_filter"], params["min_dwell"], params["has_acc"])
    |> maybe_add_cursor(params["cursor"], params["direction"])
  end

  defp maybe_add_site(opts, nil), do: opts
  defp maybe_add_site(opts, ""), do: opts

  defp maybe_add_site(opts, site) do
    # Support comma-separated sites
    sites = String.split(site, ",") |> Enum.map(&String.trim/1)
    Keyword.put(opts, :sites, sites)
  end

  defp maybe_add_exit_type(opts, nil), do: opts
  defp maybe_add_exit_type(opts, ""), do: opts
  defp maybe_add_exit_type(opts, "all"), do: opts

  defp maybe_add_exit_type(opts, exit_type) do
    # Map aliases to atoms
    exit_type_atom =
      case exit_type do
        "exits" -> :exits
        "lost" -> :lost
        "returns" -> :returns
        other -> other
      end

    Keyword.put(opts, :exit_type, exit_type_atom)
  end

  defp maybe_add_person_id(opts, nil), do: opts
  defp maybe_add_person_id(opts, ""), do: opts
  defp maybe_add_person_id(opts, person_id), do: Keyword.put(opts, :person_id, person_id)

  defp maybe_add_date_range(opts, nil, nil), do: opts

  defp maybe_add_date_range(opts, since, until_param) do
    opts
    |> then(fn o -> if since, do: add_since_filter(o, since), else: o end)
    |> then(fn o -> if until_param, do: add_until_filter(o, until_param), else: o end)
  end

  defp add_since_filter(opts, since) do
    case parse_datetime_or_date(since) do
      {:datetime, dt} -> Keyword.put(opts, :from_datetime, dt)
      {:date, date} -> Keyword.put(opts, :from_date, date)
      :error -> opts
    end
  end

  defp add_until_filter(opts, until_param) do
    case parse_datetime_or_date(until_param) do
      {:datetime, dt} -> Keyword.put(opts, :to_datetime, dt)
      {:date, date} -> Keyword.put(opts, :to_date, date)
      :error -> opts
    end
  end

  defp parse_datetime_or_date(str) do
    # Try ISO8601 datetime first
    case DateTime.from_iso8601(str) do
      {:ok, dt, _} ->
        {:datetime, dt}

      _ ->
        # Try date
        case Date.from_iso8601(str) do
          {:ok, date} -> {:date, date}
          _ -> :error
        end
    end
  end

  defp maybe_add_pos_filter(opts, pos_filter, min_dwell, has_acc) do
    opts
    |> then(fn o ->
      case pos_filter do
        "with_pos" -> Keyword.put(o, :pos_filter, :with_pos)
        "without_pos" -> Keyword.put(o, :pos_filter, :without_pos)
        "unpaid_with_pos" -> Keyword.put(o, :pos_filter, :unpaid_with_pos)
        _ -> o
      end
    end)
    |> then(fn o ->
      # min_dwell implies with_pos filter if not explicitly set
      if min_dwell && !Keyword.has_key?(o, :pos_filter) do
        Keyword.put(o, :pos_filter, :with_pos)
      else
        o
      end
    end)
    |> then(fn o ->
      # has_acc filter - need to handle this specially since list_filtered doesn't support it directly
      # For now, we'll filter in-memory (could add to context later)
      if has_acc do
        Keyword.put(o, :has_acc, parse_bool(has_acc))
      else
        o
      end
    end)
  end

  defp maybe_add_cursor(opts, nil, _), do: opts

  defp maybe_add_cursor(opts, cursor, direction) do
    case DateTime.from_iso8601(cursor) do
      {:ok, dt, _} ->
        dir = if direction == "prev", do: :prev, else: :next

        opts
        |> Keyword.put(:cursor, dt)
        |> Keyword.put(:direction, dir)

      _ ->
        opts
    end
  end

  defp parse_limit(nil), do: @default_limit

  defp parse_limit(limit_str) do
    case Integer.parse(limit_str) do
      {n, ""} when n > 0 -> min(n, @max_limit)
      _ -> @default_limit
    end
  end

  defp parse_bool("true"), do: true
  defp parse_bool("false"), do: false
  defp parse_bool("1"), do: true
  defp parse_bool("0"), do: false
  defp parse_bool(_), do: nil

  # ============================================
  # Response Formatting
  # ============================================

  defp format_journey(journey) do
    %{
      id: journey.id,
      session_id: journey.session_id,
      person_id: journey.person_id,
      site: journey.site,
      time: format_datetime(journey.time),

      # Timing
      started_at: format_datetime(journey.started_at),
      ended_at: format_datetime(journey.ended_at),
      duration_ms: journey.duration_ms,

      # Outcome
      outcome: journey.outcome,
      exit_type: journey.exit_type,
      authorized: journey.authorized,
      auth_method: journey.auth_method,

      # Gate
      gate_opened_by: journey.gate_opened_by,
      tailgated: journey.tailgated,
      gate_cmd_at: format_datetime(journey.gate_cmd_at),
      gate_opened_at: format_datetime(journey.gate_opened_at),

      # ACC
      acc_matched: journey.acc_matched,

      # Payment/POS
      payment_zone: journey.payment_zone,
      total_pos_dwell_ms: journey.total_pos_dwell_ms,
      dwell_threshold_met: journey.dwell_threshold_met,
      dwell_zone: journey.dwell_zone,

      # Group
      is_group: journey.is_group,
      member_count: journey.member_count,

      # Zones visited (summary)
      zones_visited: journey.zones_visited
    }
  end

  # Full format includes events array
  defp format_journey_full(journey) do
    journey
    |> format_journey()
    |> Map.put(:events, journey.events || [])
  end

  defp format_datetime(nil), do: nil
  defp format_datetime(%DateTime{} = dt), do: DateTime.to_iso8601(dt)
  defp format_datetime(other), do: other

  defp build_pagination(journeys, has_more, _current_cursor) do
    case journeys do
      [] ->
        %{has_next: false, has_prev: false, next_cursor: nil, prev_cursor: nil}

      journeys ->
        first = List.first(journeys)
        last = List.last(journeys)

        %{
          has_next: has_more,
          # Would need reverse query to determine
          has_prev: false,
          next_cursor: if(has_more && last, do: format_datetime(last.time), else: nil),
          prev_cursor: if(first, do: format_datetime(first.time), else: nil)
        }
    end
  end

  defp summarize_filters(params) do
    params
    |> Map.take([
      "site",
      "exit_type",
      "since",
      "until",
      "person_id",
      "pos_filter",
      "min_dwell",
      "has_acc"
    ])
    |> Enum.reject(fn {_, v} -> is_nil(v) or v == "" end)
    |> Map.new()
  end

  # ============================================
  # Statistics
  # ============================================

  defp compute_stats(journeys) do
    total = length(journeys)

    if total == 0 do
      %{total: 0}
    else
      %{
        total: total,
        by_exit_type: count_by(journeys, & &1.exit_type),
        by_outcome: count_by(journeys, & &1.outcome),
        authorized_count: Enum.count(journeys, & &1.authorized),
        unauthorized_count: Enum.count(journeys, &(not &1.authorized)),
        acc_matched_count: Enum.count(journeys, & &1.acc_matched),
        with_pos_dwell: Enum.count(journeys, &((&1.total_pos_dwell_ms || 0) >= 7000)),
        avg_duration_ms: safe_avg(journeys, & &1.duration_ms),
        avg_pos_dwell_ms: safe_avg(journeys, & &1.total_pos_dwell_ms)
      }
    end
  end

  defp count_by(journeys, key_fn) do
    journeys
    |> Enum.group_by(key_fn)
    |> Enum.map(fn {k, v} -> {k || "unknown", length(v)} end)
    |> Map.new()
  end

  defp safe_avg(journeys, value_fn) do
    values = journeys |> Enum.map(value_fn) |> Enum.reject(&is_nil/1)

    if length(values) > 0 do
      (Enum.sum(values) / length(values)) |> round()
    else
      nil
    end
  end
end
