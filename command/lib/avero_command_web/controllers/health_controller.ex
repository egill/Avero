defmodule AveroCommandWeb.HealthController do
  use AveroCommandWeb, :controller

  def index(conn, _params) do
    # Check database connection
    db_status = check_database()

    # Check MQTT connection
    mqtt_status = check_mqtt()

    status = if db_status == :ok and mqtt_status == :ok, do: :ok, else: :error
    http_status = if status == :ok, do: 200, else: 503

    conn
    |> put_status(http_status)
    |> json(%{
      status: status,
      database: db_status,
      mqtt: mqtt_status,
      timestamp: DateTime.utc_now() |> DateTime.to_iso8601()
    })
  end

  defp check_database do
    case Ecto.Adapters.SQL.query(AveroCommand.Repo, "SELECT 1", []) do
      {:ok, _} -> :ok
      {:error, _} -> :error
    end
  rescue
    _ -> :error
  end

  defp check_mqtt do
    case GenServer.whereis(AveroCommand.MQTT.Client) do
      nil -> :error
      pid when is_pid(pid) -> :ok
    end
  end
end
