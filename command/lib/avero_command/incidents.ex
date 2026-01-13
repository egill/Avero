defmodule AveroCommand.Incidents do
  @moduledoc """
  Context module for managing incidents.
  """
  import Ecto.Query
  require Logger

  alias AveroCommand.Repo
  alias AveroCommand.Incidents.Incident

  @pubsub AveroCommand.PubSub

  # ============================================
  # Queries
  # ============================================

  @doc """
  List all active incidents (not resolved/dismissed).
  Optionally filter by sites.
  """
  def list_active(opts \\ []) do
    sites = Keyword.get(opts, :sites)

    query =
      from(i in Incident,
        where: i.status in ["new", "acknowledged", "in_progress"],
        order_by: [desc: i.created_at]
      )

    query = if sites && sites != [], do: where(query, [i], i.site in ^sites), else: query

    Repo.all(query)
  rescue
    _ -> []
  end

  @doc """
  List incidents by severity, optionally filtered by sites.
  """
  def list_by_severity(severity, opts \\ []) do
    sites = Keyword.get(opts, :sites)

    query =
      from(i in Incident,
        where: i.severity == ^severity,
        where: i.status in ["new", "acknowledged", "in_progress"],
        order_by: [desc: i.created_at]
      )

    query = if sites && sites != [], do: where(query, [i], i.site in ^sites), else: query

    Repo.all(query)
  rescue
    _ -> []
  end

  @doc """
  List incidents by status, optionally filtered by sites.
  """
  def list_by_status(status, opts \\ []) do
    sites = Keyword.get(opts, :sites)

    query =
      from(i in Incident,
        where: i.status == ^status,
        order_by: [desc: i.created_at]
      )

    query = if sites && sites != [], do: where(query, [i], i.site in ^sites), else: query

    Repo.all(query)
  rescue
    _ -> []
  end

  @doc """
  List incidents grouped by hour for a specific date and sites.
  Returns a list of maps with hour and severity counts.
  """
  def list_by_hour(date, sites) do
    from(i in Incident,
      where: i.site in ^sites,
      where: fragment("DATE(?)", i.created_at) == ^date,
      group_by: fragment("EXTRACT(HOUR FROM ?)", i.created_at),
      select: %{
        hour: fragment("EXTRACT(HOUR FROM ?)::integer", i.created_at),
        high: count(fragment("CASE WHEN ? = 'high' THEN 1 END", i.severity)),
        medium: count(fragment("CASE WHEN ? = 'medium' THEN 1 END", i.severity)),
        info: count(fragment("CASE WHEN ? NOT IN ('high', 'medium') THEN 1 END", i.severity)),
        total: count(i.id)
      },
      order_by: fragment("EXTRACT(HOUR FROM ?)", i.created_at)
    )
    |> Repo.all()
  rescue
    _ -> []
  end

  @doc """
  List incidents by type for a specific date, hour, and sites.
  Returns a map of type => count.
  """
  def list_by_type_for_hour(date, hour, sites) do
    from(i in Incident,
      where: i.site in ^sites,
      where: fragment("DATE(?)", i.created_at) == ^date,
      where: fragment("EXTRACT(HOUR FROM ?)", i.created_at) == ^hour,
      group_by: i.type,
      select: {i.type, count(i.id)}
    )
    |> Repo.all()
    |> Map.new()
  rescue
    _ -> %{}
  end

  @doc """
  List incidents for a specific hour with full details.
  """
  def list_for_hour(date, hour, sites) do
    from(i in Incident,
      where: i.site in ^sites,
      where: fragment("DATE(?)", i.created_at) == ^date,
      where: fragment("EXTRACT(HOUR FROM ?)", i.created_at) == ^hour,
      order_by: [desc: i.created_at]
    )
    |> Repo.all()
  rescue
    _ -> []
  end

  @doc """
  List incidents grouped by day for a week starting from the given date.
  """
  def list_by_day_for_week(start_date, sites) do
    end_date = Date.add(start_date, 6)

    from(i in Incident,
      where: i.site in ^sites,
      where: fragment("DATE(?)", i.created_at) >= ^start_date,
      where: fragment("DATE(?)", i.created_at) <= ^end_date,
      group_by: fragment("DATE(?)", i.created_at),
      select: %{
        date: fragment("DATE(?)::date", i.created_at),
        high: count(fragment("CASE WHEN ? = 'high' THEN 1 END", i.severity)),
        medium: count(fragment("CASE WHEN ? = 'medium' THEN 1 END", i.severity)),
        info: count(fragment("CASE WHEN ? NOT IN ('high', 'medium') THEN 1 END", i.severity)),
        total: count(i.id)
      },
      order_by: fragment("DATE(?)", i.created_at)
    )
    |> Repo.all()
  rescue
    _ -> []
  end

  @doc """
  Get a single incident by ID.
  """
  def get(id) do
    Repo.get(Incident, id)
  rescue
    _ -> nil
  end

  @doc """
  Get daily incident statistics for a site and date.

  Returns a map with:
  - total: total incidents
  - high, medium, info: counts by severity
  - gate_faults: count of gate_fault + gate_stuck incidents
  - tailgating: count of tailgating incidents
  - by_type: map of type => count
  - top_types: list of {type, count} sorted by count descending
  """
  def get_daily_incident_stats(site, %Date{} = date) do
    incidents =
      from(i in Incident,
        where: i.site == ^site,
        where: fragment("DATE(?)", i.created_at) == ^date
      )
      |> Repo.all()

    # Count by severity
    high = Enum.count(incidents, &(&1.severity == "high"))
    medium = Enum.count(incidents, &(&1.severity == "medium"))
    info = length(incidents) - high - medium

    # Count specific types
    gate_faults = Enum.count(incidents, &(&1.type in ["gate_fault", "gate_stuck"]))
    tailgating = Enum.count(incidents, &(&1.type == "tailgating"))

    # Group by type
    by_type =
      incidents
      |> Enum.group_by(& &1.type)
      |> Enum.map(fn {type, incs} -> {type, length(incs)} end)
      |> Map.new()

    # Top types sorted by count
    top_types =
      by_type
      |> Enum.sort_by(fn {_type, count} -> -count end)
      |> Enum.take(5)

    %{
      total: length(incidents),
      high: high,
      medium: medium,
      info: info,
      gate_faults: gate_faults,
      tailgating: tailgating,
      by_type: by_type,
      top_types: top_types
    }
  rescue
    e ->
      Logger.warning("Failed to get daily incident stats: #{inspect(e)}")

      %{
        total: 0,
        high: 0,
        medium: 0,
        info: 0,
        gate_faults: 0,
        tailgating: 0,
        by_type: %{},
        top_types: []
      }
  end

  # ============================================
  # Mutations
  # ============================================

  @doc """
  Create a new incident.
  """
  def create(attrs) do
    %Incident{}
    |> Incident.changeset(attrs)
    |> Repo.insert()
    |> broadcast(:incident_created)
  end

  @doc """
  Acknowledge an incident.
  """
  def acknowledge(id, user \\ "system") do
    case get(id) do
      nil ->
        {:error, :not_found}

      incident ->
        incident
        |> Incident.changeset(%{
          status: "acknowledged",
          acknowledged_at: DateTime.utc_now(),
          acknowledged_by: user
        })
        |> Repo.update()
        |> broadcast(:incident_updated)
    end
  end

  @doc """
  Resolve an incident.
  """
  def resolve(id, resolution, user \\ "system") do
    case get(id) do
      nil ->
        {:error, :not_found}

      incident ->
        status = if resolution == "dismissed", do: "dismissed", else: "resolved"

        incident
        |> Incident.changeset(%{
          status: status,
          resolution: resolution,
          resolved_at: DateTime.utc_now(),
          resolved_by: user
        })
        |> Repo.update()
        |> broadcast(:incident_updated)
    end
  end

  @doc """
  Dismiss all active incidents for given sites.
  """
  def dismiss_all(sites, user \\ "system") do
    now = DateTime.utc_now()

    {count, _} =
      from(i in Incident,
        where: i.status in ["new", "acknowledged", "in_progress"],
        where: i.site in ^sites
      )
      |> Repo.update_all(
        set: [
          status: "dismissed",
          resolution: "bulk_dismissed",
          resolved_at: now,
          resolved_by: user
        ]
      )

    # Broadcast update (refresh will pick up changes)
    Phoenix.PubSub.broadcast(@pubsub, "incidents", :incidents_cleared)
    {:ok, count}
  rescue
    e -> {:error, e}
  end

  @doc """
  Add an executed action to an incident.
  """
  def add_action(id, action_type, result, user \\ "system") do
    case get(id) do
      nil ->
        {:error, :not_found}

      incident ->
        action = %{
          type: action_type,
          result: result,
          executed_by: user,
          executed_at: DateTime.utc_now() |> DateTime.to_iso8601()
        }

        executed = [action | incident.executed_actions]

        incident
        |> Incident.changeset(%{executed_actions: executed})
        |> Repo.update()
        |> broadcast(:incident_updated)
    end
  end

  # ============================================
  # PubSub
  # ============================================

  def subscribe do
    Phoenix.PubSub.subscribe(@pubsub, "incidents")
  end

  defp broadcast({:ok, incident} = result, event) do
    Phoenix.PubSub.broadcast(@pubsub, "incidents", {event, incident})
    Phoenix.PubSub.broadcast(@pubsub, "incidents:#{incident.id}", {event, incident})
    result
  end

  defp broadcast({:error, _} = result, _event), do: result
end
