defmodule AveroCommand.Entities.PersonRegistry do
  @moduledoc """
  Registry for Person GenServers.
  Tracks active persons by {site, person_id}.
  """
  require Logger

  alias AveroCommand.Entities.Person

  @registry AveroCommand.EntityRegistry
  @supervisor AveroCommand.PersonSupervisor

  @doc """
  Get or create a Person GenServer for the given site and person_id.

  Uses DynamicSupervisor.start_child atomically to avoid TOCTOU race conditions.
  """
  def get_or_create(site, person_id) do
    case start_person(site, person_id) do
      {:ok, pid} -> {:ok, pid}
      {:error, {:already_started, pid}} -> {:ok, pid}
      {:error, reason} -> {:error, reason}
    end
  end

  @doc """
  Get an existing Person GenServer, returns nil if not found.
  """
  def get(site, person_id) do
    key = {site, person_id}

    case Registry.lookup(@registry, {:person, key}) do
      [{pid, _}] when is_pid(pid) -> pid
      _ -> nil
    end
  end

  @doc """
  List all active persons with their state.
  """
  def list_all do
    Registry.select(@registry, [{{:"$1", :"$2", :"$3"}, [], [{{:"$1", :"$2", :"$3"}}]}])
    |> Enum.filter(fn {{type, _key}, _pid, _} -> type == :person end)
    |> Enum.map(fn {{:person, {site, person_id}}, pid, _} ->
      state = Person.get_state(pid)
      %{site: site, person_id: person_id, pid: inspect(pid), state: state}
    end)
  rescue
    _ -> []
  end

  defp start_person(site, person_id) do
    key = {site, person_id}

    child_spec = %{
      id: {:person, key},
      start: {Person, :start_link, [{site, person_id}]},
      restart: :temporary
    }

    case DynamicSupervisor.start_child(@supervisor, child_spec) do
      {:ok, pid} ->
        {:ok, pid}

      {:error, {:already_started, pid}} ->
        {:ok, pid}

      {:error, reason} ->
        Logger.warning("Failed to start Person #{site}:#{person_id}: #{inspect(reason)}")
        {:error, reason}
    end
  end
end
