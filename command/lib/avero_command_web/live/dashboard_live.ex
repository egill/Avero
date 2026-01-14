defmodule AveroCommandWeb.DashboardLive do
  @moduledoc """
  Real-time dashboard showing gate status, POS zones, journeys, and Grafana metrics.
  Site-aware: displays data for the currently selected site.
  """
  use AveroCommandWeb, :live_view

  alias AveroCommand.Entities.GateRegistry
  alias AveroCommand.Journeys
  alias AveroCommand.Sites

  @refresh_interval 1000

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

    gates = load_gates(site_config)
    journeys = load_recent_journeys(selected_sites)
    pos_zones = build_pos_zones(site_config)

    {:ok,
     assign(socket,
       gates: gates,
       journeys: journeys,
       pos_zones: pos_zones,
       last_updated: DateTime.utc_now()
     )}
  end

  @impl true
  def handle_info(:refresh, socket) do
    Process.send_after(self(), :refresh, @refresh_interval)
    site_config = socket.assigns[:site_config]
    gates = load_gates(site_config)
    {:noreply, assign(socket, gates: gates, last_updated: DateTime.utc_now())}
  end

  def handle_info({:gate_event, _event}, socket) do
    site_config = socket.assigns[:site_config]
    gates = load_gates(site_config)
    {:noreply, assign(socket, gates: gates, last_updated: DateTime.utc_now())}
  end

  def handle_info({:journey_created, _journey}, socket) do
    journeys = load_recent_journeys(socket.assigns[:selected_sites] || [])
    {:noreply, assign(socket, journeys: journeys)}
  end

  def handle_info({:zone_event, %{zone_id: zone_id, event_type: type}}, socket) do
    pos_zones = update_pos_zone(socket.assigns.pos_zones, zone_id, type)
    {:noreply, assign(socket, pos_zones: pos_zones)}
  end

  def handle_info(_msg, socket), do: {:noreply, socket}

  @impl true
  def handle_event("open_gate", _params, socket) do
    require Logger

    selected_site = socket.assigns[:selected_site]
    gateway_url = Sites.gateway_url(selected_site, "/gate/open")

    if gateway_url do
      Task.start(fn ->
        url = String.to_charlist(gateway_url)
        :inets.start()
        :ssl.start()

        case :httpc.request(:post, {url, [], ~c"application/json", ~c""}, [{:timeout, 5000}], []) do
          {:ok, {{_, status, _}, _, body}} ->
            Logger.info("Gate open response for #{selected_site}: status=#{status} body=#{inspect(body)}")

            if status == 200 do
              Phoenix.PubSub.broadcast(AveroCommand.PubSub, "gates", {:gate_opened, selected_site})
            end

          {:error, reason} ->
            Logger.warning("Gate open failed for #{selected_site}: #{inspect(reason)}")
        end
      end)
    end

    {:noreply, socket}
  end

  # Handle site switching - reload data for new site
  def handle_event("switch_site", %{"site" => site_key}, socket) do
    # The SiteFilterHook handles updating the assigns, but we need to reload data
    site_config = Sites.get(site_key)
    # Use site key ("netto") not site ID ("AP-NETTO-GR-01") - database uses key
    selected_sites = [site_key]

    gates = load_gates(site_config)
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
    now = DateTime.utc_now()

    Enum.map(pos_zones, fn zone ->
      if zone.id == zone_id do
        case type do
          :zone_entry ->
            # Start timer if this is the first person
            occupied_since = zone.occupied_since || now

            %{zone | occupied: true, count: zone.count + 1, occupied_since: occupied_since}

          :zone_exit ->
            new_count = max(0, zone.count - 1)

            # Accumulate dwell time when zone becomes empty
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
                paid: if(new_count == 0, do: false, else: zone.paid),
                occupied_since: occupied_since,
                total_dwell_ms: total_dwell_ms
            }

          :payment ->
            %{zone | paid: true}

          _ ->
            zone
        end
      else
        zone
      end
    end)
  end

  defp load_gates(nil), do: []

  defp load_gates(site_config) do
    GateRegistry.list_all()
    |> Enum.filter(fn g -> g.site == site_config.id end)
    |> Enum.sort_by(fn g -> {g.site, g.gate_id} end)
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

  defp format_duration(seconds) when seconds < 60 do
    "#{seconds}s"
  end

  defp format_duration(seconds) do
    minutes = div(seconds, 60)
    secs = rem(seconds, 60)

    if secs == 0 do
      "#{minutes}m"
    else
      "#{minutes}m #{secs}s"
    end
  end

  # Get Grafana panel URL for current site
  defp grafana_panel(site_key, panel_id, opts \\ []) do
    Sites.grafana_panel_url(site_key, panel_id, opts) ||
      "https://grafana.e18n.net/d-solo/command-live/command-live?orgId=1&panelId=#{panel_id}&theme=dark"
  end

  @impl true
  def render(assigns) do
    ~H"""
    <div class="space-y-6">
      <!-- Header -->
      <div class="flex items-center justify-between">
        <h1 class="text-2xl font-bold text-gray-900 dark:text-white">
          Gate Dashboard
          <span class="text-lg font-normal text-gray-500 dark:text-gray-400">
            - <%= @site_config && @site_config.name || "Unknown" %>
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
          <div class="grid grid-cols-2 gap-3">
            <.grafana_panel
              title="Gate Opens"
              src={grafana_panel(@selected_site, 4)}
            />
            <.grafana_panel
              title="Exits"
              src={grafana_panel(@selected_site, 5)}
            />
            <.grafana_panel
              title="Current Open"
              src={grafana_panel(@selected_site, 50)}
            />
            <.grafana_panel
              title="Authorized"
              src={grafana_panel(@selected_site, 3)}
            />
          </div>
        </div>
      <% end %>

      <%= if @gates == [] do %>
        <div class="text-center py-12 text-gray-500 dark:text-gray-400 bg-white dark:bg-gray-800 rounded-lg border border-gray-200 dark:border-gray-700">
          No gates registered for <%= @site_config && @site_config.name || "this site" %>. Waiting for gateway connections...
        </div>
      <% end %>

      <!-- Gate Open Duration (1h) -->
      <.dash_card title="Gate Openings (1h)">
        <div class="h-48">
          <iframe
            src={grafana_panel(@selected_site, 51, from: "now-1h")}
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
            <div class="mt-4 flex items-center gap-4 text-xs text-gray-500 dark:text-gray-400">
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
            src={grafana_panel(@selected_site, 10, from: "now-60m")}
            class="w-full h-full border-0"
            title="POS Zone Occupancy"
          ></iframe>
        </div>
      </.dash_card>

      <!-- Recent Journeys -->
      <.dash_card title="Recent Journeys">
        <div class="divide-y divide-gray-100 dark:divide-gray-700">
          <%= for journey <- @journeys do %>
            <div class="px-4 py-3 flex items-center justify-between">
              <div class="flex items-center gap-3">
                <div class={[
                  "w-2 h-2 rounded-full",
                  journey.outcome == "authorized" && "bg-green-500",
                  journey.outcome == "blocked" && "bg-red-500",
                  journey.outcome not in ["authorized", "blocked"] && "bg-gray-400"
                ]}></div>
                <div>
                  <div class="text-sm font-medium text-gray-900 dark:text-white">
                    <%= journey.outcome || "unknown" %>
                  </div>
                  <div class="text-xs text-gray-500 dark:text-gray-400">
                    <%= if journey.total_pos_dwell_ms && journey.total_pos_dwell_ms > 0, do: "#{div(journey.total_pos_dwell_ms, 1000)}s dwell", else: "no dwell" %>
                  </div>
                </div>
              </div>
              <div class="text-xs text-gray-500 dark:text-gray-400">
                <%= if journey.ended_at do %>
                  <%= Calendar.strftime(journey.ended_at, "%H:%M:%S") %>
                <% end %>
              </div>
            </div>
          <% end %>
          <%= if @journeys == [] do %>
            <div class="px-4 py-8 text-center text-gray-500 dark:text-gray-400 text-sm">
              No recent journeys
            </div>
          <% end %>
        </div>
      </.dash_card>
    </div>
    """
  end

  # Grafana embedded panel component
  attr :title, :string, required: true
  attr :src, :string, required: true

  defp grafana_panel(assigns) do
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

  # === Components ===

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
    state = assigns.gate.state || %{state: :unknown, persons_in_zone: 0, fault: false}
    gate_state = state[:state] || :unknown
    is_open = gate_state == :open
    has_fault = state[:fault] || false
    persons = state[:persons_in_zone] || 0
    exits_this_cycle = state[:exits_this_cycle] || 0
    last_opened_at = state[:last_opened_at]

    # Calculate how long gate has been open
    open_duration_seconds =
      if is_open && last_opened_at do
        DateTime.diff(DateTime.utc_now(), last_opened_at, :second)
      else
        0
      end

    assigns =
      assign(assigns,
        gate_state: gate_state,
        is_open: is_open,
        has_fault: has_fault,
        persons: persons,
        exits_this_cycle: exits_this_cycle,
        open_duration_seconds: open_duration_seconds,
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

      <%= if @is_open do %>
        <div class="mt-3 p-3 bg-green-100 dark:bg-green-900/30 rounded-lg">
          <div class="flex items-center justify-between text-sm">
            <span class="text-green-700 dark:text-green-300">Open for</span>
            <span class="font-bold text-green-800 dark:text-green-200 tabular-nums">
              <%= format_duration(@open_duration_seconds) %>
            </span>
          </div>
          <div class="flex items-center justify-between text-sm mt-1">
            <span class="text-green-700 dark:text-green-300">Exits this cycle</span>
            <span class="font-bold text-green-800 dark:text-green-200">
              <%= @exits_this_cycle %>
            </span>
          </div>
        </div>
      <% end %>

      <div class="mt-4 pt-4 border-t border-gray-200 dark:border-gray-700">
        <button
          phx-click="open_gate"
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

  attr(:is_open, :boolean, default: false)
  attr(:has_fault, :boolean, default: false)

  defp gate_animation(assigns) do
    # Colors from original Go dashboard (dashboard.monitor.js)
    # Open: green rgba(34, 197, 94, 0.3) / rgba(34, 197, 94, 0.5)
    # Closed: indigo rgba(99, 102, 241, 0.2) / #6366f1
    # Fault: red
    {door_fill, door_stroke, left_transform, right_transform} =
      cond do
        assigns.has_fault ->
          {"rgba(239, 68, 68, 0.3)", "rgba(239, 68, 68, 0.6)", "translateX(0)", "translateX(0)"}

        assigns.is_open ->
          {"rgba(34, 197, 94, 0.3)", "rgba(34, 197, 94, 0.5)", "translateX(-105px)",
           "translateX(105px)"}

        true ->
          {"rgba(99, 102, 241, 0.2)", "#6366f1", "translateX(0)", "translateX(0)"}
      end

    assigns =
      assign(assigns,
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
