defmodule AveroCommandWeb.IncidentExplorerLive do
  use AveroCommandWeb, :live_view

  alias AveroCommand.Incidents

  @impl true
  def mount(_params, _session, socket) do
    today = Date.utc_today()

    {:ok,
     socket
     |> assign(:view_mode, :daily)
     |> assign(:selected_date, today)
     |> assign(:expanded_hours, MapSet.new())
     |> assign(:page_title, "Explorer")
     |> load_data()}
  end

  defp load_data(socket) do
    case socket.assigns.view_mode do
      :daily -> load_daily_data(socket)
      :weekly -> load_weekly_data(socket)
    end
  end

  defp load_daily_data(socket) do
    date = socket.assigns.selected_date
    sites = socket.assigns.selected_sites

    hourly_data = Incidents.list_by_hour(date, sites)

    # Build a map for quick lookup
    hourly_map =
      hourly_data
      |> Enum.map(fn row -> {row.hour, row} end)
      |> Map.new()

    # Generate all hours from 7 to 23 (operating hours)
    hours =
      for hour <- 7..23 do
        case Map.get(hourly_map, hour) do
          nil -> %{hour: hour, high: 0, medium: 0, info: 0, total: 0}
          data -> data
        end
      end

    assign(socket, :hourly_data, hours)
  end

  defp load_weekly_data(socket) do
    # Get start of week (Monday)
    today = socket.assigns.selected_date
    day_of_week = Date.day_of_week(today)
    start_of_week = Date.add(today, -(day_of_week - 1))

    sites = socket.assigns.selected_sites
    daily_data = Incidents.list_by_day_for_week(start_of_week, sites)

    # Build a map for quick lookup
    daily_map =
      daily_data
      |> Enum.map(fn row -> {row.date, row} end)
      |> Map.new()

    # Generate all days of the week
    days =
      for offset <- 0..6 do
        date = Date.add(start_of_week, offset)

        case Map.get(daily_map, date) do
          nil -> %{date: date, high: 0, medium: 0, info: 0, total: 0}
          data -> data
        end
      end

    socket
    |> assign(:weekly_data, days)
    |> assign(:week_start, start_of_week)
  end

  @impl true
  def handle_event("toggle-site-menu", _params, socket) do
    {:noreply, assign(socket, :site_menu_open, !socket.assigns.site_menu_open)}
  end

  @impl true
  def handle_event("toggle-site", %{"site" => site}, socket) do
    selected = socket.assigns.selected_sites

    selected =
      if site in selected do
        List.delete(selected, site)
      else
        [site | selected]
      end

    selected = if Enum.empty?(selected), do: socket.assigns.selected_sites, else: selected

    {:noreply,
     socket
     |> assign(:selected_sites, selected)
     |> load_data()}
  end

  @impl true
  def handle_event("set-view", %{"mode" => mode}, socket) do
    mode = String.to_existing_atom(mode)

    {:noreply,
     socket
     |> assign(:view_mode, mode)
     |> load_data()}
  end

  @impl true
  def handle_event("prev-date", _params, socket) do
    new_date =
      case socket.assigns.view_mode do
        :daily -> Date.add(socket.assigns.selected_date, -1)
        :weekly -> Date.add(socket.assigns.selected_date, -7)
      end

    {:noreply,
     socket
     |> assign(:selected_date, new_date)
     |> load_data()}
  end

  @impl true
  def handle_event("next-date", _params, socket) do
    new_date =
      case socket.assigns.view_mode do
        :daily -> Date.add(socket.assigns.selected_date, 1)
        :weekly -> Date.add(socket.assigns.selected_date, 7)
      end

    {:noreply,
     socket
     |> assign(:selected_date, new_date)
     |> load_data()}
  end

  @impl true
  def handle_event("today", _params, socket) do
    {:noreply,
     socket
     |> assign(:selected_date, Date.utc_today())
     |> load_data()}
  end

  @impl true
  def handle_event("toggle-hour", %{"hour" => hour}, socket) do
    hour = String.to_integer(hour)
    expanded = socket.assigns.expanded_hours

    expanded =
      if MapSet.member?(expanded, hour) do
        MapSet.delete(expanded, hour)
      else
        MapSet.put(expanded, hour)
      end

    {:noreply, assign(socket, :expanded_hours, expanded)}
  end

  @impl true
  def handle_event("select-day", %{"date" => date_str}, socket) do
    {:ok, date} = Date.from_iso8601(date_str)

    {:noreply,
     socket
     |> assign(:view_mode, :daily)
     |> assign(:selected_date, date)
     |> assign(:expanded_hours, MapSet.new())
     |> load_data()}
  end

  @impl true
  def render(assigns) do
    ~H"""
    <div class="incident-explorer">
      <div class="mb-6 flex items-center justify-between">
        <div class="flex items-center space-x-4">
          <h2 class="text-lg font-semibold text-gray-900">Incident Explorer</h2>
          <.site_selector
            available_sites={@available_sites}
            selected_sites={@selected_sites}
            site_menu_open={@site_menu_open}
          />
        </div>
        <div class="flex items-center space-x-4">
          <div class="flex items-center space-x-2">
            <button
              phx-click="prev-date"
              class="p-1 hover:bg-gray-100 rounded"
            >
              <svg class="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M15 19l-7-7 7-7" />
              </svg>
            </button>
            <button
              phx-click="today"
              class="px-3 py-1 text-sm font-medium text-gray-700 hover:bg-gray-100 rounded"
            >
              <%= format_date_header(@view_mode, @selected_date, assigns[:week_start]) %>
            </button>
            <button
              phx-click="next-date"
              class="p-1 hover:bg-gray-100 rounded"
            >
              <svg class="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M9 5l7 7-7 7" />
              </svg>
            </button>
          </div>
          <div class="flex rounded-md shadow-sm">
            <button
              phx-click="set-view"
              phx-value-mode="daily"
              class={[
                "px-3 py-1 text-sm font-medium rounded-l-md border",
                @view_mode == :daily && "bg-blue-600 text-white border-blue-600",
                @view_mode != :daily && "bg-white text-gray-700 border-gray-300 hover:bg-gray-50"
              ]}
            >
              Daily
            </button>
            <button
              phx-click="set-view"
              phx-value-mode="weekly"
              class={[
                "px-3 py-1 text-sm font-medium rounded-r-md border-t border-r border-b",
                @view_mode == :weekly && "bg-blue-600 text-white border-blue-600",
                @view_mode != :weekly && "bg-white text-gray-700 border-gray-300 hover:bg-gray-50"
              ]}
            >
              Weekly
            </button>
          </div>
        </div>
      </div>

      <%= if @view_mode == :daily do %>
        <.daily_view hourly_data={@hourly_data} expanded_hours={@expanded_hours} selected_date={@selected_date} selected_sites={@selected_sites} />
      <% else %>
        <.weekly_view weekly_data={@weekly_data} selected_date={@selected_date} />
      <% end %>
    </div>
    """
  end

  defp daily_view(assigns) do
    ~H"""
    <div class="bg-white rounded-lg shadow overflow-hidden">
      <div class="divide-y divide-gray-200">
        <%= for hour_data <- @hourly_data do %>
          <.hour_row
            hour_data={hour_data}
            expanded={MapSet.member?(@expanded_hours, hour_data.hour)}
            selected_date={@selected_date}
            selected_sites={@selected_sites}
          />
        <% end %>
      </div>
    </div>
    """
  end

  defp hour_row(assigns) do
    ~H"""
    <div>
      <div
        class={[
          "flex items-center px-4 py-3 cursor-pointer hover:bg-gray-50",
          @hour_data.total > 0 && "bg-gray-50"
        ]}
        phx-click="toggle-hour"
        phx-value-hour={@hour_data.hour}
      >
        <div class="w-16 text-sm font-medium text-gray-500">
          <%= format_hour(@hour_data.hour) %>
        </div>
        <div class="flex-1">
          <%= if @hour_data.total > 0 do %>
            <div class="flex items-center space-x-2">
              <.severity_badges high={@hour_data.high} medium={@hour_data.medium} info={@hour_data.info} />
            </div>
          <% else %>
            <span class="text-sm text-gray-400">-</span>
          <% end %>
        </div>
        <%= if @hour_data.total > 0 do %>
          <div class="text-gray-400">
            <%= if @expanded do %>
              <svg class="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M19 9l-7 7-7-7" />
              </svg>
            <% else %>
              <svg class="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M9 5l7 7-7 7" />
              </svg>
            <% end %>
          </div>
        <% end %>
      </div>
      <%= if @expanded && @hour_data.total > 0 do %>
        <.hour_details hour={@hour_data.hour} selected_date={@selected_date} selected_sites={@selected_sites} />
      <% end %>
    </div>
    """
  end

  defp hour_details(assigns) do
    type_counts =
      Incidents.list_by_type_for_hour(assigns.selected_date, assigns.hour, assigns.selected_sites)

    incidents =
      Incidents.list_for_hour(assigns.selected_date, assigns.hour, assigns.selected_sites)

    # Group incidents by type for drill-down
    incidents_by_type = Enum.group_by(incidents, & &1.type)

    assigns =
      assigns
      |> assign(:type_counts, type_counts)
      |> assign(:incidents_by_type, incidents_by_type)

    ~H"""
    <div class="bg-gray-50 border-t border-gray-200">
      <div class="divide-y divide-gray-200">
        <%= for {type, count} <- Enum.sort_by(@type_counts, fn {_, c} -> -c end) do %>
          <div class="px-4 py-2 pl-20">
            <div class="flex items-center justify-between">
              <div class="flex items-center space-x-2">
                <span class="text-sm font-medium text-gray-700"><%= format_type(type) %></span>
                <span class="text-xs text-gray-500">(<%= count %>)</span>
              </div>
            </div>
            <div class="mt-1 space-y-1">
              <%= for incident <- Map.get(@incidents_by_type, type, []) do %>
                <.link
                  navigate={~p"/incidents/#{incident.id}"}
                  class="block text-xs text-gray-600 hover:text-blue-600 hover:bg-blue-50 px-2 py-1 rounded"
                >
                  <span class={["inline-block w-2 h-2 rounded-full mr-2", severity_dot_color(incident.severity)]}></span>
                  <%= format_time_short(incident.created_at) %> - <%= incident.context["message"] || incident.type %>
                </.link>
              <% end %>
            </div>
          </div>
        <% end %>
      </div>
    </div>
    """
  end

  defp weekly_view(assigns) do
    ~H"""
    <div class="bg-white rounded-lg shadow overflow-hidden">
      <div class="divide-y divide-gray-200">
        <%= for day_data <- @weekly_data do %>
          <div
            class={[
              "flex items-center px-4 py-4 cursor-pointer hover:bg-gray-50",
              Date.compare(day_data.date, Date.utc_today()) == :eq && "bg-blue-50"
            ]}
            phx-click="select-day"
            phx-value-date={Date.to_iso8601(day_data.date)}
          >
            <div class="w-24 text-sm font-medium text-gray-900">
              <%= format_day(day_data.date) %>
            </div>
            <div class="flex-1">
              <%= if day_data.total > 0 do %>
                <.severity_badges high={day_data.high} medium={day_data.medium} info={day_data.info} />
              <% else %>
                <span class="text-sm text-gray-400">-</span>
              <% end %>
            </div>
            <%= if Date.compare(day_data.date, Date.utc_today()) == :eq do %>
              <span class="text-xs text-blue-600 font-medium">Today</span>
            <% end %>
          </div>
        <% end %>
      </div>
    </div>
    """
  end

  defp severity_badges(assigns) do
    ~H"""
    <div class="flex items-center space-x-2">
      <%= if @high > 0 do %>
        <span class="inline-flex items-center px-2 py-0.5 rounded text-xs font-medium bg-red-100 text-red-800">
          <%= @high %> high
        </span>
      <% end %>
      <%= if @medium > 0 do %>
        <span class="inline-flex items-center px-2 py-0.5 rounded text-xs font-medium bg-yellow-100 text-yellow-800">
          <%= @medium %> medium
        </span>
      <% end %>
      <%= if @info > 0 do %>
        <span class="inline-flex items-center px-2 py-0.5 rounded text-xs font-medium bg-blue-100 text-blue-800">
          <%= @info %> info
        </span>
      <% end %>
    </div>
    """
  end

  defp severity_dot_color("high"), do: "bg-red-500"
  defp severity_dot_color("medium"), do: "bg-yellow-500"
  defp severity_dot_color(_), do: "bg-blue-400"

  defp format_time_short(nil), do: ""
  defp format_time_short(datetime), do: Calendar.strftime(datetime, "%H:%M")

  defp site_selector(assigns) do
    ~H"""
    <div class="relative">
      <button
        phx-click="toggle-site-menu"
        class="flex items-center space-x-1 px-3 py-1 bg-gray-100 hover:bg-gray-200 rounded-md text-sm font-medium text-gray-700"
      >
        <span>Sites: <%= format_site_label(@selected_sites) %></span>
        <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
          <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M19 9l-7 7-7-7" />
        </svg>
      </button>
      <div
        :if={@site_menu_open}
        class="absolute z-10 mt-1 w-64 bg-white border border-gray-200 rounded-md shadow-lg"
      >
        <div class="p-2 space-y-1">
          <%= for site <- @available_sites do %>
            <label class="flex items-center space-x-2 px-2 py-1 hover:bg-gray-50 rounded cursor-pointer">
              <input
                type="checkbox"
                phx-click="toggle-site"
                phx-value-site={site}
                checked={site in @selected_sites}
                class="rounded border-gray-300 text-blue-600 focus:ring-blue-500"
              />
              <span class="text-sm text-gray-700"><%= format_site_name(site) %></span>
            </label>
          <% end %>
        </div>
      </div>
    </div>
    """
  end

  defp format_date_header(:daily, date, _week_start) do
    if Date.compare(date, Date.utc_today()) == :eq do
      "#{Calendar.strftime(date, "%b %d, %Y")} (Today)"
    else
      Calendar.strftime(date, "%b %d, %Y")
    end
  end

  defp format_date_header(:weekly, _date, week_start) do
    "Week of #{Calendar.strftime(week_start, "%b %d, %Y")}"
  end

  defp format_hour(hour) do
    "#{String.pad_leading(to_string(hour), 2, "0")}:00"
  end

  defp format_day(date) do
    day_name = Calendar.strftime(date, "%a")
    day_num = date.day
    "#{day_name} #{day_num}"
  end

  defp format_type(type) when is_binary(type) do
    type
    |> String.replace("_", " ")
    |> String.split(" ")
    |> Enum.map(&String.capitalize/1)
    |> Enum.join(" ")
  end

  defp format_type(_), do: "Unknown"

  defp format_site_label(sites) when length(sites) == 1, do: format_site_name(hd(sites))
  defp format_site_label(sites), do: "#{length(sites)} selected"

  defp format_site_name("AP-NETTO-GR-01"), do: "Netto"
  defp format_site_name("AP-AVERO-GR-01"), do: "Avero"
  defp format_site_name("docker-gateway"), do: "Docker"
  defp format_site_name(site), do: site
end
