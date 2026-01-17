defmodule AveroCommand.Store do
  @moduledoc """
  Storage functions for events, incidents, and site configs.
  """
  import Ecto.Query
  require Logger

  alias AveroCommand.Repo
  alias AveroCommand.Store.Event

  # ============================================
  # Events
  # ============================================

  @doc """
  Insert an event into the store.
  """
  def insert_event(event_data) when is_map(event_data) do
    %Event{}
    |> Event.changeset(event_data)
    |> Repo.insert()
  rescue
    e ->
      Logger.warning("Store.insert_event failed: #{Exception.format(:error, e, __STACKTRACE__)}")
      {:error, e}
  end

  @doc """
  Get recent events, optionally filtered by site.
  """
  def recent_events(limit \\ 100, site \\ nil) do
    query =
      from(e in Event,
        order_by: [desc: e.time],
        limit: ^limit
      )

    query =
      if site do
        from(e in query, where: e.site == ^site)
      else
        query
      end

    Repo.all(query)
  rescue
    e ->
      Logger.warning("Store.recent_events failed: #{Exception.format(:error, e, __STACKTRACE__)}")
      []
  end

  @doc """
  Search events by query string.
  """
  def search_events(query_string, limit \\ 100) when is_binary(query_string) do
    if String.trim(query_string) == "" do
      recent_events(limit)
    else
      pattern = "%#{query_string}%"

      from(e in Event,
        where:
          ilike(e.event_type, ^pattern) or
            ilike(e.site, ^pattern) or
            ilike(e.zone, ^pattern),
        order_by: [desc: e.time],
        limit: ^limit
      )
      |> Repo.all()
    end
  rescue
    e ->
      Logger.warning("Store.search_events failed: #{Exception.format(:error, e, __STACKTRACE__)}")
      []
  end

  @doc """
  Get events for a specific person.
  """
  def events_for_person(site, person_id, limit \\ 50) do
    from(e in Event,
      where: e.site == ^site and e.person_id == ^person_id,
      order_by: [desc: e.time],
      limit: ^limit
    )
    |> Repo.all()
  rescue
    e ->
      Logger.warning(
        "Store.events_for_person failed: #{Exception.format(:error, e, __STACKTRACE__)}"
      )

      []
  end

  @doc """
  Get events for a specific person with extended lookup.
  Handles type coercion (string vs integer person_id) and also
  checks the data JSONB field for person_id.
  """
  def events_for_person_extended(site, person_id, limit \\ 50) do
    # Coerce person_id to integer if it's a string
    person_id_int = coerce_to_integer(person_id)
    person_id_str = to_string(person_id)

    query =
      if person_id_int do
        # Query both the column (as integer) and JSONB (as string)
        from(e in Event,
          where:
            e.site == ^site and
              (e.person_id == ^person_id_int or
                 fragment("data->>'person_id' = ?", ^person_id_str)),
          order_by: [desc: e.time],
          limit: ^limit
        )
      else
        # person_id is not a valid integer, only check JSONB
        from(e in Event,
          where:
            e.site == ^site and
              fragment("data->>'person_id' = ?", ^person_id_str),
          order_by: [desc: e.time],
          limit: ^limit
        )
      end

    Repo.all(query)
  rescue
    e ->
      Logger.warning(
        "Store.events_for_person_extended failed: #{Exception.format(:error, e, __STACKTRACE__)}"
      )

      []
  end

  defp coerce_to_integer(val) when is_integer(val), do: val

  defp coerce_to_integer(val) when is_binary(val) do
    case Integer.parse(val) do
      {int, ""} -> int
      _ -> nil
    end
  end

  defp coerce_to_integer(_), do: nil

  @doc """
  Get events in a time range for a site.
  Useful for incident investigation.
  """
  def get_events_in_range(site, from_time, to_time, limit \\ 100) do
    from(e in Event,
      where: e.site == ^site and e.time >= ^from_time and e.time <= ^to_time,
      order_by: [asc: e.time],
      limit: ^limit
    )
    |> Repo.all()
  rescue
    e ->
      Logger.warning(
        "Store.get_events_in_range failed: #{Exception.format(:error, e, __STACKTRACE__)}"
      )

      []
  end

  @doc """
  Count backward line crossings (direction=backward) in a time range.
  Backward crossings indicate someone entering through the exit.
  """
  def count_backward_crossings(site, from_time, to_time) do
    from(e in Event,
      where: e.site == ^site,
      where: e.time >= ^from_time and e.time <= ^to_time,
      where: fragment("data->>'type' = 'xovis.line.cross' AND data->>'direction' = 'backward'"),
      select: count(e.id)
    )
    |> Repo.one()
  rescue
    e ->
      Logger.warning(
        "Store.count_backward_crossings failed: #{Exception.format(:error, e, __STACKTRACE__)}"
      )

      0
  end

  @doc """
  Get gate timing statistics for a site and date.

  Returns a map with:
  - total_cycles: number of gate.closed events
  - min_ms, max_ms, avg_ms: gate open duration statistics
  - long_openings: count of openings > 30s
  - by_gate: map of gate_id => %{cycles, avg_ms}
  """
  def get_gate_timing_stats(site, %Date{} = date) do
    start_dt = DateTime.new!(date, ~T[00:00:00], "Etc/UTC")
    end_dt = DateTime.new!(Date.add(date, 1), ~T[00:00:00], "Etc/UTC")

    # Get all gate.closed events (these contain open_duration_ms)
    gate_events =
      from(e in Event,
        where: e.site == ^site,
        where: e.time >= ^start_dt and e.time < ^end_dt,
        where: e.event_type == "gates"
      )
      |> Repo.all()
      |> Enum.filter(fn e -> e.data["type"] == "gate.closed" end)

    # Extract durations from events (duration_ms field or data.open_duration_ms)
    durations =
      gate_events
      |> Enum.map(fn e -> e.duration_ms || e.data["open_duration_ms"] || 0 end)
      |> Enum.reject(&(&1 == 0))

    # Calculate stats
    {min_ms, max_ms, avg_ms} =
      if length(durations) > 0 do
        {Enum.min(durations), Enum.max(durations), Enum.sum(durations) / length(durations)}
      else
        {0, 0, 0.0}
      end

    long_openings = Enum.count(durations, &(&1 > 30_000))

    # Per-gate breakdown
    by_gate =
      gate_events
      |> Enum.group_by(& &1.gate_id)
      |> Enum.map(fn {gate_id, events} ->
        durs = Enum.map(events, fn e -> e.duration_ms || e.data["open_duration_ms"] || 0 end)
        valid_durs = Enum.reject(durs, &(&1 == 0))
        avg = if length(valid_durs) > 0, do: Enum.sum(valid_durs) / length(valid_durs), else: 0.0
        {gate_id, %{cycles: length(events), avg_ms: round(avg)}}
      end)
      |> Map.new()

    %{
      total_cycles: length(gate_events),
      min_ms: min_ms,
      max_ms: max_ms,
      avg_ms: round(avg_ms),
      long_openings: long_openings,
      by_gate: by_gate
    }
  rescue
    e ->
      Logger.warning(
        "Store.get_gate_timing_stats failed: #{Exception.format(:error, e, __STACKTRACE__)}"
      )

      %{total_cycles: 0, min_ms: 0, max_ms: 0, avg_ms: 0, long_openings: 0, by_gate: %{}}
  end

  # ============================================
  # Site Configs
  # ============================================

  defmodule SiteConfig do
    use Ecto.Schema
    import Ecto.Changeset

    @primary_key {:site, :string, []}
    schema "site_configs" do
      field(:name, :string)
      field(:timezone, :string)
      field(:operating_hours, :map)
      field(:scenario_config, :map)
      field(:notification_config, :map)
      timestamps(inserted_at: :created_at)
    end

    def changeset(config, attrs) do
      config
      |> cast(attrs, [
        :site,
        :name,
        :timezone,
        :operating_hours,
        :scenario_config,
        :notification_config
      ])
      |> validate_required([:site, :name])
    end
  end

  @doc """
  List all site configurations.
  """
  def list_site_configs do
    Repo.all(SiteConfig)
  rescue
    e ->
      Logger.warning(
        "Store.list_site_configs failed: #{Exception.format(:error, e, __STACKTRACE__)}"
      )

      []
  end

  @doc """
  Get a site configuration by site ID.
  """
  def get_site_config(site) do
    Repo.get(SiteConfig, site)
  rescue
    e ->
      Logger.warning(
        "Store.get_site_config failed: #{Exception.format(:error, e, __STACKTRACE__)}"
      )

      nil
  end
end
