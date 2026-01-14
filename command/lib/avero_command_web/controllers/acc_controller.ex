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
    pos = params["pos"] || "POS_1"
    site = params["site"] || "avero"

    Logger.info("ACC simulate: pos=#{pos} site=#{site}")

    # Call the gateway's /acc/simulate endpoint
    gateway_url = AveroCommand.Sites.gateway_url(site, "/acc/simulate?pos=#{pos}")

    if gateway_url do
      :inets.start()
      :ssl.start()

      case :httpc.request(:post, {String.to_charlist(gateway_url), [], ~c"application/json", ~c""}, [{:timeout, 5000}], []) do
        {:ok, {{_, status, _}, _, body}} when status in [200, 201] ->
          Logger.info("ACC simulate sent to gateway: status=#{status} body=#{inspect(body)}")

          conn
          |> put_resp_header("access-control-allow-origin", "*")
          |> json(%{
            ok: true,
            pos: pos,
            site: site,
            message: "ACC event sent to gateway"
          })

        {:ok, {{_, status, _}, _, body}} ->
          Logger.warning("ACC simulate failed: status=#{status} body=#{inspect(body)}")

          conn
          |> put_resp_header("access-control-allow-origin", "*")
          |> put_status(502)
          |> json(%{
            ok: false,
            pos: pos,
            site: site,
            error: "Gateway returned status #{status}"
          })

        {:error, reason} ->
          Logger.warning("ACC simulate error: #{inspect(reason)}")

          conn
          |> put_resp_header("access-control-allow-origin", "*")
          |> put_status(502)
          |> json(%{
            ok: false,
            pos: pos,
            site: site,
            error: "Failed to contact gateway: #{inspect(reason)}"
          })
      end
    else
      Logger.warning("ACC simulate: no gateway URL for site #{site}")

      conn
      |> put_resp_header("access-control-allow-origin", "*")
      |> put_status(400)
      |> json(%{
        ok: false,
        pos: pos,
        site: site,
        error: "Unknown site: #{site}"
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
