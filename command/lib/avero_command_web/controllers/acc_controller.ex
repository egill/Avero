defmodule AveroCommandWeb.AccController do
  @moduledoc """
  API controller for simulating ACC (payment terminal) events.

  Used at Avero HQ where there are no physical payment terminals,
  allowing manual simulation of payment events for testing.
  """
  use AveroCommandWeb, :controller

  require Logger

  @doc """
  Simulate an ACC payment event by forwarding to the gateway.

  POST /api/acc/simulate
  Body: { "pos": "POS_1", "site": "avero" }
  """
  def simulate(conn, params) do
    pos = params["pos"] || "POS_1"
    site = params["site"] || "avero"

    Logger.info("ACC simulate: pos=#{pos} site=#{site}")

    conn
    |> put_cors_headers()
    |> forward_to_gateway(site, pos)
  end

  def options(conn, _params) do
    conn
    |> put_cors_headers()
    |> put_resp_header("access-control-allow-methods", "POST, OPTIONS")
    |> put_resp_header("access-control-allow-headers", "content-type")
    |> send_resp(204, "")
  end

  defp put_cors_headers(conn) do
    put_resp_header(conn, "access-control-allow-origin", "*")
  end

  defp forward_to_gateway(conn, site, pos) do
    case AveroCommand.Sites.gateway_url(site, "/acc/simulate?pos=#{pos}") do
      nil ->
        Logger.warning("ACC simulate: no gateway URL for site #{site}")

        conn
        |> put_status(400)
        |> json(%{ok: false, pos: pos, site: site, error: "Unknown site: #{site}"})

      gateway_url ->
        send_gateway_request(conn, gateway_url, site, pos)
    end
  end

  defp send_gateway_request(conn, gateway_url, site, pos) do
    :inets.start()
    :ssl.start()

    request = {String.to_charlist(gateway_url), [], ~c"application/json", ~c""}
    options = [{:timeout, 5000}]

    case :httpc.request(:post, request, options, []) do
      {:ok, {{_, status, _}, _, body}} when status in [200, 201] ->
        Logger.info("ACC simulate sent to gateway: status=#{status} body=#{inspect(body)}")
        json(conn, %{ok: true, pos: pos, site: site, message: "ACC event sent to gateway"})

      {:ok, {{_, status, _}, _, body}} ->
        Logger.warning("ACC simulate failed: status=#{status} body=#{inspect(body)}")

        conn
        |> put_status(502)
        |> json(%{ok: false, pos: pos, site: site, error: "Gateway returned status #{status}"})

      {:error, reason} ->
        Logger.warning("ACC simulate error: #{inspect(reason)}")

        conn
        |> put_status(502)
        |> json(%{ok: false, pos: pos, site: site, error: "Failed to contact gateway: #{inspect(reason)}"})
    end
  end
end
