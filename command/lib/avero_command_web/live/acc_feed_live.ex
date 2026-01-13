defmodule AveroCommandWeb.AccFeedLive do
  @moduledoc """
  Real-time feed of ACC (payment terminal) events.
  Shows incoming ACC requests with their match status.
  """
  use AveroCommandWeb, :live_view

  @max_events 100

  @impl true
  def mount(_params, _session, socket) do
    if connected?(socket) do
      # Subscribe to real-time ACC events
      Phoenix.PubSub.subscribe(AveroCommand.PubSub, "acc_events")
    end

    {:ok,
     socket
     |> assign(:events, [])
     |> assign(:filter, :all)
     |> assign(:page_title, "ACC Monitor")
     |> assign(:paused, false)}
  end

  @impl true
  def handle_info({:acc_event, event}, socket) do
    if socket.assigns.paused do
      {:noreply, socket}
    else
      # Only add if event matches filter and site selection
      if matches_filter?(event, socket.assigns.filter, socket.assigns.selected_sites) do
        events = [event | socket.assigns.events] |> Enum.take(@max_events)
        {:noreply, assign(socket, :events, events)}
      else
        {:noreply, socket}
      end
    end
  end

  def handle_info(_msg, socket), do: {:noreply, socket}

  @impl true
  def handle_event("filter", %{"filter" => filter}, socket) do
    filter = String.to_existing_atom(filter)
    {:noreply, assign(socket, :filter, filter)}
  end

  @impl true
  def handle_event("toggle-pause", _params, socket) do
    {:noreply, assign(socket, :paused, !socket.assigns.paused)}
  end

  @impl true
  def handle_event("clear", _params, socket) do
    {:noreply, assign(socket, :events, [])}
  end

  defp matches_filter?(event, filter, selected_sites) do
    site_match = event.site in selected_sites or Enum.empty?(selected_sites)

    type_match = case filter do
      :all -> true
      :matched -> event.type in ["matched", "matched_no_journey"]
      :unmatched -> event.type == "unmatched"
      :late -> event.type == "late_after_gate"
    end

    site_match and type_match
  end

  @impl true
  def render(assigns) do
    ~H"""
    <div class="space-y-4">
      <!-- Header -->
      <div class="flex items-center justify-between">
        <h1 class="text-2xl font-bold text-gray-900 dark:text-white">ACC Monitor</h1>
        <div class="flex items-center gap-3">
          <!-- Event count -->
          <span class="text-sm text-gray-500 dark:text-gray-400">
            <%= length(@events) %> events
          </span>
          <!-- Pause button -->
          <button
            phx-click="toggle-pause"
            class={[
              "px-3 py-1.5 text-sm font-medium rounded-lg transition-colors",
              @paused && "bg-amber-100 text-amber-800 dark:bg-amber-900/30 dark:text-amber-300",
              !@paused && "bg-gray-100 text-gray-700 dark:bg-gray-700 dark:text-gray-300 hover:bg-gray-200 dark:hover:bg-gray-600"
            ]}
          >
            <%= if @paused, do: "▶ Resume", else: "⏸ Pause" %>
          </button>
          <!-- Clear button -->
          <button
            phx-click="clear"
            class="px-3 py-1.5 text-sm font-medium text-gray-700 dark:text-gray-300 bg-gray-100 dark:bg-gray-700 rounded-lg hover:bg-gray-200 dark:hover:bg-gray-600 transition-colors"
          >
            Clear
          </button>
        </div>
      </div>

      <!-- Filters -->
      <div class="flex items-center gap-2">
        <span class="text-sm text-gray-500 dark:text-gray-400">Filter:</span>
        <.filter_button filter={:all} current={@filter} label="All" />
        <.filter_button filter={:matched} current={@filter} label="Matched" />
        <.filter_button filter={:unmatched} current={@filter} label="Unmatched" />
        <.filter_button filter={:late} current={@filter} label="Late" />
      </div>

      <!-- Events list -->
      <div class="bg-white dark:bg-gray-800 rounded-lg border border-gray-200 dark:border-gray-700 overflow-hidden">
        <div class="divide-y divide-gray-100 dark:divide-gray-700">
          <%= if @events == [] do %>
            <div class="px-4 py-12 text-center text-gray-500 dark:text-gray-400">
              <div class="text-4xl mb-2">💳</div>
              <div>Waiting for ACC events...</div>
              <div class="text-sm mt-1">Payment terminal events will appear here in real-time</div>
            </div>
          <% else %>
            <%= for event <- @events do %>
              <.acc_event_row event={event} />
            <% end %>
          <% end %>
        </div>
      </div>
    </div>
    """
  end

  attr :filter, :atom, required: true
  attr :current, :atom, required: true
  attr :label, :string, required: true

  defp filter_button(assigns) do
    ~H"""
    <button
      phx-click="filter"
      phx-value-filter={@filter}
      class={[
        "px-3 py-1.5 text-sm font-medium rounded-lg transition-colors",
        @filter == @current && "bg-blue-100 text-blue-800 dark:bg-blue-900/30 dark:text-blue-300",
        @filter != @current && "bg-gray-100 text-gray-600 dark:bg-gray-700 dark:text-gray-400 hover:bg-gray-200 dark:hover:bg-gray-600"
      ]}
    >
      <%= @label %>
    </button>
    """
  end

  attr :event, :map, required: true

  defp acc_event_row(assigns) do
    ~H"""
    <div class="px-4 py-3 flex items-start gap-4">
      <!-- Status indicator -->
      <div class="flex-shrink-0 mt-1">
        <.status_badge type={@event.type} />
      </div>

      <!-- Main content -->
      <div class="flex-1 min-w-0">
        <div class="flex items-center gap-2">
          <span class="font-medium text-gray-900 dark:text-white">
            <%= format_type(@event.type) %>
          </span>
          <%= if @event.pos do %>
            <span class="text-sm text-gray-500 dark:text-gray-400">
              at <%= @event.pos %>
            </span>
          <% end %>
        </div>

        <div class="mt-1 flex flex-wrap items-center gap-x-4 gap-y-1 text-sm text-gray-500 dark:text-gray-400">
          <%= if @event.ip do %>
            <span>IP: <%= @event.ip %></span>
          <% end %>
          <%= if @event.tid do %>
            <span>Track: <%= @event.tid %></span>
          <% end %>
          <%= if @event.dwell_ms do %>
            <span>Dwell: <%= format_duration(@event.dwell_ms) %></span>
          <% end %>
          <%= if @event.delta_ms do %>
            <span class="text-amber-600 dark:text-amber-400">Late by: <%= format_duration(@event.delta_ms) %></span>
          <% end %>
        </div>

        <!-- Debug info for unmatched -->
        <%= if @event.type == "unmatched" and (@event.debug_active || @event.debug_pending) do %>
          <div class="mt-2 text-xs">
            <%= if @event.debug_active && length(@event.debug_active) > 0 do %>
              <div class="text-gray-500 dark:text-gray-400">
                Active tracks: <%= length(@event.debug_active) %>
                <%= for track <- Enum.take(@event.debug_active, 3) do %>
                  <span class="ml-2 px-1.5 py-0.5 bg-gray-100 dark:bg-gray-700 rounded">
                    #<%= track["tid"] || track[:tid] %> (<%= track["dwell_ms"] || track[:dwell_ms] || 0 %>ms)
                  </span>
                <% end %>
              </div>
            <% end %>
            <%= if @event.debug_pending && length(@event.debug_pending) > 0 do %>
              <div class="text-gray-500 dark:text-gray-400 mt-1">
                Pending tracks: <%= length(@event.debug_pending) %>
              </div>
            <% end %>
          </div>
        <% end %>
      </div>

      <!-- Timestamp -->
      <div class="flex-shrink-0 text-sm text-gray-400 dark:text-gray-500">
        <%= format_time(@event.time) %>
      </div>
    </div>
    """
  end

  attr :type, :string, required: true

  defp status_badge(assigns) do
    {bg_class, text_class, icon} = case assigns.type do
      "matched" -> {"bg-green-100 dark:bg-green-900/30", "text-green-700 dark:text-green-300", "✓"}
      "matched_no_journey" -> {"bg-blue-100 dark:bg-blue-900/30", "text-blue-700 dark:text-blue-300", "~"}
      "unmatched" -> {"bg-red-100 dark:bg-red-900/30", "text-red-700 dark:text-red-300", "✗"}
      "late_after_gate" -> {"bg-amber-100 dark:bg-amber-900/30", "text-amber-700 dark:text-amber-300", "⏰"}
      "received" -> {"bg-gray-100 dark:bg-gray-700", "text-gray-600 dark:text-gray-400", "→"}
      _ -> {"bg-gray-100 dark:bg-gray-700", "text-gray-600 dark:text-gray-400", "?"}
    end

    assigns = assign(assigns, bg_class: bg_class, text_class: text_class, icon: icon)

    ~H"""
    <div class={["w-8 h-8 rounded-full flex items-center justify-center text-sm font-bold", @bg_class, @text_class]}>
      <%= @icon %>
    </div>
    """
  end

  defp format_type("matched"), do: "Payment Matched"
  defp format_type("matched_no_journey"), do: "Matched (No Journey)"
  defp format_type("unmatched"), do: "Payment Unmatched"
  defp format_type("late_after_gate"), do: "Late Payment"
  defp format_type("received"), do: "Payment Received"
  defp format_type(other), do: other

  defp format_duration(nil), do: "-"
  defp format_duration(ms) when is_integer(ms) do
    cond do
      ms < 1000 -> "#{ms}ms"
      ms < 60_000 -> "#{Float.round(ms / 1000, 1)}s"
      true -> "#{div(ms, 60_000)}m #{rem(div(ms, 1000), 60)}s"
    end
  end
  defp format_duration(_), do: "-"

  defp format_time(nil), do: "-"
  defp format_time(%DateTime{} = dt), do: Calendar.strftime(dt, "%H:%M:%S")
  defp format_time(_), do: "-"
end
