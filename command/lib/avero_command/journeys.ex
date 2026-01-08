defmodule AveroCommand.Journeys do
  @moduledoc """
  Context module for managing customer journeys.

  Provides functions to query and create journey records for
  tracking customer exits, returns, and lost tracking.
  """
  import Ecto.Query
  require Logger

  alias AveroCommand.Repo
  alias AveroCommand.Journeys.Journey

  @pubsub AveroCommand.PubSub

  # ============================================
  # Queries
  # ============================================

  @doc """
  List recent journeys, optionally filtered by sites.
  Returns the most recent N journeys.
  """
  def list_recent(opts \\ []) do
    sites = Keyword.get(opts, :sites)
    limit = Keyword.get(opts, :limit, 50)

    query =
      from(j in Journey,
        order_by: [desc: j.time],
        limit: ^limit
      )

    query = if sites && sites != [], do: where(query, [j], j.site in ^sites), else: query

    Repo.all(query)
  rescue
    e ->
      Logger.warning("Failed to list journeys: #{inspect(e)}")
      []
  end

  @doc """
  List journeys by exit type (exit_confirmed, tracking_lost, returned_to_store).
  """
  def list_by_exit_type(exit_type, opts \\ []) do
    sites = Keyword.get(opts, :sites)
    limit = Keyword.get(opts, :limit, 50)

    query =
      from(j in Journey,
        where: j.exit_type == ^exit_type,
        order_by: [desc: j.time],
        limit: ^limit
      )

    query = if sites && sites != [], do: where(query, [j], j.site in ^sites), else: query

    Repo.all(query)
  rescue
    _ -> []
  end

  @doc """
  List journeys with comprehensive filtering and cursor-based pagination.

  Options:
    - sites: list of site IDs to filter by
    - exit_type: atom (:all, :exits, :returns, :lost) or string
    - person_id: string or integer to search for
    - from_date: Date or ISO8601 string for start of range
    - to_date: Date or ISO8601 string for end of range
    - pos_filter: :all | :with_pos | :without_pos
    - pos_zones: list of specific payment zones to include
    - cursor: DateTime for cursor-based pagination
    - direction: :next | :prev (default: :next)
    - limit: number of results (default: 25)
  """
  def list_filtered(opts \\ []) do
    sites = Keyword.get(opts, :sites)
    exit_type = Keyword.get(opts, :exit_type)
    person_id = Keyword.get(opts, :person_id)
    from_date = Keyword.get(opts, :from_date)
    to_date = Keyword.get(opts, :to_date)
    from_datetime = Keyword.get(opts, :from_datetime)
    to_datetime = Keyword.get(opts, :to_datetime)
    pos_filter = Keyword.get(opts, :pos_filter, :all)
    pos_zones = Keyword.get(opts, :pos_zones, [])
    cursor = Keyword.get(opts, :cursor)
    direction = Keyword.get(opts, :direction, :next)
    limit = Keyword.get(opts, :limit, 25)

    from(j in Journey)
    |> apply_site_filter(sites)
    |> apply_exit_type_filter(exit_type)
    |> apply_person_id_filter(person_id)
    |> apply_date_range_filter(from_date, to_date)
    |> apply_datetime_range_filter(from_datetime, to_datetime)
    |> apply_pos_filter(pos_filter, pos_zones)
    |> apply_cursor_pagination(cursor, direction, limit)
    |> Repo.all()
    |> maybe_reverse(direction)
  rescue
    e ->
      Logger.warning("Failed to list filtered journeys: #{inspect(e)}")
      []
  end

  @doc """
  Returns list of distinct payment_zone values from journeys.
  Excludes nil values. Optionally filtered by sites.
  """
  def list_payment_zones(opts \\ []) do
    sites = Keyword.get(opts, :sites)

    query =
      from(j in Journey,
        where: not is_nil(j.payment_zone),
        distinct: true,
        select: j.payment_zone
      )

    query = if sites && sites != [], do: where(query, [j], j.site in ^sites), else: query

    Repo.all(query) |> Enum.sort()
  rescue
    _ -> []
  end

  # Query composition helpers

  defp apply_site_filter(query, nil), do: query
  defp apply_site_filter(query, []), do: query
  defp apply_site_filter(query, sites), do: where(query, [j], j.site in ^sites)

  defp apply_exit_type_filter(query, nil), do: query
  defp apply_exit_type_filter(query, :all), do: query
  defp apply_exit_type_filter(query, :exits), do: where(query, [j], j.exit_type == "exit_confirmed")
  defp apply_exit_type_filter(query, :returns), do: where(query, [j], j.exit_type == "returned_to_store")
  defp apply_exit_type_filter(query, :lost), do: where(query, [j], j.exit_type == "tracking_lost")
  defp apply_exit_type_filter(query, exit_type) when is_binary(exit_type) do
    where(query, [j], j.exit_type == ^exit_type)
  end

  defp apply_person_id_filter(query, nil), do: query
  defp apply_person_id_filter(query, ""), do: query
  defp apply_person_id_filter(query, person_id) when is_binary(person_id) do
    case Integer.parse(String.trim(person_id)) do
      {id, ""} -> where(query, [j], j.person_id == ^id)
      _ -> query
    end
  end
  defp apply_person_id_filter(query, person_id) when is_integer(person_id) do
    where(query, [j], j.person_id == ^person_id)
  end

  defp apply_date_range_filter(query, nil, nil), do: query
  defp apply_date_range_filter(query, from_date, nil) do
    case date_to_datetime_start(from_date) do
      nil -> query
      from_dt -> where(query, [j], j.time >= ^from_dt)
    end
  end
  defp apply_date_range_filter(query, nil, to_date) do
    case date_to_datetime_end(to_date) do
      nil -> query
      to_dt -> where(query, [j], j.time <= ^to_dt)
    end
  end
  defp apply_date_range_filter(query, from_date, to_date) do
    from_dt = date_to_datetime_start(from_date)
    to_dt = date_to_datetime_end(to_date)

    query
    |> then(fn q -> if from_dt, do: where(q, [j], j.time >= ^from_dt), else: q end)
    |> then(fn q -> if to_dt, do: where(q, [j], j.time <= ^to_dt), else: q end)
  end

  # DateTime range filter (more precise than date range)
  defp apply_datetime_range_filter(query, nil, nil), do: query
  defp apply_datetime_range_filter(query, %DateTime{} = from_dt, nil) do
    where(query, [j], j.time >= ^from_dt)
  end
  defp apply_datetime_range_filter(query, nil, %DateTime{} = to_dt) do
    where(query, [j], j.time <= ^to_dt)
  end
  defp apply_datetime_range_filter(query, %DateTime{} = from_dt, %DateTime{} = to_dt) do
    query
    |> where([j], j.time >= ^from_dt)
    |> where([j], j.time <= ^to_dt)
  end
  defp apply_datetime_range_filter(query, _, _), do: query

  # 7 seconds minimum dwell to count as POS stop
  @min_dwell_ms 7000

  defp apply_pos_filter(query, :all, []), do: query
  defp apply_pos_filter(query, :all, zones) when is_list(zones) and length(zones) > 0 do
    where(query, [j], j.payment_zone in ^zones)
  end
  # "With POS" = had meaningful time at a POS zone (>= 7s dwell)
  defp apply_pos_filter(query, :with_pos, []) do
    where(query, [j], j.total_pos_dwell_ms >= @min_dwell_ms)
  end
  defp apply_pos_filter(query, :with_pos, zones) when is_list(zones) and length(zones) > 0 do
    where(query, [j], j.payment_zone in ^zones)
  end
  # "No POS" = didn't spend meaningful time at any POS zone
  defp apply_pos_filter(query, :without_pos, _) do
    where(query, [j], is_nil(j.total_pos_dwell_ms) or j.total_pos_dwell_ms < @min_dwell_ms)
  end
  # "Unpaid with POS" = unpaid but had meaningful POS stop (potential theft/walkout)
  defp apply_pos_filter(query, :unpaid_with_pos, _) do
    where(query, [j], j.authorized == false and j.total_pos_dwell_ms >= @min_dwell_ms)
  end
  defp apply_pos_filter(query, _, _), do: query

  defp apply_cursor_pagination(query, nil, _direction, limit) do
    query
    |> order_by([j], desc: j.time, desc: j.id)
    |> limit(^(limit + 1))
  end
  defp apply_cursor_pagination(query, cursor, :next, limit) do
    query
    |> where([j], j.time < ^cursor)
    |> order_by([j], desc: j.time, desc: j.id)
    |> limit(^(limit + 1))
  end
  defp apply_cursor_pagination(query, cursor, :prev, limit) do
    query
    |> where([j], j.time > ^cursor)
    |> order_by([j], asc: j.time, asc: j.id)
    |> limit(^(limit + 1))
  end

  defp maybe_reverse(results, :prev), do: Enum.reverse(results)
  defp maybe_reverse(results, :next), do: results

  defp date_to_datetime_start(%Date{} = date) do
    DateTime.new!(date, ~T[00:00:00], "Etc/UTC")
  end
  defp date_to_datetime_start(date_string) when is_binary(date_string) do
    case Date.from_iso8601(date_string) do
      {:ok, date} -> date_to_datetime_start(date)
      _ -> nil
    end
  end
  defp date_to_datetime_start(_), do: nil

  defp date_to_datetime_end(%Date{} = date) do
    DateTime.new!(date, ~T[23:59:59.999999], "Etc/UTC")
  end
  defp date_to_datetime_end(date_string) when is_binary(date_string) do
    case Date.from_iso8601(date_string) do
      {:ok, date} -> date_to_datetime_end(date)
      _ -> nil
    end
  end
  defp date_to_datetime_end(_), do: nil

  @doc """
  Get a single journey by ID.
  """
  def get_journey(id) do
    Repo.get(Journey, id)
  rescue
    _ -> nil
  end

  @doc """
  Get a journey by session ID.
  """
  def get_by_session(session_id) do
    Repo.get_by(Journey, session_id: session_id)
  rescue
    _ -> nil
  end

  # ============================================
  # Mutations
  # ============================================

  @doc """
  Create a journey from gateway-poc JSON format.

  Gateway-poc publishes journeys with short keys:
  - jid: journey UUID
  - pid: person UUID (stable across stitches)
  - tids: array of track IDs [123, 456]
  - out: outcome ("exit", "abandoned")
  - auth: authorized (boolean)
  - dwell: total dwell ms
  - acc: ACC matched (boolean)
  - t0: started_at (epoch ms)
  - t1: ended_at (epoch ms)
  - ev: events array with {t, z, ts, x} format
  """
  def create_from_gateway_json(data) when is_map(data) do
    # Parse timestamps from epoch milliseconds
    started_at = parse_epoch_ms(data["t0"])
    ended_at = parse_epoch_ms(data["t1"])

    # Get first track ID as person_id (for display)
    person_id = case data["tids"] do
      [tid | _] when is_integer(tid) -> tid
      _ -> nil
    end

    # Transform events from short-key to long-key format
    events = transform_gateway_events(data["ev"] || [])

    # Extract POS info from transformed events
    {payment_zone, total_pos_dwell_ms, dwell_threshold_met, dwell_zone} = extract_pos_info(events)

    # Map outcome
    {outcome, exit_type} = map_gateway_outcome(data["out"], data["auth"])

    # Site can come from JSON or fall back to configured default
    default_site = Application.get_env(:avero_command, :default_gateway_site, "gateway-poc")
    site = data["site"] || default_site

    # Parse gate timing fields from epoch ms
    gate_cmd_at = parse_epoch_ms(data["gate_cmd"])
    gate_opened_at = parse_epoch_ms(data["gate_open"])

    attrs = %{
      time: ended_at || DateTime.utc_now(),
      site: site,
      person_id: person_id,
      session_id: data["jid"],

      # Timing
      started_at: started_at,
      ended_at: ended_at,
      duration_ms: data["dwell"],

      # Outcome
      outcome: outcome,
      exit_type: exit_type,
      authorized: data["auth"] || false,
      auth_method: nil,
      receipt_id: nil,

      # Gate details
      gate_opened_by: if(data["gate_was_open"], do: "already_open", else: nil),
      tailgated: false,
      gate_cmd_at: gate_cmd_at,
      gate_opened_at: gate_opened_at,

      # ACC tracking
      acc_matched: data["acc"] || false,

      # Payment details
      payment_zone: payment_zone,
      total_pos_dwell_ms: total_pos_dwell_ms,

      # Dwell tracking
      dwell_threshold_met: dwell_threshold_met,
      dwell_zone: dwell_zone,

      # Group tracking
      is_group: false,
      member_count: 1,
      group_id: nil,

      # Full data - store transformed events
      zones_visited: extract_zones_visited(events),
      events: events
    }

    create_journey(attrs)
  end

  # Transform gateway-poc short-key events to long-key format
  defp transform_gateway_events(events) when is_list(events) do
    Enum.map(events, fn event ->
      ts = case event["ts"] do
        ts when is_integer(ts) ->
          DateTime.from_unix!(ts, :millisecond) |> DateTime.to_iso8601()
        _ -> nil
      end

      base = %{
        "type" => event["t"],
        "ts" => ts
      }

      # Add zone if present
      base = if event["z"], do: Map.put(base, "data", %{"zone" => event["z"]}), else: base

      # Parse extra field (x) which contains key=value pairs like "dwell=7500"
      base = if event["x"] do
        extra_data = parse_extra_field(event["x"])
        existing_data = base["data"] || %{}
        Map.put(base, "data", Map.merge(existing_data, extra_data))
      else
        base
      end

      base
    end)
  end

  defp transform_gateway_events(_), do: []

  # Parse "dwell=7500,foo=bar" format into map
  # Normalizes keys: dwell -> dwell_ms
  defp parse_extra_field(extra) when is_binary(extra) do
    extra
    |> String.split(",")
    |> Enum.map(fn pair ->
      case String.split(pair, "=", parts: 2) do
        [key, value] ->
          # Normalize key names
          normalized_key = case key do
            "dwell" -> "dwell_ms"
            other -> other
          end
          # Try to parse as integer
          parsed_value = case Integer.parse(value) do
            {int, ""} -> int
            _ -> value
          end
          {normalized_key, parsed_value}
        _ -> nil
      end
    end)
    |> Enum.reject(&is_nil/1)
    |> Map.new()
  end

  defp parse_extra_field(_), do: %{}

  # Map gateway-poc outcome to command format
  defp map_gateway_outcome("exit", true), do: {"paid_exit", "exit_confirmed"}
  defp map_gateway_outcome("exit", false), do: {"unpaid_exit", "exit_confirmed"}
  defp map_gateway_outcome("abandoned", _), do: {"lost_unauthorized", "tracking_lost"}
  defp map_gateway_outcome("in_progress", _), do: {"in_progress", "tracking_lost"}
  defp map_gateway_outcome(_, true), do: {"paid_exit", "exit_confirmed"}
  defp map_gateway_outcome(_, false), do: {"unpaid_exit", "exit_confirmed"}

  # Parse epoch milliseconds to DateTime
  defp parse_epoch_ms(nil), do: nil
  defp parse_epoch_ms(ms) when is_integer(ms), do: DateTime.from_unix!(ms, :millisecond)
  defp parse_epoch_ms(_), do: nil

  @doc """
  Create a journey from a legacy journey.completed event.
  Extracts relevant fields from the event data.
  """
  def create_from_event(event) do
    data = event.data || %{}
    events = data["events"] || []

    # Extract payment zone and dwell info from journey events
    {payment_zone, total_pos_dwell_ms, dwell_threshold_met, dwell_zone} = extract_pos_info(events)

    attrs = %{
      time: event.time || DateTime.utc_now(),
      site: event.site,
      person_id: event.person_id,
      session_id: data["session_id"],

      # Timing (fallback to event.time if started_at not provided by gateway)
      started_at: parse_timestamp(data["started_at"]) || event.time,
      ended_at: event.time,
      duration_ms: data["total_dwell_ms"],

      # Outcome
      outcome: determine_outcome(data),
      exit_type: data["exit_type"] || "exit_confirmed",
      authorized: data["authorized"] || event.authorized || false,
      auth_method: data["auth_method"] || event.auth_method,
      receipt_id: data["receipt_id"],

      # Gate details
      gate_opened_by: data["gate_opened_by"],
      tailgated: data["tailgated"] || false,

      # Payment details
      payment_zone: payment_zone,
      total_pos_dwell_ms: total_pos_dwell_ms,

      # Dwell tracking
      dwell_threshold_met: dwell_threshold_met,
      dwell_zone: dwell_zone,

      # Group tracking (from Xovis GROUP tracks)
      is_group: data["is_group"] || false,
      member_count: data["member_count"] || 1,
      group_id: data["group_id"],

      # Full data
      zones_visited: extract_zones_visited(events),
      events: events
    }

    create_journey(attrs)
  end

  @doc """
  Create a journey with the given attributes.
  """
  def create_journey(attrs) do
    %Journey{}
    |> Journey.changeset(attrs)
    |> Repo.insert()
    |> case do
      {:ok, journey} ->
        broadcast(:journey_created, journey)
        {:ok, journey}

      {:error, changeset} ->
        Logger.warning("Failed to create journey: #{inspect(changeset.errors)}")
        {:error, changeset}
    end
  rescue
    e ->
      Logger.error("Exception creating journey: #{inspect(e)}")
      {:error, :database_error}
  end

  # ============================================
  # Helpers
  # ============================================

  defp extract_pos_info(events) when is_list(events) do
    # Find payment event to get payment zone
    payment_event = Enum.find(events, fn e ->
      e["type"] == "payment"
    end)
    payment_zone = get_in(payment_event, ["data", "zone"])

    # Find dwell threshold event
    # NOTE: Go sends "dwell_threshold" (not "dwell_met") - see journey.go:RecordDwellThreshold
    dwell_event = Enum.find(events, fn e ->
      e["type"] == "dwell_threshold"
    end)
    dwell_zone = get_in(dwell_event, ["data", "zone"])

    # Calculate total POS zone dwell time from zone exits
    total_pos_dwell_ms = events
    |> Enum.filter(fn e ->
      e["type"] == "zone_exit" and
      is_pos_zone?(get_in(e, ["data", "zone"]))
    end)
    |> Enum.map(fn e -> get_in(e, ["data", "dwell_ms"]) || 0 end)
    |> Enum.sum()

    # Dwell threshold met if: explicit event OR calculated dwell >= 7000ms
    dwell_threshold_met = dwell_event != nil or total_pos_dwell_ms >= 7000

    {payment_zone, total_pos_dwell_ms, dwell_threshold_met, dwell_zone}
  end

  defp extract_pos_info(_), do: {nil, 0, false, nil}

  defp extract_zones_visited(events) when is_list(events) do
    events
    |> Enum.filter(fn e -> e["type"] in ["zone_entry", "zone_exit"] end)
    |> Enum.reduce(%{}, fn e, acc ->
      zone = get_in(e, ["data", "zone"])
      if zone do
        Map.update(acc, zone, %{zone: zone, visits: 1}, fn z ->
          %{z | visits: z.visits + 1}
        end)
      else
        acc
      end
    end)
    |> Map.values()
  end

  defp extract_zones_visited(_), do: []

  defp is_pos_zone?(zone) when is_binary(zone), do: String.starts_with?(zone, "POS")
  defp is_pos_zone?(_), do: false

  defp determine_outcome(data) do
    exit_type = data["exit_type"]
    authorized = data["authorized"]

    cond do
      exit_type == "returned_to_store" -> "returned"
      exit_type == "tracking_lost" and authorized -> "lost_authorized"
      exit_type == "tracking_lost" -> "lost_unauthorized"
      authorized -> "paid_exit"
      true -> "unpaid_exit"
    end
  end

  defp parse_timestamp(nil), do: nil
  defp parse_timestamp(ts) when is_binary(ts) do
    case DateTime.from_iso8601(ts) do
      {:ok, dt, _} -> dt
      _ -> nil
    end
  end
  defp parse_timestamp(%DateTime{} = dt), do: dt
  defp parse_timestamp(_), do: nil

  defp broadcast(event, journey) do
    Phoenix.PubSub.broadcast(@pubsub, "journeys", {event, journey})
  end
end
