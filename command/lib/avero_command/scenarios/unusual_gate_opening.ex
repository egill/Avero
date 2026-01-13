defmodule AveroCommand.Scenarios.UnusualGateOpening do
  @moduledoc """
  Scenario: Unusual Gate Opening

  Detects when a gate has been open for more than 2 minutes.
  Unlike GateStuck (60s, equipment focus), this scenario focuses on
  operational monitoring of extended gate openings.

  Detection: Timer fires 2 minutes after gate opens (via Gate GenServer)
  Resolution: Auto-resolve on gate.closed event

  When active, the incident detail view shows live statistics:
  - Total exits during the window
  - Paid vs unpaid exits
  - Backward crossings
  - List of journeys

  Severity: HIGH
  """
  require Logger
  import Ecto.Query

  alias AveroCommand.Repo
  alias AveroCommand.Incidents
  alias AveroCommand.Incidents.Incident
  alias AveroCommand.Incidents.Manager

  # 2 minutes
  @threshold_ms 120_000

  @doc """
  Returns the threshold in milliseconds.
  """
  def threshold_ms, do: @threshold_ms

  @doc """
  Create an unusual_gate_opening incident.
  Called by Gate GenServer timer after threshold is exceeded.
  """
  def create_incident(site, gate_id, opened_at) do
    started_at_ms =
      case opened_at do
        %DateTime{} -> DateTime.to_unix(opened_at, :millisecond)
        ms when is_integer(ms) -> ms
        _ -> DateTime.to_unix(DateTime.utc_now(), :millisecond) - @threshold_ms
      end

    incident_attrs = %{
      type: "unusual_gate_opening",
      severity: "high",
      category: "operational",
      site: site,
      gate_id: gate_id,
      context: %{
        gate_id: gate_id,
        started_at_ms: started_at_ms,
        is_live: true,
        message: "Gate has been open for over 2 minutes"
      },
      suggested_actions: [
        %{"id" => "investigate", "label" => "Investigate", "auto" => false},
        %{"id" => "dismiss", "label" => "Dismiss (Known maintenance)", "auto" => false}
      ]
    }

    case Manager.create_incident(incident_attrs) do
      {:ok, incident} when is_map(incident) ->
        Logger.info("UnusualGateOpening: Created incident for gate #{gate_id} at #{site}")
        {:ok, incident}

      {:ok, :duplicate} ->
        Logger.debug("UnusualGateOpening: Duplicate incident for gate #{gate_id}, skipping")
        {:ok, :duplicate}

      {:error, reason} = err ->
        Logger.warning("UnusualGateOpening: Failed to create incident: #{inspect(reason)}")
        err
    end
  end

  @doc """
  Auto-resolve any active unusual_gate_opening incident for this gate.
  Called when gate.closed event is received.
  """
  def maybe_resolve(site, gate_id) do
    case find_active_incident(site, gate_id) do
      nil ->
        :no_active_incident

      incident ->
        resolve_with_final_stats(incident)
    end
  end

  defp find_active_incident(site, gate_id) do
    from(i in Incident,
      where: i.type == "unusual_gate_opening",
      where: i.site == ^site,
      where: i.gate_id == ^gate_id,
      where: i.status in ["new", "acknowledged", "in_progress"],
      limit: 1
    )
    |> Repo.one()
  rescue
    e ->
      Logger.warning("UnusualGateOpening: find_active_incident failed: #{inspect(e)}")
      nil
  end

  defp resolve_with_final_stats(incident) do
    context = incident.context || %{}
    started_at_ms = context["started_at_ms"]
    closed_at_ms = DateTime.to_unix(DateTime.utc_now(), :millisecond)

    total_duration_ms =
      if started_at_ms do
        closed_at_ms - started_at_ms
      else
        0
      end

    # Update context with final stats
    updated_context =
      Map.merge(context, %{
        "is_live" => false,
        "closed_at_ms" => closed_at_ms,
        "total_duration_ms" => total_duration_ms
      })

    # Update the incident context first
    incident
    |> Incident.changeset(%{context: updated_context})
    |> Repo.update()

    # Then resolve it
    case Incidents.resolve(incident.id, "gate_closed", "system") do
      {:ok, resolved} ->
        Logger.info(
          "UnusualGateOpening: Auto-resolved incident #{incident.id} " <>
            "(duration: #{div(total_duration_ms, 1000)}s)"
        )

        {:ok, resolved}

      {:error, reason} = err ->
        Logger.warning("UnusualGateOpening: Failed to resolve incident: #{inspect(reason)}")
        err
    end
  end
end
