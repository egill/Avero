defmodule AveroCommandWeb.AccController do
  @moduledoc """
  API controller for simulating ACC (payment terminal) events.

  Used at Avero HQ where there are no physical payment terminals,
  allowing manual simulation of payment events for testing.
  """
  use AveroCommandWeb, :controller

  require Logger

  alias AveroCommand.Entities.PersonRegistry

  @doc """
  Simulate an ACC payment event.

  POST /api/acc/simulate
  Body: { "receipt_id": "123", "pos": "POS_1", "site": "avero" }

  Returns:
  - matched: true/false - whether a person was found in the zone
  - person_id: the matched person's track ID (if matched)
  - message: human-readable result
  """
  def simulate(conn, params) do
    receipt_id = params["receipt_id"] || generate_receipt_id()
    pos = params["pos"] || "POS_1"
    site = params["site"] || "avero"

    Logger.info("ACC simulate: receipt_id=#{receipt_id} pos=#{pos} site=#{site}")

    # Find active persons in the requested POS zone
    case find_person_in_zone(site, pos) do
      {:ok, person} ->
        # Broadcast ACC matched event
        broadcast_acc_event(site, receipt_id, pos, person)

        Logger.info("ACC matched: person_id=#{person.person_id} pos=#{pos}")

        conn
        |> put_resp_header("access-control-allow-origin", "*")
        |> json(%{
          matched: true,
          person_id: person.person_id,
          receipt_id: receipt_id,
          pos: pos,
          message: "Payment matched to person #{person.person_id}"
        })

      :not_found ->
        # Broadcast ACC unmatched event
        broadcast_acc_unmatched(site, receipt_id, pos)

        Logger.info("ACC unmatched: no person in #{pos}")

        conn
        |> put_resp_header("access-control-allow-origin", "*")
        |> json(%{
          matched: false,
          receipt_id: receipt_id,
          pos: pos,
          message: "No person found in #{pos} - use barcode to authorize"
        })
    end
  end

  def options(conn, _params) do
    conn
    |> put_resp_header("access-control-allow-origin", "*")
    |> put_resp_header("access-control-allow-methods", "POST, OPTIONS")
    |> put_resp_header("access-control-allow-headers", "content-type")
    |> send_resp(204, "")
  end

  # Find a person currently in the given POS zone
  defp find_person_in_zone(site, pos) do
    persons = PersonRegistry.list_all()

    # Find person in the specified site and zone
    match =
      persons
      |> Enum.filter(fn p ->
        p.site == site and
          p.state != nil and
          p.state[:current_zone] == pos
      end)
      |> Enum.max_by(fn p -> p.state[:started_at] end, fn -> nil end)

    if match do
      {:ok, %{person_id: match.person_id, state: match.state}}
    else
      :not_found
    end
  end

  # Broadcast ACC matched event via PubSub
  defp broadcast_acc_event(site, receipt_id, pos, person) do
    now = DateTime.utc_now()
    ts = DateTime.to_unix(now, :millisecond)

    acc_event = %{
      type: "matched",
      ts: ts,
      ip: "simulated",
      pos: pos,
      tid: person.person_id,
      dwell_ms: nil,
      gate_zone: nil,
      gate_entry_ts: nil,
      delta_ms: nil,
      gate_cmd_at: nil,
      debug_active: nil,
      debug_pending: nil,
      site: site,
      time: now,
      receipt_id: receipt_id,
      simulated: true
    }

    Phoenix.PubSub.broadcast(AveroCommand.PubSub, "acc_events", {:acc_event, acc_event})

    # Also broadcast payment event for dashboard
    Phoenix.PubSub.broadcast(
      AveroCommand.PubSub,
      "gateway:events",
      {:zone_event, %{zone_id: pos, event_type: :payment}}
    )
  end

  # Broadcast ACC unmatched event via PubSub
  defp broadcast_acc_unmatched(site, receipt_id, pos) do
    now = DateTime.utc_now()
    ts = DateTime.to_unix(now, :millisecond)

    acc_event = %{
      type: "unmatched",
      ts: ts,
      ip: "simulated",
      pos: pos,
      tid: nil,
      dwell_ms: nil,
      gate_zone: nil,
      gate_entry_ts: nil,
      delta_ms: nil,
      gate_cmd_at: nil,
      debug_active: nil,
      debug_pending: nil,
      site: site,
      time: now,
      receipt_id: receipt_id,
      simulated: true
    }

    Phoenix.PubSub.broadcast(AveroCommand.PubSub, "acc_events", {:acc_event, acc_event})
  end

  defp generate_receipt_id do
    :crypto.strong_rand_bytes(4) |> Base.encode16(case: :lower)
  end
end
