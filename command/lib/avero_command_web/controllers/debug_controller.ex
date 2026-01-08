defmodule AveroCommandWeb.DebugController do
  use AveroCommandWeb, :controller

  alias AveroCommand.Entities.PersonRegistry
  alias AveroCommand.Entities.GateRegistry

  def persons(conn, _params) do
    persons = PersonRegistry.list_all()
    json(conn, %{count: length(persons), persons: persons})
  end

  def gates(conn, _params) do
    gates = GateRegistry.list_all()
    json(conn, %{count: length(gates), gates: gates})
  end

  def events(conn, params) do
    limit = Map.get(params, "limit", "100") |> String.to_integer()

    events =
      AveroCommand.Store.Event
      |> AveroCommand.Repo.all()
      |> Enum.take(limit)
      |> Enum.map(&Map.from_struct/1)

    json(conn, %{count: length(events), events: events})
  rescue
    _ -> json(conn, %{count: 0, events: [], error: "Failed to fetch events"})
  end
end
