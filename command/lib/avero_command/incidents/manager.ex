defmodule AveroCommand.Incidents.Manager do
  @moduledoc """
  GenServer for managing incident lifecycle.
  Handles escalation, auto-actions, and deduplication.
  """
  use GenServer
  require Logger
  import Ecto.Query

  alias AveroCommand.Repo
  alias AveroCommand.Incidents
  alias AveroCommand.Incidents.Incident
  alias AveroCommand.Metrics

  # Check every minute
  @escalation_check_interval 60_000
  # 5 minute deduplication window
  @dedup_window_seconds 300

  def start_link(opts \\ []) do
    GenServer.start_link(__MODULE__, opts, name: __MODULE__)
  end

  @doc """
  Create a new incident from scenario detection.
  """
  def create_incident(attrs) do
    GenServer.call(__MODULE__, {:create_incident, attrs})
  end

  # ============================================
  # GenServer Callbacks
  # ============================================

  @impl true
  def init(_opts) do
    Logger.info("Incident Manager started")
    schedule_escalation_check()
    {:ok, %{}}
  end

  @impl true
  def handle_call({:create_incident, attrs}, _from, state) do
    # Check for duplicate/similar recent incident
    case check_duplicate(attrs) do
      {:duplicate, existing_id} ->
        Logger.debug("Duplicate incident detected, skipping: #{existing_id}")
        Metrics.inc_incident_duplicate(attrs[:type], attrs[:site])
        {:reply, {:ok, :duplicate}, state}

      :ok ->
        case Incidents.create(attrs) do
          {:ok, incident} ->
            Logger.info(
              "Incident created: #{incident.type} (#{incident.severity}) at #{incident.site}"
            )

            Metrics.inc_incident_created(
              incident.type,
              incident.severity,
              incident.category,
              incident.site
            )

            update_active_incidents_gauge()
            execute_auto_actions(incident)
            {:reply, {:ok, incident}, state}

          {:error, reason} ->
            Logger.warning("Failed to create incident: #{inspect(reason)}")
            {:reply, {:error, reason}, state}
        end
    end
  end

  @impl true
  def handle_info(:check_escalation, state) do
    check_for_escalations()
    schedule_escalation_check()
    {:noreply, state}
  end

  # ============================================
  # Private Functions
  # ============================================

  defp schedule_escalation_check do
    Process.send_after(self(), :check_escalation, @escalation_check_interval)
  end

  defp check_duplicate(%{type: type, site: site} = attrs) do
    # Use targeted database query instead of fetching all incidents
    gate_id = attrs[:gate_id] || 0
    person_id = attrs[:related_person_id]
    cutoff = DateTime.add(DateTime.utc_now(), -@dedup_window_seconds, :second)

    query =
      from(i in Incident,
        where: i.type == ^type and i.site == ^site and i.created_at > ^cutoff,
        where: i.status in ["new", "acknowledged", "in_progress"],
        limit: 1,
        select: i.id
      )

    # Add person/gate filtering based on what identifiers we have
    query =
      cond do
        # Match by person if present
        not is_nil(person_id) ->
          where(query, [i], i.related_person_id == ^person_id)

        # Match by gate_id for equipment incidents
        gate_id > 0 ->
          where(query, [i], i.gate_id == ^gate_id)

        # For site-wide incidents (gate_id=0, no person), just match type+site
        true ->
          query
      end

    case Repo.one(query) do
      nil -> :ok
      id -> {:duplicate, id}
    end
  rescue
    e ->
      Logger.warning("check_duplicate failed: #{Exception.format(:error, e, __STACKTRACE__)}")
      # Allow incident creation on error
      :ok
  end

  defp check_duplicate(_attrs), do: :ok

  defp execute_auto_actions(incident) do
    auto_actions =
      incident.suggested_actions
      |> Enum.filter(fn a -> a["auto"] == true end)

    Enum.each(auto_actions, fn action ->
      Logger.info("Auto-executing action: #{action["id"]} for incident #{incident.id}")
      # TODO: Implement actual action execution
      Incidents.add_action(incident.id, action["id"], "auto-executed")
    end)
  end

  defp check_for_escalations do
    # Find incidents that need escalation (unacknowledged for > 5 minutes)
    active = Incidents.list_by_status("new")

    Enum.each(active, fn incident ->
      age_seconds = DateTime.diff(DateTime.utc_now(), incident.created_at, :second)

      if incident.severity == "high" and age_seconds > 300 do
        Logger.warning("Escalating incident #{incident.id}: unacknowledged for #{age_seconds}s")
        # TODO: Send escalation notification
      end
    end)

    # Update active incidents gauge during escalation check
    update_active_incidents_gauge()
  end

  defp update_active_incidents_gauge do
    count = Incidents.list_active() |> length()
    Metrics.set_active_incidents(count)
  rescue
    _ -> :ok
  end
end
