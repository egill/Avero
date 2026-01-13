defmodule AveroCommandWeb.AccLive do
  @moduledoc """
  LiveView for ACC (payment terminal) monitoring.

  Shows:
  - Active POS zones with ACC entities
  - Received/matched/unmatched payment counts
  - Match rate percentages
  - Time since last payment per zone
  """
  use AveroCommandWeb, :live_view

  alias AveroCommand.Entities.AccRegistry

  @refresh_interval 5000

  @impl true
  def mount(_params, _session, socket) do
    if connected?(socket) do
      # Subscribe to ACC-related events
      Phoenix.PubSub.subscribe(AveroCommand.PubSub, "acc")
      Phoenix.PubSub.subscribe(AveroCommand.PubSub, "zones")
      # Schedule periodic refresh
      Process.send_after(self(), :refresh, @refresh_interval)
    end

    acc_entities = load_acc_entities()

    {:ok,
     socket
     |> assign(:acc_entities, acc_entities)
     |> assign(:page_title, "ACC Monitor")
     |> assign(:last_updated, DateTime.utc_now())}
  end

  @impl true
  def handle_info(:refresh, socket) do
    Process.send_after(self(), :refresh, @refresh_interval)
    acc_entities = load_acc_entities()
    {:noreply, assign(socket, acc_entities: acc_entities, last_updated: DateTime.utc_now())}
  end

  @impl true
  def handle_info({:acc_event, _event}, socket) do
    acc_entities = load_acc_entities()
    {:noreply, assign(socket, acc_entities: acc_entities, last_updated: DateTime.utc_now())}
  end

  @impl true
  def handle_info(_msg, socket), do: {:noreply, socket}

  defp load_acc_entities do
    AccRegistry.list_all()
    |> Enum.map(fn entity ->
      state = entity.state || %{}
      Map.merge(entity, %{
        received_count: state[:received_count] || 0,
        matched_count: state[:matched_count] || 0,
        unmatched_count: state[:unmatched_count] || 0,
        match_rate: state[:match_rate],
        last_event_at: state[:last_event_at],
        started_at: state[:started_at]
      })
    end)
    |> Enum.sort_by(fn e -> {e.site, e.pos_zone} end)
  end

  @impl true
  def render(assigns) do
    ~H"""
    <div class="acc-monitor">
      <div class="mb-4 sm:mb-6 flex flex-col sm:flex-row sm:items-center sm:justify-between gap-3">
        <div class="flex items-center space-x-4">
          <h2 class="text-base sm:text-lg font-semibold text-gray-900 dark:text-white">
            ACC Monitor
          </h2>
          <span class="text-xs text-gray-500 dark:text-gray-400">
            Updated: <%= format_time(@last_updated) %>
          </span>
        </div>
        <div class="flex items-center space-x-2">
          <span class="text-sm text-gray-600 dark:text-gray-400">
            <%= length(@acc_entities) %> active zones
          </span>
        </div>
      </div>

      <%= if Enum.empty?(@acc_entities) do %>
        <.acc_empty_state />
      <% else %>
        <div class="grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
          <%= for entity <- @acc_entities do %>
            <.acc_card entity={entity} />
          <% end %>
        </div>

        <div class="mt-6">
          <.summary_stats entities={@acc_entities} />
        </div>
      <% end %>
    </div>
    """
  end

  # Empty state component
  defp acc_empty_state(assigns) do
    ~H"""
    <div class="text-center py-12 bg-white rounded-lg shadow dark:bg-gray-800">
      <svg
        class="mx-auto h-12 w-12 text-gray-400"
        fill="none"
        viewBox="0 0 24 24"
        stroke="currentColor"
      >
        <path
          stroke-linecap="round"
          stroke-linejoin="round"
          stroke-width="2"
          d="M3 10h18M7 15h1m4 0h1m-7 4h12a3 3 0 003-3V8a3 3 0 00-3-3H6a3 3 0 00-3 3v8a3 3 0 003 3z"
        />
      </svg>
      <p class="mt-2 text-gray-500 dark:text-gray-400">No active ACC zones</p>
      <p class="text-sm text-gray-400 dark:text-gray-500 mt-1">
        Payment terminals will appear here when events are received
      </p>
    </div>
    """
  end

  # Individual ACC zone card
  attr :entity, :map, required: true

  defp acc_card(assigns) do
    time_since = time_since_last_event(assigns.entity.last_event_at)
    health_status = health_status(time_since)

    assigns =
      assigns
      |> assign(:time_since, time_since)
      |> assign(:health_status, health_status)

    ~H"""
    <div class={[
      "bg-white dark:bg-gray-800 rounded-lg border p-4 transition-all",
      health_border(@health_status)
    ]}>
      <div class="flex items-center justify-between mb-3">
        <div>
          <h3 class="font-semibold text-gray-900 dark:text-white">
            <%= @entity.pos_zone %>
          </h3>
          <p class="text-xs text-gray-500 dark:text-gray-400">
            <%= @entity.site %>
          </p>
        </div>
        <.health_badge status={@health_status} />
      </div>

      <div class="space-y-2">
        <.stat_row label="Received" value={@entity.received_count} color="blue" />
        <.stat_row label="Matched" value={@entity.matched_count} color="green" />
        <.stat_row label="Unmatched" value={@entity.unmatched_count} color="red" />
      </div>

      <div class="mt-3 pt-3 border-t border-gray-200 dark:border-gray-700">
        <div class="flex items-center justify-between">
          <span class="text-sm text-gray-500 dark:text-gray-400">Match Rate</span>
          <span class={[
            "text-sm font-semibold",
            match_rate_color(@entity.match_rate)
          ]}>
            <%= format_match_rate(@entity.match_rate) %>
          </span>
        </div>
        <div class="mt-2 flex items-center justify-between">
          <span class="text-sm text-gray-500 dark:text-gray-400">Last Event</span>
          <span class="text-sm text-gray-700 dark:text-gray-300">
            <%= format_time_ago(@time_since) %>
          </span>
        </div>
      </div>
    </div>
    """
  end

  # Stat row component
  attr :label, :string, required: true
  attr :value, :integer, required: true
  attr :color, :string, required: true

  defp stat_row(assigns) do
    ~H"""
    <div class="flex items-center justify-between">
      <span class="text-sm text-gray-600 dark:text-gray-400"><%= @label %></span>
      <span class={[
        "text-sm font-medium px-2 py-0.5 rounded",
        stat_color(@color)
      ]}>
        <%= @value %>
      </span>
    </div>
    """
  end

  defp stat_color("blue"), do: "bg-blue-100 text-blue-700 dark:bg-blue-900/50 dark:text-blue-300"
  defp stat_color("green"), do: "bg-green-100 text-green-700 dark:bg-green-900/50 dark:text-green-300"
  defp stat_color("red"), do: "bg-red-100 text-red-700 dark:bg-red-900/50 dark:text-red-300"
  defp stat_color(_), do: "bg-gray-100 text-gray-700 dark:bg-gray-700 dark:text-gray-300"

  # Health badge component
  attr :status, :atom, required: true

  defp health_badge(assigns) do
    ~H"""
    <span class={[
      "px-2 py-1 text-xs font-medium rounded-full",
      health_badge_color(@status)
    ]}>
      <%= health_label(@status) %>
    </span>
    """
  end

  defp health_badge_color(:healthy), do: "bg-green-100 text-green-700 dark:bg-green-900/50 dark:text-green-300"
  defp health_badge_color(:stale), do: "bg-yellow-100 text-yellow-700 dark:bg-yellow-900/50 dark:text-yellow-300"
  defp health_badge_color(:offline), do: "bg-red-100 text-red-700 dark:bg-red-900/50 dark:text-red-300"
  defp health_badge_color(_), do: "bg-gray-100 text-gray-700 dark:bg-gray-700 dark:text-gray-300"

  defp health_label(:healthy), do: "Active"
  defp health_label(:stale), do: "Stale"
  defp health_label(:offline), do: "Offline"
  defp health_label(_), do: "Unknown"

  defp health_border(:healthy), do: "border-green-200 dark:border-green-800"
  defp health_border(:stale), do: "border-yellow-200 dark:border-yellow-800"
  defp health_border(:offline), do: "border-red-200 dark:border-red-800"
  defp health_border(_), do: "border-gray-200 dark:border-gray-700"

  # Summary stats component
  attr :entities, :list, required: true

  defp summary_stats(assigns) do
    totals = calculate_totals(assigns.entities)
    assigns = assign(assigns, :totals, totals)

    ~H"""
    <div class="bg-white dark:bg-gray-800 rounded-lg border border-gray-200 dark:border-gray-700 p-4">
      <h3 class="text-sm font-semibold text-gray-700 dark:text-gray-300 mb-3">
        Summary Statistics
      </h3>
      <div class="grid grid-cols-2 sm:grid-cols-4 gap-4">
        <div class="text-center">
          <p class="text-2xl font-bold text-blue-600 dark:text-blue-400">
            <%= @totals.received %>
          </p>
          <p class="text-xs text-gray-500 dark:text-gray-400">Total Received</p>
        </div>
        <div class="text-center">
          <p class="text-2xl font-bold text-green-600 dark:text-green-400">
            <%= @totals.matched %>
          </p>
          <p class="text-xs text-gray-500 dark:text-gray-400">Total Matched</p>
        </div>
        <div class="text-center">
          <p class="text-2xl font-bold text-red-600 dark:text-red-400">
            <%= @totals.unmatched %>
          </p>
          <p class="text-xs text-gray-500 dark:text-gray-400">Total Unmatched</p>
        </div>
        <div class="text-center">
          <p class="text-2xl font-bold text-purple-600 dark:text-purple-400">
            <%= format_match_rate(@totals.match_rate) %>
          </p>
          <p class="text-xs text-gray-500 dark:text-gray-400">Overall Match Rate</p>
        </div>
      </div>
    </div>
    """
  end

  # Helper functions

  defp calculate_totals(entities) do
    received = Enum.sum(Enum.map(entities, & &1.received_count))
    matched = Enum.sum(Enum.map(entities, & &1.matched_count))
    unmatched = Enum.sum(Enum.map(entities, & &1.unmatched_count))

    match_rate =
      if received > 0 do
        Float.round(matched / received * 100, 1)
      else
        nil
      end

    %{
      received: received,
      matched: matched,
      unmatched: unmatched,
      match_rate: match_rate
    }
  end

  defp time_since_last_event(nil), do: nil

  defp time_since_last_event(last_event_at) do
    DateTime.diff(DateTime.utc_now(), last_event_at, :second)
  end

  defp health_status(nil), do: :unknown
  defp health_status(seconds) when seconds < 60, do: :healthy
  defp health_status(seconds) when seconds < 300, do: :stale
  defp health_status(_), do: :offline

  defp format_time_ago(nil), do: "Never"
  defp format_time_ago(seconds) when seconds < 60, do: "#{seconds}s ago"
  defp format_time_ago(seconds) when seconds < 3600, do: "#{div(seconds, 60)}m ago"
  defp format_time_ago(seconds), do: "#{div(seconds, 3600)}h ago"

  defp format_match_rate(nil), do: "N/A"
  defp format_match_rate(rate), do: "#{rate}%"

  defp match_rate_color(nil), do: "text-gray-500 dark:text-gray-400"
  defp match_rate_color(rate) when rate >= 90, do: "text-green-600 dark:text-green-400"
  defp match_rate_color(rate) when rate >= 70, do: "text-yellow-600 dark:text-yellow-400"
  defp match_rate_color(_), do: "text-red-600 dark:text-red-400"

  defp format_time(datetime) do
    Calendar.strftime(datetime, "%H:%M:%S")
  end
end
