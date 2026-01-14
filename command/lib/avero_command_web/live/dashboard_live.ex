defmodule AveroCommandWeb.DashboardLive do
  @moduledoc """
  Real-time dashboard showing gate status, POS zones, journeys, and Grafana metrics.
  Site-aware: displays data for the currently selected site.
  """
  use AveroCommandWeb, :live_view

  require Logger

  alias AveroCommand.Entities.GateRegistry
  alias AveroCommand.Journeys
  alias AveroCommand.Sites

  @refresh_interval 1000
  @http_timeout 5000

  @impl true
  def mount(_params, _session, socket) do
    if connected?(socket) do
      # Subscribe to real-time updates
      Phoenix.PubSub.subscribe(AveroCommand.PubSub, "gates")
      Phoenix.PubSub.subscribe(AveroCommand.PubSub, "journeys")
      # Zone events are broadcast on "gateway:events" channel by EventRouter
      Phoenix.PubSub.subscribe(AveroCommand.PubSub, "gateway:events")
      Process.send_after(self(), :refresh, @refresh_interval)
    end

    # Get site info from the hook
    selected_site = socket.assigns[:selected_site] || "netto"
    site_config = socket.assigns[:site_config] || Sites.get(selected_site)
    selected_sites = socket.assigns[:selected_sites] || []

    gates = load_gates(selected_site)
    journeys = load_recent_journeys(selected_sites)
    pos_zones = build_pos_zones(site_config)

    {:ok,
     socket
     |> assign(:page_title, "Dashboard")
     |> assign(:gates, gates)
     |> assign(:journeys, journeys)
     |> assign(:pos_zones, pos_zones)
     |> assign(:authorized_at_gate, 0)
     |> assign(:last_updated, DateTime.utc_now())}
  end

  @impl true
  def handle_info(:refresh, socket) do
    Process.send_after(self(), :refresh, @refresh_interval)
    selected_site = socket.assigns[:selected_site]
    gates = load_gates(selected_site)
    {:noreply, assign(socket, gates: gates, last_updated: DateTime.utc_now())}
  end

  @impl true
  def handle_info({:gate_event, _event}, socket) do
    selected_site = socket.assigns[:selected_site]
    gates = load_gates(selected_site)
    {:noreply, assign(socket, gates: gates, last_updated: DateTime.utc_now())}
  end

  @impl true
  def handle_info({:journey_created, _journey}, socket) do
    journeys = load_recent_journeys(socket.assigns[:selected_sites] || [])
    {:noreply, assign(socket, journeys: journeys)}
  end

  @impl true
  def handle_info({:zone_event, %{site: event_site, zone_id: zone_id, event_type: type}}, socket) do
    if event_site != socket.assigns[:selected_site] do
      {:noreply, socket}
    else
      pos_zones = update_pos_zone(socket.assigns.pos_zones, zone_id, type)
      {:noreply, assign(socket, pos_zones: pos_zones)}
    end
  end

  @impl true
  def handle_info(_msg, socket), do: {:noreply, socket}

  @impl true
  def handle_event("open_gate", _params, socket) do
    selected_site = socket.assigns[:selected_site]

    case Sites.gateway_url(selected_site, "/gate/open") do
      nil ->
        {:noreply, socket}

      gateway_url ->
        Task.start(fn -> send_gate_open_request(gateway_url, selected_site) end)
        {:noreply, socket}
    end
  end

  def handle_event("simulate_acc", %{"pos" => pos}, socket) do
    selected_site = socket.assigns[:selected_site]

    Task.start(fn -> send_acc_simulate_request(selected_site, pos) end)

    {:noreply, put_flash(socket, :info, "ACC triggered for #{pos}")}
  end

  # Handle site switching - reload data for new site
  def handle_event("switch_site", %{"site" => site_key}, socket) do
    # The SiteFilterHook handles updating the assigns, but we need to reload data
    site_config = Sites.get(site_key)
    # Use site key ("netto") not site ID ("AP-NETTO-GR-01") - database uses key
    selected_sites = [site_key]

    gates = load_gates(site_key)
    journeys = load_recent_journeys(selected_sites)
    pos_zones = build_pos_zones(site_config)

    {:noreply,
     socket
     |> assign(:selected_site, site_key)
     |> assign(:site_config, site_config)
     |> assign(:selected_sites, selected_sites)
     |> assign(:gates, gates)
     |> assign(:journeys, journeys)
     |> assign(:pos_zones, pos_zones)
     |> put_flash(:info, "Switched to #{site_config.name}")}
  end

  defp update_pos_zone(pos_zones, zone_id, type) do
    Enum.map(pos_zones, fn zone ->
      if zone.id == zone_id do
        apply_zone_event(zone, type)
      else
        zone
      end
    end)
  end

  defp apply_zone_event(zone, :zone_entry) do
    occupied_since = zone.occupied_since || DateTime.utc_now()
    %{zone | occupied: true, count: zone.count + 1, occupied_since: occupied_since}
  end

  defp apply_zone_event(zone, :zone_exit) do
    new_count = max(0, zone.count - 1)
    now = DateTime.utc_now()

    {occupied_since, total_dwell_ms} =
      if new_count == 0 and zone.occupied_since do
        elapsed = DateTime.diff(now, zone.occupied_since, :millisecond)
        {nil, zone.total_dwell_ms + elapsed}
      else
        {zone.occupied_since, zone.total_dwell_ms}
      end

    %{
      zone
      | occupied: new_count > 0,
        count: new_count,
        paid: new_count > 0 and zone.paid,
        occupied_since: occupied_since,
        total_dwell_ms: total_dwell_ms
    }
  end

  defp apply_zone_event(zone, :payment), do: %{zone | paid: true}
  defp apply_zone_event(zone, _type), do: zone

  defp load_gates(nil), do: []

  defp load_gates(site_key) when is_binary(site_key) do
    # Filter by site key (e.g., "netto", "avero") which matches what the gateway sends
    GateRegistry.list_all()
    |> Enum.filter(fn g -> g.site == site_key end)
    |> Enum.sort_by(fn g -> {g.site, g.gate_id} end)
  end

  defp load_gates(_site_config) do
    # Fallback for old calls passing site_config struct
    []
  end

  defp load_recent_journeys(sites) do
    try do
      Journeys.list_filtered(sites: sites, exit_type: :exits, limit: 5)
    rescue
      _ -> []
    end
  end

  defp build_pos_zones(nil), do: []

  defp build_pos_zones(site_config) do
    site_config.pos_zones
    |> Enum.map(fn zone_id ->
      %{
        id: zone_id,
        occupied: false,
        count: 0,
        paid: false,
        # Track when current occupation started (for live timer)
        occupied_since: nil,
        # Total accumulated dwell time in ms
        total_dwell_ms: 0
      }
    end)
  end

  defp pos_zone_total_count(pos_zones) do
    Enum.reduce(pos_zones, 0, fn zone, acc -> acc + (zone.count || 0) end)
  end

  defp format_duration_ms(nil), do: "--:--"

  defp format_duration_ms(ms) when is_integer(ms) do
    total_seconds = div(ms, 1000)
    minutes = div(total_seconds, 60)
    seconds = rem(total_seconds, 60)
    "#{minutes}:#{String.pad_leading(Integer.to_string(seconds), 2, "0")}"
  end

  defp gate_door_colors(true, _is_open), do: {"rgba(239, 68, 68, 0.3)", "rgba(239, 68, 68, 0.6)"}
  defp gate_door_colors(false, true), do: {"rgba(34, 197, 94, 0.3)", "rgba(34, 197, 94, 0.5)"}
  defp gate_door_colors(false, false), do: {"rgba(99, 102, 241, 0.2)", "#6366f1"}

  defp extract_gate_state(gate) do
    state = gate.state || %{state: :unknown, persons_in_zone: 0, fault: false}
    gate_state = state[:state] || :unknown
    is_open = gate_state == :open
    last_opened_at = state[:last_opened_at]

    opened_at_ms =
      if is_open && last_opened_at do
        DateTime.to_unix(last_opened_at, :millisecond)
      else
        nil
      end

    %{
      gate_state: gate_state,
      is_open: is_open,
      has_fault: state[:fault] || false,
      persons: state[:persons_in_zone] || 0,
      opened_at_ms: opened_at_ms,
      last_open_duration_ms: state[:last_open_duration_ms],
      max_open_duration_ms: state[:max_open_duration_ms],
      min_open_duration_ms: state[:min_open_duration_ms]
    }
  end

  defp grafana_panel_url(site_key, panel_id, opts \\ []) do
    default_url =
      "https://grafana.e18n.net/d-solo/command-live/command-live?orgId=1&panelId=#{panel_id}&theme=dark"

    base_url = Sites.grafana_panel_url(site_key, panel_id, opts) || default_url

    case Keyword.get(opts, :refresh) do
      nil -> base_url
      refresh -> base_url <> "&refresh=#{refresh}"
    end
  end

  # HTTP helpers

  defp send_gate_open_request(url, site) do
    ensure_http_started()

    case :httpc.request(:post, {charlist(url), [], ~c"application/json", ~c""}, http_opts(), []) do
      {:ok, {{_, status, _}, _, body}} ->
        Logger.info("Gate open response for #{site}: status=#{status} body=#{inspect(body)}")

        if status == 200 do
          Phoenix.PubSub.broadcast(AveroCommand.PubSub, "gates", {:gate_opened, site})
        end

      {:error, reason} ->
        Logger.warning("Gate open failed for #{site}: #{inspect(reason)}")
    end
  end

  defp send_acc_simulate_request(site, pos) do
    ensure_http_started()
    url = "http://localhost:4000/api/acc/simulate"
    body = Jason.encode!(%{"pos" => pos, "site" => site})
    headers = [{~c"content-type", ~c"application/json"}]

    case :httpc.request(
           :post,
           {charlist(url), headers, ~c"application/json", charlist(body)},
           http_opts(),
           []
         ) do
      {:ok, {{_, status, _}, _, resp_body}} ->
        Logger.info(
          "ACC simulate response for #{site}/#{pos}: status=#{status} body=#{inspect(resp_body)}"
        )

      {:error, reason} ->
        Logger.warning("ACC simulate failed for #{site}/#{pos}: #{inspect(reason)}")
    end
  end

  defp ensure_http_started do
    :inets.start()
    :ssl.start()
  end

  defp http_opts, do: [{:timeout, @http_timeout}]
  defp charlist(s), do: String.to_charlist(s)

  @impl true
  def render(assigns) do
    ~H"""
    <div class="space-y-6">
      <!-- Header -->
      <div class="flex items-center justify-between">
        <h1 class="text-2xl font-bold text-gray-900 dark:text-white">
          Dashboard
          <span class="text-lg font-normal text-gray-500 dark:text-gray-400">
            · <%= @site_config && @site_config.name || "Unknown" %>
          </span>
        </h1>
        <div class="text-sm text-gray-500 dark:text-gray-400">
          Last updated: <%= Calendar.strftime(@last_updated, "%H:%M:%S") %>
        </div>
      </div>

      <!-- Gates with Stats -->
      <%= for gate <- @gates do %>
        <div class="grid grid-cols-1 lg:grid-cols-2 gap-6">
          <!-- Gate Card -->
          <.gate_card gate={gate} />

          <!-- Grafana Stats Grid -->
          <div class="grid grid-cols-3 gap-3">
            <.grafana_panel_component
              title="Gate Opens"
              src={grafana_panel_url(@selected_site, 4)}
            />
            <.grafana_panel_component
              title="Exits"
              src={grafana_panel_url(@selected_site, 5)}
            />
            <.grafana_panel_component
              title="Active Tracks"
              src={grafana_panel_url(@selected_site, 2)}
            />
            <.grafana_panel_component
              title="Current Open"
              src={grafana_panel_url(@selected_site, 50, refresh: "1s")}
            />
            <.grafana_panel_component
              title="Authorized"
              src={grafana_panel_url(@selected_site, 3)}
            />
          </div>
        </div>
      <% end %>

      <%= if @gates == [] do %>
        <div class="bg-white dark:bg-gray-800 rounded-lg border border-gray-200 dark:border-gray-700 p-6">
          <div class="text-center text-gray-500 dark:text-gray-400 mb-4">
            No gates registered for <%= @site_config && @site_config.name || "this site" %>. Waiting for gateway connections...
          </div>
          <div class="flex justify-center">
            <button
              phx-click="open_gate"
              class="px-6 py-3 bg-green-600 hover:bg-green-700 text-white font-medium rounded-lg transition-colors duration-200 flex items-center justify-center gap-2"
            >
              <svg xmlns="http://www.w3.org/2000/svg" class="h-5 w-5" viewBox="0 0 20 20" fill="currentColor">
                <path fill-rule="evenodd" d="M10 18a8 8 0 100-16 8 8 0 000 16zM9.555 7.168A1 1 0 008 8v4a1 1 0 001.555.832l3-2a1 1 0 000-1.664l-3-2z" clip-rule="evenodd" />
              </svg>
              Open Gate
            </button>
          </div>
        </div>
      <% end %>

      <!-- Gate Open Duration (1h) -->
      <.dash_card title="Gate Openings (1h)">
        <div class="h-48">
          <iframe
            src={grafana_panel_url(@selected_site, 51, from: "now-1h")}
            class="w-full h-full border-0"
            title="Gate Open Duration"
          ></iframe>
        </div>
      </.dash_card>

      <!-- POS Zones -->
      <.dash_card title="POS Zones">
        <div class="p-4">
          <%= if @pos_zones != [] do %>
            <div class={"grid grid-cols-#{min(length(@pos_zones), 5)} gap-3"}>
              <%= for zone <- @pos_zones do %>
                <.pos_zone zone={zone} />
              <% end %>
            </div>
            <div class="mt-4 flex items-center justify-between">
              <div class="flex items-center gap-4 text-xs text-gray-500 dark:text-gray-400">
                <div class="flex items-center gap-1">
                  <div class="w-3 h-3 rounded bg-gray-100 dark:bg-gray-700 border border-gray-200 dark:border-gray-600"></div>
                  <span>Empty</span>
                </div>
                <div class="flex items-center gap-1">
                  <div class="w-3 h-3 rounded bg-amber-100 dark:bg-amber-900/30 border-2 border-amber-400"></div>
                  <span>Occupied</span>
                </div>
                <div class="flex items-center gap-1">
                  <div class="w-3 h-3 rounded bg-green-100 dark:bg-green-900/30 border-2 border-green-400"></div>
                  <span>Paid</span>
                </div>
              </div>
              <%= if @selected_site == "avero" do %>
                <div class="flex items-center gap-3">
                  <span class="text-xs text-gray-500 dark:text-gray-400">
                    <%= pos_zone_total_count(@pos_zones) %> in POS · <%= @authorized_at_gate %> auth
                  </span>
                  <button
                    phx-click="simulate_acc"
                    phx-value-pos="POS_1"
                    class="px-3 py-1.5 bg-blue-600 hover:bg-blue-700 text-white text-xs font-medium rounded transition-colors"
                  >
                    ACC POS_1
                  </button>
                </div>
              <% end %>
            </div>
          <% else %>
            <div class="text-center py-4 text-gray-500 dark:text-gray-400 text-sm">
              No POS zones configured for this site
            </div>
          <% end %>
        </div>
      </.dash_card>

      <!-- POS Zone Occupancy Graph (60m) -->
      <.dash_card title="POS Zone Occupancy (60m)">
        <div class="h-48">
          <iframe
            src={grafana_panel_url(@selected_site, 10, from: "now-60m")}
            class="w-full h-full border-0"
            title="POS Zone Occupancy"
          ></iframe>
        </div>
      </.dash_card>

      <!-- Recent Journeys -->
      <.dash_card title="Recent Journeys">
        <div class="divide-y divide-gray-100 dark:divide-gray-700">
          <%= for journey <- @journeys do %>
            <.link
              navigate={~p"/journeys"}
              class="block px-4 py-3 hover:bg-gray-50 dark:hover:bg-gray-700/50 transition-colors"
            >
              <div class="flex items-center justify-between">
                <div class="flex items-center gap-3">
                  <div class={[
                    "w-2 h-2 rounded-full shrink-0",
                    journey.outcome == "authorized" && "bg-green-500",
                    journey.outcome == "blocked" && "bg-red-500",
                    journey.outcome not in ["authorized", "blocked"] && "bg-gray-400"
                  ]}></div>
                  <div class="min-w-0">
                    <div class="flex items-center gap-2">
                      <span class="text-sm font-medium text-gray-900 dark:text-white">
                        #<%= journey.person_id %>
                      </span>
                      <span class={[
                        "px-1.5 py-0.5 text-xs font-medium rounded",
                        journey.outcome == "authorized" && "bg-green-100 text-green-700 dark:bg-green-900/30 dark:text-green-400",
                        journey.outcome == "blocked" && "bg-red-100 text-red-700 dark:bg-red-900/30 dark:text-red-400",
                        journey.outcome not in ["authorized", "blocked"] && "bg-gray-100 text-gray-600 dark:bg-gray-700 dark:text-gray-400"
                      ]}>
                        <%= journey.outcome || "unknown" %>
                      </span>
                      <%= if journey.acc_matched do %>
                        <span class="px-1.5 py-0.5 text-xs font-medium rounded bg-blue-100 text-blue-700 dark:bg-blue-900/30 dark:text-blue-400">
                          ACC
                        </span>
                      <% end %>
                    </div>
                    <div class="text-xs text-gray-500 dark:text-gray-400 mt-0.5">
                      <%= if journey.payment_zone do %>
                        <%= journey.payment_zone %> ·
                      <% end %>
                      <%= if journey.total_pos_dwell_ms && journey.total_pos_dwell_ms > 0 do %>
                        <%= div(journey.total_pos_dwell_ms, 1000) %>s dwell
                      <% else %>
                        no dwell
                      <% end %>
                    </div>
                  </div>
                </div>
                <div class="text-xs text-gray-500 dark:text-gray-400 shrink-0 ml-3">
                  <%= if journey.ended_at do %>
                    <%= Calendar.strftime(journey.ended_at, "%H:%M:%S") %>
                  <% end %>
                </div>
              </div>
            </.link>
          <% end %>
          <%= if @journeys == [] do %>
            <div class="px-4 py-8 text-center text-gray-500 dark:text-gray-400 text-sm">
              No recent journeys
            </div>
          <% end %>
        </div>
        <div class="px-4 py-2 border-t border-gray-100 dark:border-gray-700">
          <.link navigate={~p"/journeys"} class="text-sm text-blue-600 hover:text-blue-700 dark:text-blue-400 dark:hover:text-blue-300">
            View all journeys →
          </.link>
        </div>
      </.dash_card>
    </div>
    """
  end

  # Components

  attr(:title, :string, required: true)
  attr(:src, :string, required: true)

  defp grafana_panel_component(assigns) do
    ~H"""
    <div class="bg-white dark:bg-gray-800 rounded-lg border border-gray-200 dark:border-gray-700 overflow-hidden">
      <div class="h-24">
        <iframe
          src={@src}
          class="w-full h-full border-0"
          title={@title}
        ></iframe>
      </div>
    </div>
    """
  end

  attr(:title, :string, default: nil)
  slot(:inner_block, required: true)

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

  attr(:gate, :map, required: true)

  defp gate_card(assigns) do
    assigns = assign(assigns, extract_gate_state(assigns.gate))

    ~H"""
    <div class="rounded-lg border border-gray-200 dark:border-gray-700 bg-white dark:bg-gray-800 p-6">
      <div class="flex items-center justify-between mb-4">
        <div>
          <h3 class="font-semibold text-gray-900 dark:text-white">Gate <%= @gate.gate_id %></h3>
          <p class="text-xs text-gray-500 dark:text-gray-400"><%= @gate.site %></p>
        </div>
        <div class="px-3 py-1 rounded-full text-xs font-medium bg-gray-100 text-gray-700 dark:bg-gray-700 dark:text-gray-300">
          <%= if @has_fault, do: "FAULT", else: String.upcase(to_string(@gate_state)) %>
        </div>
      </div>

      <div class="flex justify-center my-6">
        <.gate_animation is_open={@is_open} has_fault={@has_fault} />
      </div>

      <div class="space-y-2 text-sm">
        <div class="flex items-center justify-between">
          <span class="text-gray-500 dark:text-gray-400">Persons in zone</span>
          <span class="font-medium text-gray-900 dark:text-white tabular-nums"><%= @persons %></span>
        </div>

        <%= if @is_open && @opened_at_ms do %>
          <div class="flex items-center justify-between">
            <span class="text-gray-500 dark:text-gray-400">Open for</span>
            <span
              id={"gate-timer-#{@gate.site}-#{@gate.gate_id}"}
              phx-hook="LiveTimer"
              data-started-at={@opened_at_ms}
              class="font-medium text-gray-900 dark:text-white tabular-nums"
            >--:--</span>
          </div>
        <% end %>

        <%= if @last_open_duration_ms do %>
          <div class="flex items-center justify-between">
            <span class="text-gray-500 dark:text-gray-400">Last open</span>
            <span class="font-medium text-gray-900 dark:text-white tabular-nums"><%= format_duration_ms(@last_open_duration_ms) %></span>
          </div>
        <% end %>

        <%= if @max_open_duration_ms && @min_open_duration_ms do %>
          <div class="flex items-center justify-between">
            <span class="text-gray-500 dark:text-gray-400">Max / Min</span>
            <span class="font-medium text-gray-900 dark:text-white tabular-nums">
              <%= format_duration_ms(@max_open_duration_ms) %> / <%= format_duration_ms(@min_open_duration_ms) %>
            </span>
          </div>
        <% end %>
      </div>

      <div class="mt-4 pt-4 border-t border-gray-200 dark:border-gray-700">
        <button
          phx-click="open_gate"
          class="w-full px-4 py-2 bg-gray-900 hover:bg-gray-800 dark:bg-gray-700 dark:hover:bg-gray-600 text-white font-medium rounded-lg transition-colors duration-200 flex items-center justify-center gap-2"
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

  attr(:is_open, :boolean, default: false)
  attr(:has_fault, :boolean, default: false)

  defp gate_animation(assigns) do
    {door_fill, door_stroke} = gate_door_colors(assigns.has_fault, assigns.is_open)
    {left_x, right_x} = if assigns.is_open, do: {36, 356}, else: {141, 251}

    assigns =
      assign(assigns,
        door_fill: door_fill,
        door_stroke: door_stroke,
        left_x: left_x,
        right_x: right_x,
        state_key: if(assigns.is_open, do: "open", else: "closed")
      )

    ~H"""
    <svg id={"gate-svg-#{@state_key}"} viewBox="0 0 500 160" class="w-full max-w-md h-auto">
      <!-- LEFT DOOR -->
      <rect
        x={@left_x} y="30" width="110" height="100"
        fill={@door_fill}
        stroke={@door_stroke}
        stroke-width="2"
        rx="4"
      />
      <!-- RIGHT DOOR -->
      <rect
        x={@right_x} y="30" width="110" height="100"
        fill={@door_fill}
        stroke={@door_stroke}
        stroke-width="2"
        rx="4"
      />
      <!-- LEFT PILLAR -->
      <rect x="30" y="15" width="110" height="130" rx="8" fill="#334155" stroke="#1e293b" stroke-width="0" />
      <!-- RIGHT PILLAR -->
      <rect x="360" y="15" width="110" height="130" rx="8" fill="#334155" stroke="#1e293b" stroke-width="0" />
    </svg>
    """
  end

  attr(:zone, :map, required: true)

  defp pos_zone(assigns) do
    # Extract zone number: "POS_1" -> "1", "POS_2" -> "2", etc.
    zone_num =
      case assigns.zone.id do
        "POS_" <> n -> n
        "100" <> n -> n
        id -> id
      end

    # Calculate current dwell (running timer if occupied)
    current_dwell_ms =
      if assigns.zone.occupied_since do
        DateTime.diff(DateTime.utc_now(), assigns.zone.occupied_since, :millisecond)
      else
        0
      end

    total_ms = assigns.zone.total_dwell_ms + current_dwell_ms

    assigns =
      assigns
      |> assign(:zone_num, zone_num)
      |> assign(:total_ms, total_ms)

    ~H"""
    <div class={[
      "relative rounded-lg p-3 text-center transition-all min-h-[70px]",
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
      <%!-- Always show count to prevent layout jump --%>
      <div class={[
        "text-xs tabular-nums",
        @zone.count > 0 && "text-gray-700 dark:text-gray-300 font-medium",
        @zone.count == 0 && "text-gray-400 dark:text-gray-500"
      ]}>
        <%= @zone.count %> people
      </div>
      <%!-- Show dwell time if any --%>
      <%= if @total_ms > 0 do %>
        <div class="text-xs text-gray-500 dark:text-gray-400 tabular-nums">
          <%= format_dwell_ms(@total_ms) %>
        </div>
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

  defp format_dwell_ms(ms) when ms < 1000, do: "<1s"
  defp format_dwell_ms(ms) when ms < 60_000, do: "#{div(ms, 1000)}s"

  defp format_dwell_ms(ms) when ms < 3_600_000 do
    mins = div(ms, 60_000)
    secs = div(rem(ms, 60_000), 1000)
    "#{mins}m #{secs}s"
  end

  defp format_dwell_ms(ms) do
    hours = div(ms, 3_600_000)
    mins = div(rem(ms, 3_600_000), 60_000)
    "#{hours}h #{mins}m"
  end
end
