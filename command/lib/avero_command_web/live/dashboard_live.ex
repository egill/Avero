defmodule AveroCommandWeb.DashboardLive do
  @moduledoc """
  Real-time dashboard showing gate status, POS zones, journeys, and Grafana metrics.
  """
  use AveroCommandWeb, :live_view

  alias AveroCommand.Entities.GateRegistry
  alias AveroCommand.Journeys

  @refresh_interval 1000
  @pos_zone_ids ["POS_1", "POS_2", "POS_3", "POS_4", "POS_5"]

  @impl true
  def mount(_params, _session, socket) do
    if connected?(socket) do
      Phoenix.PubSub.subscribe(AveroCommand.PubSub, "gates")
      Phoenix.PubSub.subscribe(AveroCommand.PubSub, "gateway:events")
      Phoenix.PubSub.subscribe(AveroCommand.PubSub, "gateway:journeys")
      Process.send_after(self(), :refresh, @refresh_interval)
    end

    gates = load_gates()
    journeys = load_recent_journeys(socket.assigns[:selected_sites] || [])
    pos_zones = init_pos_zones()

    {:ok, assign(socket,
      page_title: "Dashboard",
      gates: gates,
      journeys: journeys,
      pos_zones: pos_zones,
      last_updated: DateTime.utc_now()
    )}
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

  defp init_pos_zones do
    Enum.map(@pos_zone_ids, fn id ->
      %{id: id, occupied: false, count: 0, paid: false}
    end)
  end

  defp update_pos_zone(pos_zones, zone_id, type) do
    Enum.map(pos_zones, fn zone ->
      if zone.id == zone_id do
        case type do
          :zone_entry -> %{zone | occupied: true, count: zone.count + 1}
          :zone_exit -> %{zone | occupied: zone.count > 1, count: max(0, zone.count - 1)}
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
    <div class="space-y-4">
      <!-- Header Row -->
      <div class="flex items-center justify-between">
        <div class="flex items-center gap-3">
          <h1 class="text-2xl font-bold text-gray-900 dark:text-white">Dashboard</h1>
          <div class="flex items-center gap-1.5">
            <span class="relative flex h-2.5 w-2.5">
              <span class="animate-ping absolute inline-flex h-full w-full rounded-full bg-green-400 opacity-75"></span>
              <span class="relative inline-flex rounded-full h-2.5 w-2.5 bg-green-500"></span>
            </span>
            <span class="text-xs text-gray-500 dark:text-gray-400">Live</span>
          </div>
        </div>
        <span class="text-xs text-gray-400"><%= format_time(@last_updated) %></span>
      </div>

      <!-- Grafana Stats Row -->
      <div class="grid grid-cols-2 md:grid-cols-3 lg:grid-cols-6 gap-2">
        <div class="bg-white dark:bg-gray-800 rounded border border-gray-200 dark:border-gray-700 overflow-hidden">
          <iframe
            src="https://grafana.e18n.net/d-solo/NETTO-GRANDI-gateway/avero-hq-live?orgId=1&panelId=2&theme=light&refresh=5s"
            class="w-full h-20 border-0"
            loading="lazy"
            title="Active Tracks"
          ></iframe>
        </div>
        <div class="bg-white dark:bg-gray-800 rounded border border-gray-200 dark:border-gray-700 overflow-hidden">
          <iframe
            src="https://grafana.e18n.net/d-solo/NETTO-GRANDI-gateway/avero-hq-live?orgId=1&panelId=4&theme=light&refresh=5s"
            class="w-full h-20 border-0"
            loading="lazy"
            title="Exits"
          ></iframe>
        </div>
        <div class="bg-white dark:bg-gray-800 rounded border border-gray-200 dark:border-gray-700 overflow-hidden">
          <iframe
            src="https://grafana.e18n.net/d-solo/NETTO-GRANDI-gateway/avero-hq-live?orgId=1&panelId=5&theme=light&refresh=5s"
            class="w-full h-20 border-0"
            loading="lazy"
            title="Gate Opens"
          ></iframe>
        </div>
        <div class="bg-white dark:bg-gray-800 rounded border border-gray-200 dark:border-gray-700 overflow-hidden">
          <iframe
            src="https://grafana.e18n.net/d-solo/NETTO-GRANDI-gateway/avero-hq-live?orgId=1&panelId=6&theme=light&refresh=5s"
            class="w-full h-20 border-0"
            loading="lazy"
            title="Payments"
          ></iframe>
        </div>
        <div class="bg-white dark:bg-gray-800 rounded border border-gray-200 dark:border-gray-700 overflow-hidden">
          <iframe
            src="https://grafana.e18n.net/d-solo/NETTO-GRANDI-gateway/avero-hq-live?orgId=1&panelId=25&theme=light&refresh=5s"
            class="w-full h-20 border-0"
            loading="lazy"
            title="POS Unpaid"
          ></iframe>
        </div>
        <div class="bg-white dark:bg-gray-800 rounded border border-gray-200 dark:border-gray-700 overflow-hidden">
          <iframe
            src="https://grafana.e18n.net/d-solo/NETTO-GRANDI-gateway/avero-hq-live?orgId=1&panelId=26&theme=light&refresh=5s"
            class="w-full h-20 border-0"
            loading="lazy"
            title="Exits Lost"
          ></iframe>
        </div>
      </div>

      <!-- Main Content: Gate + POS Zones -->
      <div class="grid grid-cols-1 lg:grid-cols-3 gap-4">
        <div class="lg:col-span-2">
          <%= if Enum.empty?(@gates) do %>
            <.dash_card title="Gate Status">
              <div class="p-8 text-center text-gray-500 dark:text-gray-400">
                <p>No active gates</p>
              </div>
            </.dash_card>
          <% else %>
            <%= for gate <- @gates do %>
              <.gate_card gate={gate} />
            <% end %>
          <% end %>
        </div>

        <div>
          <.dash_card title="POS Zones">
            <div class="p-4 grid grid-cols-5 gap-2">
              <%= for zone <- @pos_zones do %>
                <.pos_zone zone={zone} />
              <% end %>
            </div>
          </.dash_card>

          <div class="mt-4">
            <.dash_card title="Gate Info">
              <div class="p-3 space-y-2 text-sm">
                <div class="flex justify-between">
                  <span class="text-gray-500">Active Gates</span>
                  <span class="font-medium"><%= length(@gates) %></span>
                </div>
                <div class="flex justify-between">
                  <span class="text-gray-500">Open</span>
                  <span class="font-medium text-green-600"><%= Enum.count(@gates, fn g -> g.state && g.state.state == :open end) %></span>
                </div>
                <div class="flex justify-between">
                  <span class="text-gray-500">Faults</span>
                  <span class="font-medium text-red-600"><%= Enum.count(@gates, fn g -> g.state && g.state.fault end) %></span>
                </div>
              </div>
            </.dash_card>
          </div>
        </div>
      </div>

      <!-- Charts Row -->
      <div class="grid grid-cols-1 lg:grid-cols-2 gap-4">
        <.dash_card title="Exits by Type (1h)">
          <iframe
            src="https://grafana.e18n.net/d-solo/NETTO-GRANDI-gateway/avero-hq-live?orgId=1&panelId=11&theme=light&from=now-1h&to=now&refresh=5s"
            class="w-full h-40 border-0"
            loading="lazy"
          ></iframe>
        </.dash_card>

        <.dash_card title="People Tracking (30m)">
          <iframe
            src="https://grafana.e18n.net/d-solo/NETTO-GRANDI-gateway/avero-hq-live?orgId=1&panelId=8&theme=light&from=now-30m&to=now&refresh=5s"
            class="w-full h-40 border-0"
            loading="lazy"
          ></iframe>
        </.dash_card>
      </div>

      <!-- Journeys + Gate State -->
      <div class="grid grid-cols-1 lg:grid-cols-2 gap-4">
        <.dash_card title="Recent Journeys">
          <div class="divide-y divide-gray-100 dark:divide-gray-800 max-h-64 overflow-y-auto">
            <%= if Enum.empty?(@journeys) do %>
              <div class="p-4 text-center text-sm text-gray-500 dark:text-gray-400">
                No recent journeys
              </div>
            <% else %>
              <%= for journey <- @journeys do %>
                <div class="px-4 py-2 flex items-center justify-between">
                  <div class="flex items-center gap-3">
                    <div class={[
                      "w-2 h-2 rounded-full",
                      journey.outcome == "paid_exit" && "bg-green-500",
                      journey.outcome == "unpaid_exit" && "bg-red-500",
                      journey.outcome not in ["paid_exit", "unpaid_exit"] && "bg-gray-400"
                    ]}></div>
                    <div>
                      <span class="text-sm font-medium text-gray-900 dark:text-white">
                        <%= journey.outcome || "unknown" %>
                      </span>
                      <span class="text-xs text-gray-500 dark:text-gray-400 ml-2">
                        <%= if journey.total_pos_dwell_ms, do: "#{div(journey.total_pos_dwell_ms, 1000)}s", else: "" %>
                      </span>
                    </div>
                  </div>
                  <span class="text-xs text-gray-400">
                    <%= format_journey_time(journey.ended_at || journey.time) %>
                  </span>
                </div>
              <% end %>
            <% end %>
          </div>
        </.dash_card>

        <.dash_card title="Gate State (5m)">
          <iframe
            src="https://grafana.e18n.net/d-solo/NETTO-GRANDI-gateway/avero-hq-live?orgId=1&panelId=15&theme=light&from=now-5m&to=now&refresh=5s"
            class="w-full h-40 border-0"
            loading="lazy"
          ></iframe>
        </.dash_card>
      </div>

      <!-- Additional Metrics -->
      <div class="grid grid-cols-1 lg:grid-cols-3 gap-4">
        <.dash_card title="Cumulative Exits (24h)">
          <iframe
            src="https://grafana.e18n.net/d-solo/NETTO-GRANDI-gateway/avero-hq-live?orgId=1&panelId=16&theme=light&from=now-24h&to=now&refresh=30s"
            class="w-full h-36 border-0"
            loading="lazy"
          ></iframe>
        </.dash_card>

        <.dash_card title="Gate Cycle Duration">
          <iframe
            src="https://grafana.e18n.net/d-solo/NETTO-GRANDI-gateway/avero-hq-live?orgId=1&panelId=19&theme=light&from=now-1h&to=now&refresh=5s"
            class="w-full h-36 border-0"
            loading="lazy"
          ></iframe>
        </.dash_card>

        <.dash_card title="Tailgating Detection">
          <iframe
            src="https://grafana.e18n.net/d-solo/NETTO-GRANDI-gateway/avero-hq-live?orgId=1&panelId=20&theme=light&from=now-1h&to=now&refresh=5s"
            class="w-full h-36 border-0"
            loading="lazy"
          ></iframe>
        </.dash_card>
      </div>
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
    zone_num = assigns.zone.id |> String.replace("POS_", "")
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

  # === Helpers ===

  defp format_time(datetime) do
    Calendar.strftime(datetime, "%H:%M:%S")
  end

  defp format_journey_time(nil), do: "-"
  defp format_journey_time(%DateTime{} = dt) do
    diff = DateTime.diff(DateTime.utc_now(), dt, :second)
    cond do
      diff < 60 -> "#{diff}s ago"
      diff < 3600 -> "#{div(diff, 60)}m ago"
      true -> Calendar.strftime(dt, "%H:%M")
    end
  end
  defp format_journey_time(%NaiveDateTime{} = ndt) do
    ndt |> DateTime.from_naive!("Etc/UTC") |> format_journey_time()
  end
  defp format_journey_time(_), do: "-"
end
