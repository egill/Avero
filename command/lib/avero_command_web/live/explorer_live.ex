defmodule AveroCommandWeb.ExplorerLive do
  use AveroCommandWeb, :live_view

  alias AveroCommand.Store

  @impl true
  def mount(_params, _session, socket) do
    {:ok,
     socket
     |> assign(:events, [])
     |> assign(:query, "")
     |> assign(:limit, 100)
     |> assign(:page_title, "Explorer")}
  end

  @impl true
  def handle_event("search", %{"query" => query, "limit" => limit}, socket) do
    limit = String.to_integer(limit)
    events = Store.search_events(query, limit)

    {:noreply,
     socket
     |> assign(:events, events)
     |> assign(:query, query)
     |> assign(:limit, limit)}
  end

  @impl true
  def handle_event("load_recent", _params, socket) do
    events = Store.recent_events(socket.assigns.limit)

    {:noreply, assign(socket, :events, events)}
  end

  @impl true
  def render(assigns) do
    ~H"""
    <div class="explorer">
      <.header>
        Historical Explorer
        <:subtitle>Search and analyze past events</:subtitle>
      </.header>

      <div class="mt-6 bg-white shadow rounded-lg p-6">
        <form phx-submit="search" class="flex space-x-4">
          <input
            type="text"
            name="query"
            value={@query}
            placeholder="Search events (e.g., event_type, site, person_id)"
            class="flex-1 rounded-md border-gray-300 shadow-sm focus:border-blue-500 focus:ring-blue-500"
          />
          <select name="limit" class="rounded-md border-gray-300 shadow-sm">
            <option value="50" selected={@limit == 50}>50</option>
            <option value="100" selected={@limit == 100}>100</option>
            <option value="500" selected={@limit == 500}>500</option>
          </select>
          <button
            type="submit"
            class="px-4 py-2 bg-blue-600 text-white rounded-md hover:bg-blue-700"
          >
            Search
          </button>
          <button
            type="button"
            phx-click="load_recent"
            class="px-4 py-2 bg-gray-200 text-gray-700 rounded-md hover:bg-gray-300"
          >
            Load Recent
          </button>
        </form>
      </div>

      <div class="mt-6 bg-white shadow rounded-lg overflow-hidden">
        <table class="min-w-full divide-y divide-gray-200">
          <thead class="bg-gray-50">
            <tr>
              <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                Time
              </th>
              <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                Type
              </th>
              <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                Site
              </th>
              <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                Person
              </th>
              <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                Gate
              </th>
              <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                Data
              </th>
            </tr>
          </thead>
          <tbody class="bg-white divide-y divide-gray-200">
            <%= if Enum.empty?(@events) do %>
              <tr>
                <td colspan="6" class="px-6 py-12 text-center text-gray-500">
                  No events found. Try searching or loading recent events.
                </td>
              </tr>
            <% else %>
              <%= for event <- @events do %>
                <tr class="hover:bg-gray-50">
                  <td class="px-6 py-4 whitespace-nowrap text-sm text-gray-500">
                    <%= format_time(event.time) %>
                  </td>
                  <td class="px-6 py-4 whitespace-nowrap text-sm font-medium text-gray-900">
                    <%= event.event_type %>
                  </td>
                  <td class="px-6 py-4 whitespace-nowrap text-sm text-gray-500">
                    <%= event.site %>
                  </td>
                  <td class="px-6 py-4 whitespace-nowrap text-sm text-gray-500">
                    <%= event.person_id || "-" %>
                  </td>
                  <td class="px-6 py-4 whitespace-nowrap text-sm text-gray-500">
                    <%= event.gate_id || "-" %>
                  </td>
                  <td class="px-6 py-4 text-sm text-gray-500 max-w-xs truncate">
                    <%= truncate_json(event.data) %>
                  </td>
                </tr>
              <% end %>
            <% end %>
          </tbody>
        </table>
      </div>
    </div>
    """
  end

  defp format_time(nil), do: "-"

  defp format_time(datetime) do
    Calendar.strftime(datetime, "%H:%M:%S")
  end

  defp truncate_json(nil), do: "-"

  defp truncate_json(data) when is_map(data) do
    json = Jason.encode!(data)

    if String.length(json) > 50 do
      String.slice(json, 0..47) <> "..."
    else
      json
    end
  end

  defp truncate_json(data), do: inspect(data)
end
