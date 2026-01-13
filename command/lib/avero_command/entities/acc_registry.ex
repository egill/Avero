defmodule AveroCommand.Entities.AccRegistry do
  @moduledoc """
  Registry for ACC (payment terminal) GenServers.
  Tracks active ACC entities by {site, pos_zone}.
  """
  require Logger

  alias AveroCommand.Entities.Acc

  @registry AveroCommand.EntityRegistry
  @supervisor AveroCommand.AccSupervisor

  @doc """
  Get or create an ACC GenServer for the given site and POS zone.

  Uses DynamicSupervisor.start_child atomically to avoid TOCTOU race conditions.
  """
  def get_or_create(site, pos_zone) do
    case start_acc(site, pos_zone) do
      {:ok, pid} -> {:ok, pid}
      {:error, {:already_started, pid}} -> {:ok, pid}
      {:error, reason} -> {:error, reason}
    end
  end

  @doc """
  Get an existing ACC GenServer, returns nil if not found.
  """
  def get(site, pos_zone) do
    key = {site, pos_zone}

    case Registry.lookup(@registry, {:acc, key}) do
      [{pid, _}] when is_pid(pid) -> pid
      _ -> nil
    end
  end

  @doc """
  List all active ACC entities with their state.
  """
  def list_all do
    Registry.select(@registry, [{{:"$1", :"$2", :"$3"}, [], [{{:"$1", :"$2", :"$3"}}]}])
    |> Enum.filter(fn {{type, _key}, _pid, _} -> type == :acc end)
    |> Enum.map(fn {{:acc, {site, pos_zone}}, pid, _} ->
      state = Acc.get_state(pid)
      %{site: site, pos_zone: pos_zone, pid: inspect(pid), state: state}
    end)
  rescue
    _ -> []
  end

  defp start_acc(site, pos_zone) do
    key = {site, pos_zone}

    child_spec = %{
      id: {:acc, key},
      start: {Acc, :start_link, [{site, pos_zone}]},
      restart: :temporary
    }

    case DynamicSupervisor.start_child(@supervisor, child_spec) do
      {:ok, pid} ->
        {:ok, pid}

      {:error, {:already_started, pid}} ->
        {:ok, pid}

      {:error, reason} ->
        Logger.warning("Failed to start ACC #{site}:#{pos_zone}: #{inspect(reason)}")
        {:error, reason}
    end
  end
end
