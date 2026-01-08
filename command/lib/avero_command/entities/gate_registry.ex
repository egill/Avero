defmodule AveroCommand.Entities.GateRegistry do
  @moduledoc """
  Registry for Gate GenServers.
  Tracks active gates by {site, gate_id}.
  """
  require Logger

  alias AveroCommand.Entities.Gate

  @registry AveroCommand.EntityRegistry
  @supervisor AveroCommand.GateSupervisor

  @doc """
  Get or create a Gate GenServer for the given site and gate_id.

  Uses DynamicSupervisor.start_child atomically to avoid TOCTOU race conditions.
  """
  def get_or_create(site, gate_id) do
    case start_gate(site, gate_id) do
      {:ok, pid} -> {:ok, pid}
      {:error, {:already_started, pid}} -> {:ok, pid}
      {:error, reason} -> {:error, reason}
    end
  end

  @doc """
  Get an existing Gate GenServer, returns nil if not found.
  """
  def get(site, gate_id) do
    key = {site, gate_id}

    case Registry.lookup(@registry, {:gate, key}) do
      [{pid, _}] when is_pid(pid) -> pid
      _ -> nil
    end
  end

  @doc """
  List all active gates with their state.
  """
  def list_all do
    Registry.select(@registry, [{{:"$1", :"$2", :"$3"}, [], [{{:"$1", :"$2", :"$3"}}]}])
    |> Enum.filter(fn {{type, _key}, _pid, _} -> type == :gate end)
    |> Enum.map(fn {{:gate, {site, gate_id}}, pid, _} ->
      state = Gate.get_state(pid)
      %{site: site, gate_id: gate_id, pid: inspect(pid), state: state}
    end)
  rescue
    _ -> []
  end

  defp start_gate(site, gate_id) do
    key = {site, gate_id}

    child_spec = %{
      id: {:gate, key},
      start: {Gate, :start_link, [{site, gate_id}]},
      restart: :temporary
    }

    case DynamicSupervisor.start_child(@supervisor, child_spec) do
      {:ok, pid} ->
        {:ok, pid}

      {:error, {:already_started, pid}} ->
        {:ok, pid}

      {:error, reason} ->
        Logger.warning("Failed to start Gate #{site}:#{gate_id}: #{inspect(reason)}")
        {:error, reason}
    end
  end
end
