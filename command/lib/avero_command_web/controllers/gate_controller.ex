defmodule AveroCommandWeb.GateController do
  use AveroCommandWeb, :controller

  require Logger

  @gateway_ips %{
    "netto" => "100.80.187.3",
    "avero" => "100.65.110.63",
    "grandi" => "100.80.187.4"
  }

  def open(conn, params) do
    site = params["site"] || "netto"
    gateway_ip = @gateway_ips[site]

    if gateway_ip do
      url = ~c"http://#{gateway_ip}:9090/gate/open"
      :inets.start()

      case :httpc.request(:post, {url, [], ~c"application/json", ~c""}, [{:timeout, 5000}], []) do
        {:ok, {{_, 200, _}, _, body}} ->
          Logger.info("Gate opened for #{site}: #{inspect(body)}")

          conn
          |> put_resp_header("access-control-allow-origin", "*")
          |> json(%{ok: true, site: site})

        {:ok, {{_, status, _}, _, body}} ->
          Logger.warning("Gate open failed for #{site}: status=#{status} body=#{inspect(body)}")

          conn
          |> put_resp_header("access-control-allow-origin", "*")
          |> put_status(502)
          |> json(%{ok: false, error: "gateway_error", status: status})

        {:error, reason} ->
          Logger.warning("Gate open failed for #{site}: #{inspect(reason)}")

          conn
          |> put_resp_header("access-control-allow-origin", "*")
          |> put_status(502)
          |> json(%{ok: false, error: "connection_failed", reason: inspect(reason)})
      end
    else
      conn
      |> put_resp_header("access-control-allow-origin", "*")
      |> put_status(400)
      |> json(%{ok: false, error: "unknown_site", site: site})
    end
  end

  def options(conn, _params) do
    conn
    |> put_resp_header("access-control-allow-origin", "*")
    |> put_resp_header("access-control-allow-methods", "POST, OPTIONS")
    |> put_resp_header("access-control-allow-headers", "content-type")
    |> send_resp(204, "")
  end
end
