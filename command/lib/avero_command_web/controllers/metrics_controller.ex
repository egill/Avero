defmodule AveroCommandWeb.MetricsController do
  @moduledoc """
  Prometheus metrics endpoint for scraping.

  Exposes all registered Prometheus metrics in text format.
  """
  use AveroCommandWeb, :controller

  def index(conn, _params) do
    metrics = Prometheus.Format.Text.format()

    conn
    |> put_resp_content_type("text/plain")
    |> send_resp(200, metrics)
  end
end
