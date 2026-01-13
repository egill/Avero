defmodule AveroCommandWeb.DashboardLive do
  @moduledoc """
  Real-time dashboard showing gate status, POS zones, journeys, and Grafana metrics.
  """
  use AveroCommandWeb, :live_view

  alias AveroCommand.Entities.GateRegistry
  alias AveroCommand.Journeys

  @refresh_interval 1000

  @impl true
  def mount(_params, _session, socket) do
    # Redirect to Grafana dashboard
    {:ok, redirect(socket, external: "https://grafana.e18n.net/d/command-live/command-live?orgId=1&theme=dark&kiosk=tv&refresh=5s")}
  end

  @impl true
  def handle_info(:refresh, socket) do
    Process.send_after(self(), :refresh, @refresh_interval)
    gates = load_gates()
    {:noreply, assign(socket, gates: gates, last_updated: DateTime.utc_now())}
  end

  def handle_info({:gate_event, _event}, socket) do
    gates = load_gates()
    {:noreply, assign(socket, gates: gates, last_updated: DateTime.utc_now())}
  end

  def handle_info({:journey_completed, _journey}, socket) do
    journeys = load_recent_journeys(socket.assigns[:selected_sites] || [])
    {:noreply, assign(socket, journeys: journeys)}
  end

  def handle_info({:zone_event, %{zone_id: zone_id, event_type: type}}, socket) do
    pos_zones = update_pos_zone(socket.assigns.pos_zones, zone_id, type)
    {:noreply, assign(socket, pos_zones: pos_zones)}
  end

  def handle_info(_msg, socket), do: {:noreply, socket}

  @impl true
  def handle_event("open_gate", %{"site" => site}, socket) do
    require Logger

    # Map site to gateway IP (via Tailscale)
    gateway_ip = case site do
      "netto" -> "100.80.187.3"
      "grandi" -> "100.80.187.4"
      _ -> nil
    end

    if gateway_ip do
      # Call the gateway's HTTP endpoint to open the gate
      Task.start(fn ->
        url = ~c"http://#{gateway_ip}:9090/gate/open"
        # Ensure httpc is available (should be started at app boot but be safe)
        :inets.start()
        :ssl.start()

        case :httpc.request(:post, {url, [], ~c"application/json", ~c""}, [{:timeout, 5000}], []) do
          {:ok, {{_, status, _}, _, body}} ->
            Logger.info("Gate open response for #{site}: status=#{status} body=#{inspect(body)}")
            if status == 200 do
              Phoenix.PubSub.broadcast(AveroCommand.PubSub, "gates", {:gate_opened, site})
            end
          {:error, reason} ->
            Logger.warning("Gate open failed for #{site}: #{inspect(reason)}")
        end
      end)
    end

    {:noreply, socket}
  end

  defp update_pos_zone(pos_zones, zone_id, type) do
    Enum.map(pos_zones, fn zone ->
      if zone.id == zone_id do
        case type do
          :zone_entry -> %{zone | occupied: true, count: zone.count + 1}
          :zone_exit ->
            new_count = max(0, zone.count - 1)
            %{zone | occupied: new_count > 0, count: new_count, paid: if(new_count == 0, do: false, else: zone.paid)}
          :payment -> %{zone | paid: true}
          _ -> zone
        end
      else
        zone
      end
    end)
  end

  defp load_gates do
    GateRegistry.list_all()
    |> Enum.sort_by(fn g -> {g.site, g.gate_id} end)
  end

  defp load_recent_journeys(sites) do
    try do
      Journeys.list_filtered(sites: sites, exit_type: :exits, limit: 5)
    rescue
      _ -> []
    end
  end

  @impl true
  def render(assigns) do
    ~H"""
    <div id="grafana-fullscreen" phx-update="ignore" class="fixed inset-0 -m-4 lg:-m-8">
      <iframe
        src="https://grafana.e18n.net/d/command-live/command-live?orgId=1&theme=dark&kiosk=tv&refresh=5s"
        class="w-full h-full border-0"
        title="Command Live Dashboard"
      ></iframe>
    </div>
    """
  end

  # === Components ===

  attr :title, :string, default: nil
  slot :inner_block, required: true

  defp dash_card(assigns) do
    ~H"""
    <div class="bg-white dark:bg-gray-800 rounded-lg border border-gray-200 dark:border-gray-700 overflow-hidden">
      <div :if={@title} class="px-4 py-3 border-b border-gray-100 dark:border-gray-700">
        <h3 class="text-sm font-semibold text-gray-700 dark:text-gray-300"><%= @title %></h3>
      </div>
      <%= render_slot(@inner_block) %>
    </div>
    """
  end

  attr :gate, :map, required: true

  defp gate_card(assigns) do
    state = assigns.gate.state || %{state: :unknown, persons_in_zone: 0, fault: false}
    gate_state = state[:state] || :unknown
    is_open = gate_state == :open
    has_fault = state[:fault] || false
    persons = state[:persons_in_zone] || 0

    assigns = assign(assigns,
      gate_state: gate_state,
      is_open: is_open,
      has_fault: has_fault,
      persons: persons,
      state: state
    )

    ~H"""
    <div class={[
      "rounded-lg border p-6 transition-all duration-300",
      @has_fault && "border-red-500 bg-red-50 dark:bg-red-900/20",
      !@has_fault && @is_open && "border-green-500 bg-green-50 dark:bg-green-900/20",
      !@has_fault && !@is_open && "border-gray-200 dark:border-gray-700 bg-white dark:bg-gray-800"
    ]}>
      <div class="flex items-center justify-between mb-4">
        <div>
          <h3 class="font-semibold text-gray-900 dark:text-white">Gate <%= @gate.gate_id %></h3>
          <p class="text-xs text-gray-500 dark:text-gray-400"><%= @gate.site %></p>
        </div>
        <div class={[
          "px-3 py-1 rounded-full text-xs font-medium",
          @has_fault && "bg-red-100 text-red-700 dark:bg-red-900/50 dark:text-red-300",
          !@has_fault && @is_open && "bg-green-100 text-green-700 dark:bg-green-900/50 dark:text-green-300",
          !@has_fault && !@is_open && "bg-gray-100 text-gray-700 dark:bg-gray-700 dark:text-gray-300"
        ]}>
          <%= if @has_fault, do: "FAULT", else: String.upcase(to_string(@gate_state)) %>
        </div>
      </div>

      <div class="flex justify-center my-6">
        <.gate_animation is_open={@is_open} has_fault={@has_fault} />
      </div>

      <div class="flex items-center justify-between text-sm">
        <span class="text-gray-500 dark:text-gray-400">Persons in zone</span>
        <span class={[
          "font-semibold",
          @persons > 0 && "text-brand-600 dark:text-brand-400",
          @persons == 0 && "text-gray-700 dark:text-gray-300"
        ]}>
          <%= @persons %>
        </span>
      </div>

      <div class="mt-4 pt-4 border-t border-gray-200 dark:border-gray-700">
        <button
          phx-click="open_gate"
          phx-value-site={@gate.site}
          class="w-full px-4 py-2 bg-green-600 hover:bg-green-700 text-white font-medium rounded-lg transition-colors duration-200 flex items-center justify-center gap-2"
        >
          <svg xmlns="http://www.w3.org/2000/svg" class="h-5 w-5" viewBox="0 0 20 20" fill="currentColor">
            <path fill-rule="evenodd" d="M10 18a8 8 0 100-16 8 8 0 000 16zM9.555 7.168A1 1 0 008 8v4a1 1 0 001.555.832l3-2a1 1 0 000-1.664l-3-2z" clip-rule="evenodd" />
          </svg>
          Open Gate
        </button>
      </div>
    </div>
    """
  end

  attr :is_open, :boolean, default: false
  attr :has_fault, :boolean, default: false

  defp gate_animation(assigns) do
    # Colors from original Go dashboard (dashboard.monitor.js)
    # Open: green rgba(34, 197, 94, 0.3) / rgba(34, 197, 94, 0.5)
    # Closed: indigo rgba(99, 102, 241, 0.2) / #6366f1
    # Fault: red
    {door_fill, door_stroke, left_transform, right_transform} = cond do
      assigns.has_fault ->
        {"rgba(239, 68, 68, 0.3)", "rgba(239, 68, 68, 0.6)", "translateX(0)", "translateX(0)"}
      assigns.is_open ->
        {"rgba(34, 197, 94, 0.3)", "rgba(34, 197, 94, 0.5)", "translateX(-105px)", "translateX(105px)"}
      true ->
        {"rgba(99, 102, 241, 0.2)", "#6366f1", "translateX(0)", "translateX(0)"}
    end

    assigns = assign(assigns,
      door_fill: door_fill,
      door_stroke: door_stroke,
      left_transform: left_transform,
      right_transform: right_transform
    )

    ~H"""
    <svg id="gate-svg" viewBox="0 0 500 160" class="w-full max-w-md h-auto">
      <!-- LEFT DOOR -->
      <rect
        id="gate-left-door"
        x="141" y="30" width="110" height="100"
        fill={@door_fill}
        stroke={@door_stroke}
        stroke-width="2"
        rx="4"
        style={"transform: #{@left_transform}; transition: transform 2.5s ease, fill 0.3s ease, stroke 0.3s ease;"}
      />
      <!-- RIGHT DOOR -->
      <rect
        id="gate-right-door"
        x="251" y="30" width="110" height="100"
        fill={@door_fill}
        stroke={@door_stroke}
        stroke-width="2"
        rx="4"
        style={"transform: #{@right_transform}; transition: transform 2.5s ease, fill 0.3s ease, stroke 0.3s ease;"}
      />
      <!-- LEFT PILLAR -->
      <rect x="30" y="15" width="110" height="130" rx="8" fill="#334155" stroke="#1e293b" stroke-width="0" />
      <!-- RIGHT PILLAR -->
      <rect x="360" y="15" width="110" height="130" rx="8" fill="#334155" stroke="#1e293b" stroke-width="0" />
    </svg>
    """
  end

  attr :zone, :map, required: true

  defp pos_zone(assigns) do
    # Extract zone number: "1001" -> "1", "1002" -> "2", etc.
    zone_num = case assigns.zone.id do
      "100" <> n -> n
      id -> id
    end
    assigns = assign(assigns, :zone_num, zone_num)

    ~H"""
    <div class={[
      "relative rounded-lg p-3 text-center transition-all",
      @zone.occupied && @zone.paid && "bg-green-100 dark:bg-green-900/30 border-2 border-green-400",
      @zone.occupied && !@zone.paid && "bg-amber-100 dark:bg-amber-900/30 border-2 border-amber-400",
      !@zone.occupied && "bg-gray-100 dark:bg-gray-700 border border-gray-200 dark:border-gray-600"
    ]}>
      <div class={[
        "text-lg font-bold",
        @zone.occupied && @zone.paid && "text-green-700 dark:text-green-400",
        @zone.occupied && !@zone.paid && "text-amber-700 dark:text-amber-400",
        !@zone.occupied && "text-gray-400 dark:text-gray-500"
      ]}>
        <%= @zone_num %>
      </div>
      <%= if @zone.count > 0 do %>
        <div class="text-xs text-gray-600 dark:text-gray-400"><%= @zone.count %></div>
      <% end %>
      <%= if @zone.occupied && @zone.paid do %>
        <div class="absolute -top-1 -right-1 w-4 h-4 bg-green-500 rounded-full flex items-center justify-center">
          <svg class="w-2.5 h-2.5 text-white" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="3">
            <path stroke-linecap="round" stroke-linejoin="round" d="M5 13l4 4L19 7" />
          </svg>
        </div>
      <% end %>
    </div>
    """
  end

end
