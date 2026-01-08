defmodule AveroCommand.MQTT.Client do
  @moduledoc """
  MQTT client for subscribing to gateway events.
  Uses Tortoise311 for MQTT connectivity with automatic reconnection.
  """
  use GenServer
  require Logger

  alias AveroCommand.MQTT.EventRouter

  @reconnect_interval 5_000  # 5 seconds between reconnection attempts
  @max_reconnect_attempts 10  # Max consecutive failures before longer backoff

  def start_link(opts \\ []) do
    GenServer.start_link(__MODULE__, opts, name: __MODULE__)
  end

  @doc """
  Check if the MQTT client is connected.
  """
  def connected? do
    GenServer.call(__MODULE__, :connected?)
  catch
    :exit, _ -> false
  end

  @impl true
  def init(_opts) do
    config = Application.get_env(:avero_command, :mqtt, [])
    host = Keyword.get(config, :host, "localhost")
    port = Keyword.get(config, :port, 1883)
    username = Keyword.get(config, :username)
    password = Keyword.get(config, :password)
    client_id = Keyword.get(config, :client_id, "avero_command_#{:rand.uniform(10000)}")
    topics = Keyword.get(config, :topics, ["avero/events/#"])

    state = %{
      client_id: client_id,
      host: host,
      port: port,
      username: username,
      password: password,
      topics: topics,
      connected: false,
      reconnect_attempts: 0
    }

    # Attempt initial connection
    send(self(), :connect)

    {:ok, state}
  end

  @impl true
  def handle_call(:connected?, _from, state) do
    {:reply, state.connected, state}
  end

  @impl true
  def handle_info(:connect, state) do
    case attempt_connection(state) do
      {:ok, new_state} ->
        {:noreply, new_state}

      {:error, reason, new_state} ->
        Logger.warning("MQTT connection failed: #{inspect(reason)}, will retry in #{reconnect_delay(new_state)}ms")
        schedule_reconnect(new_state)
        {:noreply, new_state}
    end
  end

  @impl true
  def handle_info(:reconnect, state) do
    send(self(), :connect)
    {:noreply, state}
  end

  @impl true
  def handle_info({:mqtt_connected}, state) do
    Logger.info("MQTT Client connected")
    {:noreply, %{state | connected: true, reconnect_attempts: 0}}
  end

  @impl true
  def handle_info({:mqtt_disconnected, reason}, state) do
    Logger.warning("MQTT Client disconnected: #{inspect(reason)}")
    new_state = %{state | connected: false}
    schedule_reconnect(new_state)
    {:noreply, new_state}
  end

  @impl true
  def handle_info(msg, state) do
    Logger.debug("MQTT Client received: #{inspect(msg)}")
    {:noreply, state}
  end

  # ============================================
  # Private Functions
  # ============================================

  defp attempt_connection(state) do
    Logger.info("MQTT Client connecting to #{state.host}:#{state.port} (attempt #{state.reconnect_attempts + 1})")

    conn_opts = build_connection_opts(state)

    case Tortoise311.Connection.start_link(conn_opts) do
      {:ok, _pid} ->
        Logger.info("MQTT Client connection initiated")
        {:ok, %{state | connected: true, reconnect_attempts: 0}}

      {:error, reason} ->
        new_state = %{state | connected: false, reconnect_attempts: state.reconnect_attempts + 1}
        {:error, reason, new_state}
    end
  rescue
    e ->
      new_state = %{state | connected: false, reconnect_attempts: state.reconnect_attempts + 1}
      {:error, e, new_state}
  end

  defp build_connection_opts(state) do
    conn_opts = [
      client_id: state.client_id,
      handler: {AveroCommand.MQTT.Handler, []},
      server: {Tortoise311.Transport.Tcp, host: String.to_charlist(state.host), port: state.port},
      subscriptions: Enum.map(state.topics, fn topic -> {topic, 0} end)
    ]

    if state.username && state.password do
      conn_opts
      |> Keyword.put(:user_name, state.username)
      |> Keyword.put(:password, state.password)
    else
      conn_opts
    end
  end

  defp schedule_reconnect(state) do
    delay = reconnect_delay(state)
    Process.send_after(self(), :reconnect, delay)
  end

  defp reconnect_delay(%{reconnect_attempts: attempts}) when attempts >= @max_reconnect_attempts do
    # After many failures, use exponential backoff up to 5 minutes
    min(300_000, @reconnect_interval * :math.pow(2, attempts - @max_reconnect_attempts)) |> trunc()
  end

  defp reconnect_delay(_state) do
    @reconnect_interval
  end
end

defmodule AveroCommand.MQTT.Handler do
  @moduledoc """
  Tortoise311 handler for processing incoming MQTT messages.
  """
  use Tortoise311.Handler
  require Logger

  alias AveroCommand.MQTT.EventRouter

  @impl true
  def init(_opts) do
    Logger.info("MQTT Handler initialized")
    {:ok, %{}}
  end

  @impl true
  def connection(:up, state) do
    Logger.info("MQTT connection up")
    {:ok, state}
  end

  @impl true
  def connection(:down, state) do
    Logger.warning("MQTT connection down")
    {:ok, state}
  end

  @impl true
  def connection(:terminating, state) do
    Logger.info("MQTT connection terminating")
    {:ok, state}
  end

  @impl true
  def subscription(:up, topic, state) do
    Logger.info("Subscribed to #{topic}")
    {:ok, state}
  end

  @impl true
  def subscription({:warn, _}, topic, state) do
    Logger.warning("Subscription warning for #{topic}")
    {:ok, state}
  end

  @impl true
  def subscription({:error, reason}, topic, state) do
    Logger.error("Subscription error for #{topic}: #{inspect(reason)}")
    {:ok, state}
  end

  @impl true
  def subscription(:down, topic, state) do
    Logger.info("Unsubscribed from #{topic}")
    {:ok, state}
  end

  @impl true
  def handle_message(topic, payload, state) do
    Logger.debug("Received message on #{Enum.join(topic, "/")}")

    case Jason.decode(payload) do
      {:ok, event_data} ->
        EventRouter.route_event(topic, event_data)

      {:error, reason} ->
        Logger.warning("Failed to parse MQTT message: #{inspect(reason)}")
    end

    {:ok, state}
  end

  @impl true
  def terminate(reason, _state) do
    Logger.info("MQTT Handler terminating: #{inspect(reason)}")
    :ok
  end
end
