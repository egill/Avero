defmodule AveroCommandWeb.DashboardLive do
  @moduledoc """
  Real-time dashboard showing gate status with animations.
  Displays gate open/close states, persons in zone, and recent events.
  """
  use AveroCommandWeb, :live_view

  alias AveroCommand.Entities.GateRegistry

  @refresh_interval 1000

  @impl true
  def mount(_params, _session, socket) do
    if connected?(socket) do
      # Subscribe to gate events
      Phoenix.PubSub.subscribe(AveroCommand.PubSub, "gates")
      # Schedule periodic refresh
      Process.send_after(self(), :refresh, @refresh_interval)
    end

    gates = load_gates()

    {:ok, assign(socket,
      page_title: "Dashboard",
      gates: gates,
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

  defp load_gates do
    GateRegistry.list_all()
    |> Enum.sort_by(fn g -> {g.site, g.gate_id} end)
  end

  @impl true
  def render(assigns) do
    ~H"""
    <div class="space-y-6">
      <!-- Header -->
      <div class="flex flex-col sm:flex-row sm:items-center sm:justify-between gap-4">
        <div>
          <h1 class="text-2xl font-bold text-gray-900 dark:text-white">Gate Dashboard</h1>
          <p class="text-sm text-gray-500 dark:text-gray-400">
            Real-time gate status • Last updated: <%= format_time(@last_updated) %>
          </p>
        </div>
        <div class="flex items-center gap-2">
          <span class="flex h-3 w-3 relative">
            <span class="animate-ping absolute inline-flex h-full w-full rounded-full bg-green-400 opacity-75"></span>
            <span class="relative inline-flex rounded-full h-3 w-3 bg-green-500"></span>
          </span>
          <span class="text-sm text-gray-600 dark:text-gray-300">Live</span>
        </div>
      </div>

      <!-- Gates Grid -->
      <%= if Enum.empty?(@gates) do %>
        <div class="rounded-xl border-2 border-dashed border-gray-300 dark:border-gray-700 p-12 text-center">
          <svg class="mx-auto h-12 w-12 text-gray-400" fill="none" viewBox="0 0 24 24" stroke="currentColor">
            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5" d="M19 11H5m14 0a2 2 0 012 2v6a2 2 0 01-2 2H5a2 2 0 01-2-2v-6a2 2 0 012-2m14 0V9a2 2 0 00-2-2M5 11V9a2 2 0 012-2m0 0V5a2 2 0 012-2h6a2 2 0 012 2v2M7 7h10" />
          </svg>
          <h3 class="mt-4 text-lg font-medium text-gray-900 dark:text-white">No active gates</h3>
          <p class="mt-2 text-sm text-gray-500 dark:text-gray-400">
            Gates will appear here when they become active.
          </p>
        </div>
      <% else %>
        <div class="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4 gap-6">
          <%= for gate <- @gates do %>
            <.gate_card gate={gate} />
          <% end %>
        </div>
      <% end %>

      <!-- Stats Overview -->
      <div class="grid grid-cols-2 md:grid-cols-4 gap-4">
        <.stat_card
          label="Active Gates"
          value={length(@gates)}
          icon="gate"
        />
        <.stat_card
          label="Open"
          value={Enum.count(@gates, fn g -> g.state && g.state.state == :open end)}
          icon="open"
          color="green"
        />
        <.stat_card
          label="Closed"
          value={Enum.count(@gates, fn g -> g.state && g.state.state == :closed end)}
          icon="closed"
          color="gray"
        />
        <.stat_card
          label="Faults"
          value={Enum.count(@gates, fn g -> g.state && g.state.fault end)}
          icon="fault"
          color="red"
        />
      </div>
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
      "relative rounded-xl border p-6 transition-all duration-300",
      @has_fault && "border-red-500 bg-red-50 dark:bg-red-900/20 dark:border-red-700",
      !@has_fault && @is_open && "border-green-500 bg-green-50 dark:bg-green-900/20 dark:border-green-700",
      !@has_fault && !@is_open && "border-gray-200 bg-white dark:bg-gray-800 dark:border-gray-700"
    ]}>
      <!-- Gate Header -->
      <div class="flex items-center justify-between mb-4">
        <div>
          <h3 class="font-semibold text-gray-900 dark:text-white">
            Gate <%= @gate.gate_id %>
          </h3>
          <p class="text-xs text-gray-500 dark:text-gray-400"><%= @gate.site %></p>
        </div>
        <div class={[
          "px-2 py-1 rounded-full text-xs font-medium",
          @has_fault && "bg-red-100 text-red-700 dark:bg-red-900/50 dark:text-red-300",
          !@has_fault && @is_open && "bg-green-100 text-green-700 dark:bg-green-900/50 dark:text-green-300",
          !@has_fault && !@is_open && "bg-gray-100 text-gray-700 dark:bg-gray-700 dark:text-gray-300"
        ]}>
          <%= if @has_fault, do: "FAULT", else: String.upcase(to_string(@gate_state)) %>
        </div>
      </div>

      <!-- Gate Animation -->
      <div class="flex justify-center my-6">
        <.gate_animation is_open={@is_open} has_fault={@has_fault} />
      </div>

      <!-- Gate Info -->
      <div class="space-y-2 text-sm">
        <div class="flex justify-between items-center">
          <span class="text-gray-500 dark:text-gray-400">Persons in zone</span>
          <span class={[
            "font-medium",
            @persons > 0 && "text-brand-500",
            @persons == 0 && "text-gray-700 dark:text-gray-300"
          ]}>
            <%= @persons %>
          </span>
        </div>
        <%= if @state[:last_opened_at] do %>
          <div class="flex justify-between items-center">
            <span class="text-gray-500 dark:text-gray-400">Last opened</span>
            <span class="text-gray-700 dark:text-gray-300 text-xs">
              <%= format_relative_time(@state[:last_opened_at]) %>
            </span>
          </div>
        <% end %>
      </div>
    </div>
    """
  end

  attr :is_open, :boolean, default: false
  attr :has_fault, :boolean, default: false

  defp gate_animation(assigns) do
    ~H"""
    <div class="relative w-32 h-24">
      <!-- Gate Frame -->
      <div class="absolute inset-x-0 top-0 h-2 bg-gray-400 dark:bg-gray-600 rounded-t"></div>
      <div class="absolute left-0 top-0 w-2 h-full bg-gray-400 dark:bg-gray-600 rounded-l"></div>
      <div class="absolute right-0 top-0 w-2 h-full bg-gray-400 dark:bg-gray-600 rounded-r"></div>

      <!-- Gate Door (Left) -->
      <div
        class={[
          "absolute top-2 left-2 w-[calc(50%-4px)] h-[calc(100%-8px)] rounded transition-all duration-500 ease-in-out origin-left",
          @has_fault && "bg-red-500 animate-pulse",
          !@has_fault && @is_open && "bg-green-500 -rotate-90 scale-x-50",
          !@has_fault && !@is_open && "bg-gray-500 dark:bg-gray-600"
        ]}
      >
        <div class="absolute right-1 top-1/2 -translate-y-1/2 w-1 h-4 bg-white/50 rounded"></div>
      </div>

      <!-- Gate Door (Right) -->
      <div
        class={[
          "absolute top-2 right-2 w-[calc(50%-4px)] h-[calc(100%-8px)] rounded transition-all duration-500 ease-in-out origin-right",
          @has_fault && "bg-red-500 animate-pulse",
          !@has_fault && @is_open && "bg-green-500 rotate-90 scale-x-50",
          !@has_fault && !@is_open && "bg-gray-500 dark:bg-gray-600"
        ]}
      >
        <div class="absolute left-1 top-1/2 -translate-y-1/2 w-1 h-4 bg-white/50 rounded"></div>
      </div>

      <!-- Open indicator glow -->
      <%= if @is_open && !@has_fault do %>
        <div class="absolute inset-0 flex items-center justify-center">
          <div class="w-8 h-8 bg-green-400/30 rounded-full animate-ping"></div>
        </div>
      <% end %>
    </div>
    """
  end

  attr :label, :string, required: true
  attr :value, :integer, required: true
  attr :icon, :string, required: true
  attr :color, :string, default: "brand"

  defp stat_card(assigns) do
    ~H"""
    <div class="rounded-xl border border-gray-200 dark:border-gray-700 bg-white dark:bg-gray-800 p-4">
      <div class="flex items-center gap-3">
        <div class={[
          "flex h-10 w-10 items-center justify-center rounded-lg",
          @color == "brand" && "bg-brand-100 dark:bg-brand-900/30",
          @color == "green" && "bg-green-100 dark:bg-green-900/30",
          @color == "red" && "bg-red-100 dark:bg-red-900/30",
          @color == "gray" && "bg-gray-100 dark:bg-gray-700"
        ]}>
          <.stat_icon icon={@icon} color={@color} />
        </div>
        <div>
          <p class="text-2xl font-bold text-gray-900 dark:text-white"><%= @value %></p>
          <p class="text-xs text-gray-500 dark:text-gray-400"><%= @label %></p>
        </div>
      </div>
    </div>
    """
  end

  attr :icon, :string, required: true
  attr :color, :string, default: "brand"

  defp stat_icon(assigns) do
    ~H"""
    <%= case @icon do %>
      <% "gate" -> %>
        <svg class={["w-5 h-5", icon_color(@color)]} fill="none" viewBox="0 0 24 24" stroke="currentColor">
          <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M19 11H5m14 0a2 2 0 012 2v6a2 2 0 01-2 2H5a2 2 0 01-2-2v-6a2 2 0 012-2m14 0V9a2 2 0 00-2-2M5 11V9a2 2 0 012-2m0 0V5a2 2 0 012-2h6a2 2 0 012 2v2M7 7h10" />
        </svg>
      <% "open" -> %>
        <svg class={["w-5 h-5", icon_color(@color)]} fill="none" viewBox="0 0 24 24" stroke="currentColor">
          <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M5 11l7-7 7 7M5 19l7-7 7 7" />
        </svg>
      <% "closed" -> %>
        <svg class={["w-5 h-5", icon_color(@color)]} fill="none" viewBox="0 0 24 24" stroke="currentColor">
          <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M19 13l-7 7-7-7m14-8l-7 7-7-7" />
        </svg>
      <% "fault" -> %>
        <svg class={["w-5 h-5", icon_color(@color)]} fill="none" viewBox="0 0 24 24" stroke="currentColor">
          <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 9v2m0 4h.01m-6.938 4h13.856c1.54 0 2.502-1.667 1.732-3L13.732 4c-.77-1.333-2.694-1.333-3.464 0L3.34 16c-.77 1.333.192 3 1.732 3z" />
        </svg>
      <% _ -> %>
        <span></span>
    <% end %>
    """
  end

  defp icon_color("brand"), do: "text-brand-500"
  defp icon_color("green"), do: "text-green-500"
  defp icon_color("red"), do: "text-red-500"
  defp icon_color("gray"), do: "text-gray-500 dark:text-gray-400"
  defp icon_color(_), do: "text-gray-500"

  defp format_time(datetime) do
    Calendar.strftime(datetime, "%H:%M:%S")
  end

  defp format_relative_time(nil), do: "-"
  defp format_relative_time(datetime) when is_binary(datetime) do
    case DateTime.from_iso8601(datetime) do
      {:ok, dt, _} -> format_relative_time(dt)
      _ -> datetime
    end
  end
  defp format_relative_time(%DateTime{} = datetime) do
    diff = DateTime.diff(DateTime.utc_now(), datetime, :second)
    cond do
      diff < 60 -> "#{diff}s ago"
      diff < 3600 -> "#{div(diff, 60)}m ago"
      diff < 86400 -> "#{div(diff, 3600)}h ago"
      true -> Calendar.strftime(datetime, "%b %d %H:%M")
    end
  end
  defp format_relative_time(_), do: "-"
end
