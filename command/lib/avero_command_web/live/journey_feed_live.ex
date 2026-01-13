defmodule AveroCommandWeb.JourneyFeedLive do
  use AveroCommandWeb, :live_view

  alias AveroCommand.Journeys

  @page_size 25
  @min_dwell_ms 7000  # 7 seconds minimum dwell to count as POS stop

  @impl true
  def mount(_params, _session, socket) do
    if connected?(socket) do
      # Subscribe to real-time journey updates
      Phoenix.PubSub.subscribe(AveroCommand.PubSub, "journeys")
    end

    # Load available POS zones
    available_pos_zones = Journeys.list_payment_zones(sites: socket.assigns.selected_sites)

    socket =
      socket
      |> assign(:page_title, "Customer Journeys")
      |> assign(:expanded_journey_ids, MapSet.new())
      # Existing filter
      |> assign(:filter, :all)
      # New filters
      |> assign(:person_id_search, "")
      |> assign(:selected_date, Date.utc_today())
      |> assign(:from_date, nil)
      |> assign(:to_date, nil)
      |> assign(:from_datetime, nil)
      |> assign(:to_datetime, nil)
      |> assign(:pos_filter, :all)
      |> assign(:selected_pos_zones, [])
      |> assign(:available_pos_zones, available_pos_zones)
      |> assign(:pos_menu_open, false)
      |> assign(:advanced_filters_open, false)
      # ACC filter (payment terminal match)
      |> assign(:acc_filter, :all)
      # Duration filter - hide journeys < 7s by default
      |> assign(:hide_short_journeys, true)
      # Pagination
      |> assign(:cursor, nil)
      |> assign(:direction, :next)
      |> assign(:has_next, false)
      |> assign(:has_prev, false)
      |> assign(:page_size, @page_size)

    socket = load_journeys(socket)

    {:ok, socket}
  end

  @impl true
  def handle_info({:journey_created, journey}, socket) do
    # Only add if we're on the first page (no cursor) and journey matches all filters
    if socket.assigns.cursor == nil and should_include_journey?(journey, socket.assigns) do
      # Add to front of list, keep only page_size
      journeys = Enum.take([journey | socket.assigns.journeys], socket.assigns.page_size)
      {:noreply, assign(socket, :journeys, journeys)}
    else
      {:noreply, socket}
    end
  end

  defp should_include_journey?(journey, assigns) do
    matches_site?(journey, assigns.selected_sites) and
      matches_exit_type?(journey, assigns.filter) and
      matches_person_id?(journey, assigns.person_id_search) and
      matches_datetime_range?(journey, assigns.from_datetime, assigns.to_datetime) and
      matches_pos_filter?(journey, assigns.pos_filter, assigns.selected_pos_zones) and
      matches_acc_filter?(journey, assigns.acc_filter) and
      matches_min_duration?(journey, assigns.hide_short_journeys)
  end

  defp matches_site?(journey, sites), do: journey.site in sites

  defp matches_exit_type?(_journey, :all), do: true
  defp matches_exit_type?(journey, :exits), do: journey.exit_type == "exit_confirmed"
  defp matches_exit_type?(journey, :returns), do: journey.exit_type == "returned_to_store"
  defp matches_exit_type?(journey, :lost), do: journey.exit_type == "tracking_lost"
  defp matches_exit_type?(_journey, _), do: true

  defp matches_person_id?(_journey, nil), do: true
  defp matches_person_id?(_journey, ""), do: true
  defp matches_person_id?(journey, person_id) when is_binary(person_id) do
    case Integer.parse(String.trim(person_id)) do
      {id, ""} -> journey.person_id == id
      _ -> true
    end
  end

  defp matches_datetime_range?(_journey, nil, nil), do: true
  defp matches_datetime_range?(journey, from_datetime, to_datetime) do
    from_ok = is_nil(from_datetime) or DateTime.compare(journey.time, from_datetime) != :lt
    to_ok = is_nil(to_datetime) or DateTime.compare(journey.time, to_datetime) != :gt
    from_ok and to_ok
  end

  defp matches_pos_filter?(_journey, :all, []), do: true
  defp matches_pos_filter?(journey, :all, zones) when length(zones) > 0 do
    journey.payment_zone in zones
  end
  # "With POS" = had meaningful time at a POS zone (>= 7s dwell)
  defp matches_pos_filter?(journey, :with_pos, []) do
    journey.total_pos_dwell_ms != nil and journey.total_pos_dwell_ms >= @min_dwell_ms
  end
  defp matches_pos_filter?(journey, :with_pos, zones) when length(zones) > 0 do
    journey.payment_zone in zones
  end
  # "No POS" = didn't spend meaningful time at any POS zone
  defp matches_pos_filter?(journey, :without_pos, _) do
    is_nil(journey.total_pos_dwell_ms) or journey.total_pos_dwell_ms < @min_dwell_ms
  end
  # "Unpaid with POS" = unpaid but had meaningful POS stop
  defp matches_pos_filter?(journey, :unpaid_with_pos, _) do
    journey.authorized != true and
      journey.total_pos_dwell_ms != nil and
      journey.total_pos_dwell_ms >= @min_dwell_ms
  end
  defp matches_pos_filter?(_journey, _, _), do: true

  defp matches_min_duration?(_journey, false), do: true
  defp matches_min_duration?(journey, true) do
    journey.duration_ms != nil and journey.duration_ms >= @min_dwell_ms
  end

  # ACC filter: filter journeys by payment terminal match status
  defp matches_acc_filter?(_journey, :all), do: true
  defp matches_acc_filter?(journey, :acc_matched), do: journey.acc_matched == true
  defp matches_acc_filter?(journey, :acc_not_matched), do: journey.acc_matched != true

  @impl true
  def handle_event("filter", %{"filter" => filter}, socket) do
    filter_atom = String.to_existing_atom(filter)

    socket
    |> assign(:filter, filter_atom)
    |> reset_pagination_and_reload()
  end

  @impl true
  def handle_event("toggle_expand", %{"id" => id}, socket) do
    id_int = String.to_integer(id)
    expanded = socket.assigns.expanded_journey_ids

    new_expanded =
      if MapSet.member?(expanded, id_int) do
        MapSet.delete(expanded, id_int)
      else
        MapSet.put(expanded, id_int)
      end

    {:noreply, assign(socket, :expanded_journey_ids, new_expanded)}
  end

  @impl true
  def handle_event("collapse_all", _params, socket) do
    {:noreply, assign(socket, :expanded_journey_ids, MapSet.new())}
  end

  @impl true
  def handle_event("expand_all", _params, socket) do
    ids = socket.assigns.journeys |> Enum.map(& &1.id) |> MapSet.new()
    {:noreply, assign(socket, :expanded_journey_ids, ids)}
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

    # Don't allow empty selection
    selected = if Enum.empty?(selected), do: socket.assigns.selected_sites, else: selected

    # Reload POS zones for new site selection
    available_pos_zones = Journeys.list_payment_zones(sites: selected)

    socket =
      socket
      |> assign(:selected_sites, selected)
      |> assign(:available_pos_zones, available_pos_zones)
      |> assign(:site_menu_open, false)
      |> assign(:cursor, nil)
      |> assign(:direction, :next)
      |> load_journeys()

    {:noreply, socket}
  end

  # Person ID search
  @impl true
  def handle_event("search_person", %{"person_id" => person_id}, socket) do
    socket
    |> assign(:person_id_search, person_id)
    |> reset_pagination_and_reload()
  end

  # Date navigation
  @impl true
  def handle_event("prev-date", _params, socket) do
    new_date = Date.add(socket.assigns.selected_date, -1)

    socket
    |> assign(:selected_date, new_date)
    |> assign(:from_date, new_date)
    |> assign(:to_date, new_date)
    |> reset_pagination_and_reload()
  end

  @impl true
  def handle_event("next-date", _params, socket) do
    new_date = Date.add(socket.assigns.selected_date, 1)

    socket
    |> assign(:selected_date, new_date)
    |> assign(:from_date, new_date)
    |> assign(:to_date, new_date)
    |> reset_pagination_and_reload()
  end

  @impl true
  def handle_event("today", _params, socket) do
    socket
    |> assign(:selected_date, Date.utc_today())
    |> assign(:from_date, nil)
    |> assign(:to_date, nil)
    |> reset_pagination_and_reload()
  end

  # Date range picker - consolidated handlers
  @impl true
  def handle_event("set-from-date", %{"value" => value}, socket) do
    case parse_date(value) do
      {:ok, date} ->
        socket |> assign(:from_date, date) |> reset_pagination_and_reload()
      :error ->
        {:noreply, socket}
    end
  end

  @impl true
  def handle_event("set-to-date", %{"value" => value}, socket) do
    case parse_date(value) do
      {:ok, date} ->
        socket |> assign(:to_date, date) |> reset_pagination_and_reload()
      :error ->
        {:noreply, socket}
    end
  end

  @impl true
  def handle_event("clear-date-range", _params, socket) do
    socket
    |> assign(:from_date, nil)
    |> assign(:to_date, nil)
    |> assign(:selected_date, Date.utc_today())
    |> reset_pagination_and_reload()
  end

  # DateTime range picker - consolidated handlers
  @impl true
  def handle_event("set-from-datetime", %{"value" => value}, socket) do
    case parse_datetime_local(value) do
      {:ok, datetime} ->
        socket |> assign(:from_datetime, datetime) |> reset_pagination_and_reload()
      _ ->
        {:noreply, socket}
    end
  end

  @impl true
  def handle_event("set-to-datetime", %{"value" => value}, socket) do
    case parse_datetime_local(value) do
      {:ok, datetime} ->
        socket |> assign(:to_datetime, datetime) |> reset_pagination_and_reload()
      _ ->
        {:noreply, socket}
    end
  end

  @impl true
  def handle_event("clear-datetime-range", _params, socket) do
    socket
    |> assign(:from_datetime, nil)
    |> assign(:to_datetime, nil)
    |> reset_pagination_and_reload()
  end

  # POS quick filter
  @impl true
  def handle_event("pos-filter", %{"filter" => filter}, socket) do
    pos_filter = String.to_existing_atom(filter)

    socket
    |> assign(:pos_filter, pos_filter)
    |> assign(:selected_pos_zones, [])
    |> reset_pagination_and_reload()
  end

  # POS zone multi-select
  @impl true
  def handle_event("toggle-pos-menu", _params, socket) do
    {:noreply, assign(socket, :pos_menu_open, !socket.assigns.pos_menu_open)}
  end

  @impl true
  def handle_event("toggle-pos-zone", %{"zone" => zone}, socket) do
    selected = socket.assigns.selected_pos_zones
    selected = if zone in selected, do: List.delete(selected, zone), else: [zone | selected]

    socket
    |> assign(:selected_pos_zones, selected)
    |> reset_pagination_and_reload()
  end

  @impl true
  def handle_event("clear-pos-zones", _params, socket) do
    socket
    |> assign(:selected_pos_zones, [])
    |> reset_pagination_and_reload()
  end

  # Advanced filters toggle
  @impl true
  def handle_event("toggle-advanced-filters", _params, socket) do
    {:noreply, assign(socket, :advanced_filters_open, !socket.assigns.advanced_filters_open)}
  end

  # Duration filter toggle
  @impl true
  def handle_event("toggle-hide-short", _params, socket) do
    socket
    |> assign(:hide_short_journeys, !socket.assigns.hide_short_journeys)
    |> reset_pagination_and_reload()
  end

  # ACC filter
  @impl true
  def handle_event("acc-filter", %{"filter" => filter}, socket) do
    acc_filter = String.to_existing_atom(filter)

    socket
    |> assign(:acc_filter, acc_filter)
    |> reset_pagination_and_reload()
  end

  # Pagination
  @impl true
  def handle_event("next-page", _params, socket) do
    # Use the last journey's time as cursor
    case List.last(socket.assigns.journeys) do
      nil ->
        {:noreply, socket}

      last_journey ->
        socket =
          socket
          |> assign(:cursor, last_journey.time)
          |> assign(:direction, :next)
          |> load_journeys()

        {:noreply, socket}
    end
  end

  @impl true
  def handle_event("prev-page", _params, socket) do
    # Use the first journey's time as cursor
    case List.first(socket.assigns.journeys) do
      nil ->
        {:noreply, socket}

      first_journey ->
        socket =
          socket
          |> assign(:cursor, first_journey.time)
          |> assign(:direction, :prev)
          |> load_journeys()

        {:noreply, socket}
    end
  end

  @impl true
  def handle_event("first-page", _params, socket) do
    reset_pagination_and_reload(socket)
  end

  # Helper to reset pagination and reload journeys
  defp reset_pagination_and_reload(socket) do
    socket
    |> assign(:cursor, nil)
    |> assign(:direction, :next)
    |> load_journeys()
    |> then(&{:noreply, &1})
  end

  # Unified load function using list_filtered
  defp load_journeys(socket) do
    assigns = socket.assigns

    opts = [
      sites: assigns.selected_sites,
      exit_type: assigns.filter,
      person_id: assigns.person_id_search,
      from_date: assigns.from_date,
      to_date: assigns.to_date,
      from_datetime: assigns.from_datetime,
      to_datetime: assigns.to_datetime,
      pos_filter: assigns.pos_filter,
      pos_zones: assigns.selected_pos_zones,
      acc_filter: assigns.acc_filter,
      min_duration_ms: if(assigns.hide_short_journeys, do: @min_dwell_ms, else: nil),
      cursor: assigns.cursor,
      direction: assigns.direction,
      limit: assigns.page_size
    ]

    results = Journeys.list_filtered(opts)

    # Detect has_more by checking if we got limit+1 results
    {journeys, has_more} =
      if length(results) > assigns.page_size do
        {Enum.take(results, assigns.page_size), true}
      else
        {results, false}
      end

    # Determine pagination state
    {has_next, has_prev} =
      case assigns.direction do
        :next -> {has_more, assigns.cursor != nil}
        :prev -> {assigns.cursor != nil, has_more}
      end

    socket
    |> assign(:journeys, journeys)
    |> assign(:has_next, has_next)
    |> assign(:has_prev, has_prev)
  end

  @impl true
  def render(assigns) do
    ~H"""
    <div class="journey-feed">
      <%!-- Header with title, site selector, and exit type filters --%>
      <div class="mb-4 sm:mb-6 flex flex-col sm:flex-row sm:items-center sm:justify-between gap-3">
        <div class="flex items-center justify-between sm:justify-start space-x-4">
          <h2 class="text-base sm:text-lg font-semibold text-gray-900">Customer Journeys</h2>
          <.site_selector
            available_sites={@available_sites}
            selected_sites={@selected_sites}
            site_menu_open={@site_menu_open}
          />
        </div>
        <div class="flex flex-wrap gap-2">
          <button
            phx-click="filter"
            phx-value-filter="all"
            class={filter_button_class(@filter == :all, "blue")}
          >
            All
          </button>
          <button
            phx-click="filter"
            phx-value-filter="exits"
            class={filter_button_class(@filter == :exits, "green")}
          >
            Exits
          </button>
          <button
            phx-click="filter"
            phx-value-filter="returns"
            class={filter_button_class(@filter == :returns, "yellow")}
          >
            Returns
          </button>
          <button
            phx-click="filter"
            phx-value-filter="lost"
            class={filter_button_class(@filter == :lost, "red")}
          >
            Lost
          </button>
        </div>
      </div>

      <%!-- Quick Filters Bar --%>
      <div class="bg-white shadow rounded-lg p-3 mb-4">
        <div class="flex flex-wrap items-center gap-3">
          <%!-- Person ID Search --%>
          <div class="flex-shrink-0">
            <form phx-change="search_person" phx-submit="search_person">
              <input
                type="text"
                name="person_id"
                value={@person_id_search}
                placeholder="Person ID"
                class="w-28 px-3 py-1.5 text-sm border border-gray-300 rounded-md focus:ring-blue-500 focus:border-blue-500"
                phx-debounce="300"
              />
            </form>
          </div>

          <%!-- Date Navigation --%>
          <div class="flex items-center space-x-1 border-l pl-3">
            <button phx-click="prev-date" class="p-1.5 hover:bg-gray-100 rounded" title="Previous day">
              <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M15 19l-7-7 7-7" />
              </svg>
            </button>
            <button
              phx-click="today"
              class="px-2 py-1 text-sm font-medium text-gray-700 hover:bg-gray-100 rounded"
              title="Go to today"
            >
              <%= format_selected_date(@selected_date) %>
            </button>
            <button phx-click="next-date" class="p-1.5 hover:bg-gray-100 rounded" title="Next day">
              <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M9 5l7 7-7 7" />
              </svg>
            </button>
          </div>

          <%!-- POS Quick Filter --%>
          <div class="flex items-center space-x-1 border-l pl-3">
            <button
              phx-click="pos-filter"
              phx-value-filter="all"
              class={pos_filter_button_class(@pos_filter == :all)}
            >
              All
            </button>
            <button
              phx-click="pos-filter"
              phx-value-filter="with_pos"
              class={pos_filter_button_class(@pos_filter == :with_pos)}
            >
              With POS
            </button>
            <button
              phx-click="pos-filter"
              phx-value-filter="without_pos"
              class={pos_filter_button_class(@pos_filter == :without_pos)}
            >
              No POS
            </button>
            <button
              phx-click="pos-filter"
              phx-value-filter="unpaid_with_pos"
              class={pos_filter_button_class(@pos_filter == :unpaid_with_pos, "red")}
            >
              Unpaid w/ POS
            </button>
          </div>

          <%!-- Duration Filter --%>
          <div class="flex items-center border-l pl-3">
            <button
              phx-click="toggle-hide-short"
              class={pos_filter_button_class(@hide_short_journeys)}
            >
              Hide &lt;7s
            </button>
          </div>

          <%!-- ACC Filter --%>
          <div class="flex items-center space-x-1 border-l pl-3">
            <button
              phx-click="acc-filter"
              phx-value-filter="all"
              class={acc_filter_button_class(@acc_filter == :all)}
              title="Show all journeys"
            >
              All
            </button>
            <button
              phx-click="acc-filter"
              phx-value-filter="acc_matched"
              class={acc_filter_button_class(@acc_filter == :acc_matched, "green")}
              title="Only show journeys with ACC payment match"
            >
              ACC ✓
            </button>
            <button
              phx-click="acc-filter"
              phx-value-filter="acc_not_matched"
              class={acc_filter_button_class(@acc_filter == :acc_not_matched, "red")}
              title="Only show journeys without ACC payment match"
            >
              No ACC
            </button>
          </div>

          <%!-- Advanced Filters Toggle --%>
          <button
            phx-click="toggle-advanced-filters"
            class="ml-auto flex items-center text-sm text-gray-600 hover:text-gray-900"
          >
            <span>Advanced</span>
            <svg class={"w-4 h-4 ml-1 transition-transform #{if @advanced_filters_open, do: "rotate-180"}"} fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M19 9l-7 7-7-7" />
            </svg>
          </button>
        </div>

        <%!-- Advanced Filters Panel (collapsible) --%>
        <div :if={@advanced_filters_open} class="mt-3 pt-3 border-t border-gray-200">
          <div class="flex flex-wrap gap-4 items-center">
            <%!-- DateTime Range Picker --%>
            <div class="flex items-center space-x-2">
              <label class="text-sm text-gray-600">From:</label>
              <input
                type="datetime-local"
                value={@from_datetime && format_datetime_local(@from_datetime)}
                phx-change="set-from-datetime"
                name="value"
                class="px-2 py-1 text-sm border border-gray-300 rounded"
              />
              <label class="text-sm text-gray-600">To:</label>
              <input
                type="datetime-local"
                value={@to_datetime && format_datetime_local(@to_datetime)}
                phx-change="set-to-datetime"
                name="value"
                class="px-2 py-1 text-sm border border-gray-300 rounded"
              />
              <button
                :if={@from_datetime || @to_datetime}
                phx-click="clear-datetime-range"
                class="text-gray-400 hover:text-gray-600"
                title="Clear datetime range"
              >
                <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                  <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 18L18 6M6 6l12 12" />
                </svg>
              </button>
            </div>

            <%!-- POS Zone Multi-Select --%>
            <.pos_zone_selector
              available_zones={@available_pos_zones}
              selected_zones={@selected_pos_zones}
              menu_open={@pos_menu_open}
            />
          </div>
        </div>
      </div>

      <%!-- Journey Cards Header --%>
      <div :if={length(@journeys) > 0} class="flex items-center justify-between mb-2">
        <span class="text-xs text-gray-500 uppercase tracking-wide font-medium">
          <%= length(@journeys) %> journeys
        </span>
        <div class="flex items-center gap-2">
          <button
            :if={MapSet.size(@expanded_journey_ids) > 0}
            phx-click="collapse_all"
            class="text-xs text-gray-500 hover:text-gray-700 px-2 py-1 hover:bg-gray-100 rounded"
          >
            Collapse all
          </button>
          <button
            :if={MapSet.size(@expanded_journey_ids) < length(@journeys)}
            phx-click="expand_all"
            class="text-xs text-gray-500 hover:text-gray-700 px-2 py-1 hover:bg-gray-100 rounded"
          >
            Expand all
          </button>
        </div>
      </div>

      <%!-- Journey Cards --%>
      <div class="space-y-2">
        <%= if Enum.empty?(@journeys) do %>
          <div class="text-center py-12 bg-white rounded-lg shadow">
            <p class="text-gray-500">No customer journeys</p>
            <p class="text-sm text-gray-400 mt-2">
              <%= if @person_id_search != "" or @from_date != nil or @to_date != nil or @pos_filter != :all or @selected_pos_zones != [] or @acc_filter != :all or @hide_short_journeys do %>
                Try adjusting your filters
              <% else %>
                Journeys will appear here when customers exit
              <% end %>
            </p>
          </div>
        <% else %>
          <%= for journey <- @journeys do %>
            <.journey_card
              journey={journey}
              expanded={MapSet.member?(@expanded_journey_ids, journey.id)}
            />
          <% end %>
        <% end %>
      </div>

      <%!-- Pagination --%>
      <div :if={@has_prev or @has_next} class="mt-4 flex items-center justify-center gap-2">
        <button
          :if={@has_prev}
          phx-click="prev-page"
          class="px-3 py-1.5 text-sm bg-white hover:bg-gray-50 border border-gray-200 rounded-md shadow-sm"
        >
          ← Previous
        </button>
        <button
          :if={@cursor}
          phx-click="first-page"
          class="px-3 py-1.5 text-sm text-gray-600 hover:bg-gray-100 rounded-md"
        >
          Latest
        </button>
        <button
          :if={@has_next}
          phx-click="next-page"
          class="px-3 py-1.5 text-sm bg-white hover:bg-gray-50 border border-gray-200 rounded-md shadow-sm"
        >
          Next →
        </button>
      </div>
    </div>
    """
  end

  defp filter_button_class(active, color) do
    base = "px-2 sm:px-3 py-1 rounded-md text-xs sm:text-sm font-medium"
    if active do
      case color do
        "blue" -> "#{base} bg-blue-600 text-white"
        "green" -> "#{base} bg-green-600 text-white"
        "yellow" -> "#{base} bg-yellow-600 text-white"
        "red" -> "#{base} bg-red-600 text-white"
        _ -> "#{base} bg-gray-600 text-white"
      end
    else
      "#{base} bg-gray-200 text-gray-700 hover:bg-gray-300"
    end
  end

  defp pos_filter_button_class(active, color \\ "blue")
  defp pos_filter_button_class(active, color) do
    base = "px-2 py-1 rounded text-xs font-medium"
    if active do
      case color do
        "red" -> "#{base} bg-red-600 text-white"
        _ -> "#{base} bg-blue-600 text-white"
      end
    else
      "#{base} bg-gray-100 text-gray-600 hover:bg-gray-200"
    end
  end

  defp acc_filter_button_class(active, color \\ "blue")
  defp acc_filter_button_class(active, color) do
    base = "px-2 py-1 rounded text-xs font-medium"
    if active do
      case color do
        "green" -> "#{base} bg-green-600 text-white"
        "red" -> "#{base} bg-red-600 text-white"
        _ -> "#{base} bg-blue-600 text-white"
      end
    else
      "#{base} bg-gray-100 text-gray-600 hover:bg-gray-200"
    end
  end

  defp format_selected_date(date) do
    today = Date.utc_today()
    cond do
      date == today -> "Today"
      date == Date.add(today, -1) -> "Yesterday"
      date == Date.add(today, 1) -> "Tomorrow"
      true -> Calendar.strftime(date, "%b %d")
    end
  end

  # Date/DateTime helpers for form inputs
  defp parse_date(""), do: {:ok, nil}
  defp parse_date(str) when is_binary(str) do
    case Date.from_iso8601(str) do
      {:ok, date} -> {:ok, date}
      _ -> :error
    end
  end

  defp format_datetime_local(%DateTime{} = dt) do
    Calendar.strftime(dt, "%Y-%m-%dT%H:%M")
  end
  defp format_datetime_local(_), do: nil

  defp parse_datetime_local(""), do: {:ok, nil}
  defp parse_datetime_local(str) when is_binary(str) do
    # datetime-local format: "2025-12-29T10:30"
    case NaiveDateTime.from_iso8601(str <> ":00") do
      {:ok, naive} -> {:ok, DateTime.from_naive!(naive, "Etc/UTC")}
      error -> error
    end
  end
  defp parse_datetime_local(_), do: {:error, :invalid}

  # POS zone selector component
  defp pos_zone_selector(assigns) do
    ~H"""
    <div class="relative">
      <button
        phx-click="toggle-pos-menu"
        class="flex items-center space-x-1 px-3 py-1 bg-gray-100 hover:bg-gray-200 rounded-md text-sm font-medium text-gray-700"
      >
        <span>POS Zones: <%= format_pos_label(@selected_zones) %></span>
        <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
          <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M19 9l-7 7-7-7" />
        </svg>
      </button>
      <div
        :if={@menu_open}
        class="absolute z-10 mt-1 w-48 bg-white border border-gray-200 rounded-md shadow-lg"
      >
        <%= if Enum.empty?(@available_zones) do %>
          <div class="p-3 text-sm text-gray-500">No POS zones found</div>
        <% else %>
          <div class="p-2 space-y-1 max-h-48 overflow-y-auto">
            <%= for zone <- @available_zones do %>
              <label class="flex items-center space-x-2 px-2 py-1 hover:bg-gray-50 rounded cursor-pointer">
                <input
                  type="checkbox"
                  phx-click="toggle-pos-zone"
                  phx-value-zone={zone}
                  checked={zone in @selected_zones}
                  class="rounded border-gray-300 text-blue-600 focus:ring-blue-500"
                />
                <span class="text-sm text-gray-700"><%= zone %></span>
              </label>
            <% end %>
          </div>
          <div :if={length(@selected_zones) > 0} class="border-t p-2">
            <button
              phx-click="clear-pos-zones"
              class="text-xs text-gray-500 hover:text-gray-700"
            >
              Clear all
            </button>
          </div>
        <% end %>
      </div>
    </div>
    """
  end

  defp format_pos_label([]), do: "All"
  defp format_pos_label(zones) when length(zones) == 1, do: hd(zones)
  defp format_pos_label(zones), do: "#{length(zones)} selected"

  defp journey_card(assigns) do
    # Determine border accent color based on exit type
    border_color = case assigns.journey.exit_type do
      "exit_confirmed" -> if assigns.journey.authorized, do: "border-l-green-500", else: "border-l-gray-300"
      "returned_to_store" -> "border-l-amber-400"
      "tracking_lost" -> "border-l-red-400"
      _ -> "border-l-gray-300"
    end
    assigns = assign(assigns, :border_color, border_color)

    ~H"""
    <div class={"bg-white shadow-sm rounded-lg overflow-hidden border-l-4 #{@border_color} transition-shadow hover:shadow-md"}>
      <%!-- Card Header --%>
      <div
        class="px-4 py-3 cursor-pointer hover:bg-gray-50/50 select-none"
        phx-click="toggle_expand"
        phx-value-id={@journey.id}
      >
        <%!-- Single row: compact info display --%>
        <div class="flex items-center justify-between gap-4">
          <%!-- Left: Exit badge + Person ID --%>
          <div class="flex items-center gap-3 min-w-0">
            <.exit_type_badge exit_type={@journey.exit_type} />
            <span class="font-mono font-semibold text-gray-900 tabular-nums">
              #<%= @journey.person_id %>
            </span>
            <%= if is_stitched?(@journey) do %>
              <span class="px-1 py-0.5 text-[10px] font-medium rounded bg-purple-100 text-purple-700" title="Track stitched">
                🔗
              </span>
            <% end %>
            <%= if @journey.member_count > 1 do %>
              <% other_members = Enum.reject(@journey.group_member_ids || [], &(&1 == @journey.person_id)) %>
              <span class="px-1 py-0.5 text-[10px] font-medium rounded bg-blue-100 text-blue-700" title={"ACC group with ##{Enum.join(other_members, ", #")}"}>
                👥 w/ <%= Enum.map_join(other_members, ", ", &"##{&1}") %>
              </span>
            <% end %>
            <.payment_status
              authorized={@journey.authorized}
              payment_zone={@journey.payment_zone}
              total_pos_dwell_ms={@journey.total_pos_dwell_ms}
            />
          </div>

          <%!-- Right: Time + Duration + Indicators + Expand --%>
          <div class="flex items-center gap-3 flex-shrink-0">
            <%= if @journey.tailgated do %>
              <span class="text-xs text-orange-600 font-medium">⚠ Tail</span>
            <% end %>
            <%= if quick_exit?(@journey) do %>
              <span class="text-xs text-amber-500 font-medium">⚡</span>
            <% end %>
            <%= if @journey.duration_ms do %>
              <span class="text-xs text-gray-400 tabular-nums">
                <%= format_duration(@journey.duration_ms) %>
              </span>
            <% end %>
            <span class="text-sm text-gray-600 tabular-nums font-medium">
              <%= format_time(@journey.time) %>
            </span>
            <svg class={"w-4 h-4 text-gray-400 transition-transform #{if @expanded, do: "rotate-180"}"} fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M19 9l-7 7-7-7" />
            </svg>
          </div>
        </div>
      </div>

      <%!-- Expanded Journey Detail --%>
      <%= if @expanded do %>
        <div class="border-t border-gray-100 bg-gray-50/30">
          <%!-- Timing Summary --%>
          <.journey_timing_summary journey={@journey} />
          <%!-- Timeline --%>
          <div class="px-4 py-3 bg-gray-50">
            <.journey_timeline events={@journey.events || []} />
          </div>
        </div>
      <% end %>
    </div>
    """
  end

  defp exit_type_badge(assigns) do
    {bg_class, text} = case assigns.exit_type do
      "exit_confirmed" -> {"bg-green-100 text-green-700", "EXIT"}
      "returned_to_store" -> {"bg-amber-100 text-amber-700", "RTN"}
      "tracking_lost" -> {"bg-red-100 text-red-700", "LOST"}
      _ -> {"bg-gray-100 text-gray-600", "?"}
    end
    assigns = assign(assigns, :bg_class, bg_class)
    assigns = assign(assigns, :text, text)

    ~H"""
    <span class={"px-1.5 py-0.5 text-[10px] font-bold rounded tracking-wide #{@bg_class}"}>
      <%= @text %>
    </span>
    """
  end

  defp payment_status(assigns) do
    ~H"""
    <%= if @authorized do %>
      <span class="inline-flex items-center gap-1 px-2 py-0.5 rounded-full bg-green-50 text-green-700 text-xs font-medium">
        <svg class="w-3 h-3" fill="currentColor" viewBox="0 0 20 20">
          <path fill-rule="evenodd" d="M16.707 5.293a1 1 0 010 1.414l-8 8a1 1 0 01-1.414 0l-4-4a1 1 0 011.414-1.414L8 12.586l7.293-7.293a1 1 0 011.414 0z" clip-rule="evenodd" />
        </svg>
        Paid
        <%= if @payment_zone do %>
          <span class="text-green-600/70">@ <%= @payment_zone %></span>
        <% end %>
      </span>
    <% else %>
      <span class="inline-flex items-center px-2 py-0.5 rounded-full bg-red-50 text-red-600 text-xs font-medium">
        Unpaid
      </span>
    <% end %>
    """
  end

  # Check if this is a "quick exit" - bypassed checkout entirely
  # Quick = went straight from entry to exit without stopping at any POS zone
  defp quick_exit?(journey) do
    # If they spent >= 7s at any POS zone, they stopped at checkout (not quick)
    has_pos_stop = journey.total_pos_dwell_ms != nil and journey.total_pos_dwell_ms >= @min_dwell_ms
    # Quick = no meaningful POS stop AND not authorized
    not has_pos_stop and journey.authorized != true
  end

  # Check if journey had track stitching (track ID changed mid-journey)
  defp is_stitched?(journey) do
    Enum.any?(journey.events || [], fn event ->
      event["type"] == "stitch"
    end)
  end

  # Timing summary shown in expanded view
  defp journey_timing_summary(assigns) do
    events = assigns.journey.events || []

    # Extract timing data from events - support both old and new event type names
    gate_cmd = find_event_by_type(events, "gate_cmd") || find_event_by_type(events, "gate_open_requested")
    gate_open = find_event_by_type(events, "gate_open") || find_event_by_type(events, "gate_opened")
    last_pos_exit = find_last_pos_exit(events)
    exit_event = find_event_by_type(events, "exit_cross") || find_event_by_type(events, "exit")
    entry_event = find_event_by_type(events, "entry_cross") || find_event_by_type(events, "entry")

    # Gate Open: prefer gate_open (RS485 confirmation), fall back to gate_cmd
    gate_open_confirmed_at = get_event_timestamp(gate_open)

    assigns =
      assigns
      |> assign(:entry_at, get_event_timestamp(entry_event))
      |> assign(:gate_cmd_at, get_event_timestamp(gate_cmd))
      |> assign(:gate_open_confirmed_at, gate_open_confirmed_at)
      |> assign(:pos_exit_at, get_event_timestamp(last_pos_exit))
      |> assign(:exit_at, get_event_timestamp(exit_event))

    ~H"""
    <div class="px-4 py-2 bg-white border-b border-gray-100">
      <div class="grid grid-cols-2 sm:grid-cols-5 gap-2 text-xs">
        <div>
          <span class="text-gray-500">Total:</span>
          <span class="font-medium ml-1"><%= format_duration(@journey.duration_ms) || "-" %></span>
        </div>
        <div>
          <span class="text-gray-500">Entry:</span>
          <span class="font-medium ml-1"><%= format_event_timestamp(@entry_at) %></span>
        </div>
        <div>
          <span class="text-gray-500">Gate Cmd:</span>
          <span class="font-medium ml-1"><%= format_event_timestamp(@gate_cmd_at) %></span>
        </div>
        <div>
          <span class="text-gray-500">Gate Opened:</span>
          <span class="font-medium ml-1">
            <%= if @gate_open_confirmed_at do %>
              <%= format_event_timestamp(@gate_open_confirmed_at) %>
            <% else %>
              <span class="text-gray-400">(no RS485)</span>
            <% end %>
          </span>
        </div>
        <div>
          <span class="text-gray-500">POS→Exit:</span>
          <span class="font-medium ml-1"><%= format_pos_to_exit_time(@pos_exit_at, @exit_at) %></span>
        </div>
      </div>
    </div>
    """
  end

  defp find_event_by_type(events, type) do
    Enum.find(events, fn e -> e["type"] == type end)
  end

  defp find_last_pos_exit(events) do
    # Try to find zone_exit from POS zone first
    pos_exit =
      events
      |> Enum.filter(fn e ->
        e["type"] == "zone_exit" and
        is_binary(get_in(e, ["data", "zone"])) and
        String.starts_with?(get_in(e, ["data", "zone"]), "POS")
      end)
      |> List.last()

    # Fall back to acc event if no POS zone exit recorded
    # (handles cases where zone tracking was lost but payment was received)
    pos_exit || find_event_by_type(events, "acc") || find_event_by_type(events, "acc_payment")
  end

  defp get_event_timestamp(nil), do: nil
  defp get_event_timestamp(%{"ts" => ts}) when is_binary(ts) do
    case DateTime.from_iso8601(ts) do
      {:ok, dt, _} -> dt
      _ -> nil
    end
  end
  defp get_event_timestamp(_), do: nil

  defp format_event_timestamp(nil), do: "-"
  defp format_event_timestamp(dt), do: Calendar.strftime(dt, "%H:%M:%S")

  defp format_pos_to_exit_time(nil, _), do: "-"
  defp format_pos_to_exit_time(_, nil), do: "-"
  defp format_pos_to_exit_time(pos_exit, exit_time) do
    diff_ms = DateTime.diff(exit_time, pos_exit, :millisecond)
    if diff_ms >= 0 do
      format_duration(diff_ms)
    else
      "-"
    end
  end

  defp journey_timeline(assigns) do
    # Build a map of zone -> dwell_ms from zone_exit events for quick lookup
    zone_dwells =
      assigns.events
      |> Enum.filter(&(&1["type"] == "zone_exit"))
      |> Enum.map(fn e -> {get_in(e, ["data", "zone"]), get_in(e, ["data", "dwell_ms"]) || 0} end)
      |> Map.new()

    # Filter events:
    # - Remove state_change (redundant with zone events)
    # - Remove dwell_threshold (we'll show dwell info on zone_exit instead)
    # - Remove quick POS stops (zone_entry/exit where dwell < 7s for POS zones)
    filtered_events =
      assigns.events
      |> Enum.reject(fn event ->
        type = event["type"]
        zone = get_in(event, ["data", "zone"])
        is_pos = zone && String.starts_with?(zone, "POS")
        dwell = Map.get(zone_dwells, zone, 0)

        cond do
          type == "state_change" -> true
          type == "dwell_threshold" -> true  # We show this info on zone_exit instead
          type == "zone_entry" and is_pos and dwell < @min_dwell_ms -> true
          type == "zone_exit" and is_pos and dwell < @min_dwell_ms -> true
          true -> false
        end
      end)
      |> Enum.reverse()

    # Calculate timing info for each event
    events_with_timing = calculate_event_timings(filtered_events)

    assigns = assign(assigns, :events_with_timing, events_with_timing)

    ~H"""
    <div class="space-y-2">
      <div class="flex items-center justify-between">
        <h4 class="text-xs font-semibold text-gray-500 uppercase tracking-wider">Journey Timeline</h4>
        <div class="text-xs text-gray-400 font-mono">
          <%= length(@events_with_timing) %> events
        </div>
      </div>
      <%= if Enum.empty?(@events_with_timing) do %>
        <p class="text-sm text-gray-400 italic">No journey events recorded</p>
      <% else %>
        <div class="overflow-x-auto">
          <table class="w-full text-xs">
            <thead>
              <tr class="border-b border-gray-200 text-left text-gray-500">
                <th class="py-1.5 pr-3 font-medium">Event</th>
                <th class="py-1.5 px-2 font-medium text-right whitespace-nowrap">+Start</th>
                <th class="py-1.5 px-2 font-medium text-right whitespace-nowrap">Gap</th>
                <th class="py-1.5 px-2 font-medium whitespace-nowrap">Time</th>
              </tr>
            </thead>
            <tbody class="font-mono">
              <%= for {event, timing} <- @events_with_timing do %>
                <.timeline_table_row event={event} timing={timing} />
              <% end %>
            </tbody>
          </table>
        </div>
      <% end %>
    </div>
    """
  end

  # Calculate timing info: time since start, delta from previous event
  # Events come in newest-first order (already reversed), so last = earliest = journey start
  defp calculate_event_timings(events) do
    case events do
      [] -> []
      _ ->
        # Get earliest event (last in list since reversed) as journey start
        start_time = events |> List.last() |> parse_event_timestamp()

        # Walk through events (newest first), tracking previous event for delta
        {results, _} = Enum.map_reduce(events, {start_time, nil}, fn event, {start_ts, prev_ts} ->
          event_ts = parse_event_timestamp(event)

          timing = %{
            absolute: event_ts,
            # Time since journey START (positive = after start)
            since_start: if(start_ts && event_ts, do: DateTime.diff(event_ts, start_ts, :millisecond), else: nil),
            # Time since PREVIOUS event in display order (newest first, so delta = prev - current)
            delta: if(prev_ts && event_ts, do: DateTime.diff(prev_ts, event_ts, :millisecond), else: nil)
          }

          {{event, timing}, {start_ts, event_ts}}
        end)
        results
    end
  end

  defp parse_event_timestamp(%{"ts" => ts}) when is_binary(ts) do
    case DateTime.from_iso8601(ts) do
      {:ok, dt, _} -> dt
      _ -> nil
    end
  end
  defp parse_event_timestamp(_), do: nil

  # Table row for timeline - cleaner columnar layout
  defp timeline_table_row(assigns) do
    {icon, color, description} = format_journey_event(assigns.event)

    assigns = assigns
      |> assign(:icon, icon)
      |> assign(:color, color)
      |> assign(:description, description)

    ~H"""
    <tr class="border-b border-gray-100 hover:bg-gray-50/50">
      <!-- Event description -->
      <td class="py-1.5 pr-3">
        <div class="flex items-center gap-1.5">
          <span class={@color}><%= @icon %></span>
          <span class="text-gray-700"><%= @description %></span>
        </div>
      </td>

      <!-- Time since start -->
      <td class="py-1.5 px-2 text-right text-gray-500 whitespace-nowrap">
        <%= if @timing.since_start do %>
          +<%= format_elapsed_ms(@timing.since_start) %>
        <% else %>
          -
        <% end %>
      </td>

      <!-- Gap (time since previous event) -->
      <td class="py-1.5 px-2 text-right whitespace-nowrap">
        <%= if @timing.delta && @timing.delta > 0 do %>
          <span class={"inline-block px-1 py-0.5 rounded text-xs #{delta_color(@timing.delta)}"}>
            <%= format_delta_ms(@timing.delta) %>
          </span>
        <% else %>
          <span class="text-gray-300">-</span>
        <% end %>
      </td>

      <!-- Event timestamp -->
      <td class="py-1.5 px-2 text-gray-600 whitespace-nowrap">
        <%= format_time_with_ms(@timing.absolute) %>
      </td>
    </tr>
    """
  end


  # Color delta badges based on time - helps spot delays
  defp delta_color(ms) when ms < 100, do: "bg-green-100 text-green-700"
  defp delta_color(ms) when ms < 500, do: "bg-blue-100 text-blue-700"
  defp delta_color(ms) when ms < 1000, do: "bg-yellow-100 text-yellow-700"
  defp delta_color(_ms), do: "bg-red-100 text-red-700"

  # Format time with milliseconds: "09:36:11.234"
  defp format_time_with_ms(nil), do: "-"
  defp format_time_with_ms(%DateTime{} = dt) do
    ms = div(elem(dt.microsecond, 0), 1000)
    Calendar.strftime(dt, "%H:%M:%S") <> "." <> String.pad_leading("#{ms}", 3, "0")
  end

  # Format elapsed time: "0.234s" or "1.50s" or "12.5s" or "1m 23s"
  defp format_elapsed_ms(nil), do: "-"
  defp format_elapsed_ms(ms) when ms < 10_000 do
    seconds = ms / 1000
    :erlang.float_to_binary(seconds, decimals: 2) <> "s"
  end
  defp format_elapsed_ms(ms) when ms < 60_000 do
    seconds = ms / 1000
    :erlang.float_to_binary(seconds, decimals: 1) <> "s"
  end
  defp format_elapsed_ms(ms) do
    total_seconds = div(ms, 1000)
    minutes = div(total_seconds, 60)
    seconds = rem(total_seconds, 60)
    "#{minutes}m #{seconds}s"
  end

  # Format delta time: "0.15s" or "1.2s"
  defp format_delta_ms(nil), do: "-"
  defp format_delta_ms(ms) when ms < 1000 do
    seconds = ms / 1000
    :erlang.float_to_binary(seconds, decimals: 2) <> "s"
  end
  defp format_delta_ms(ms) when ms < 10_000 do
    seconds = ms / 1000
    :erlang.float_to_binary(seconds, decimals: 1) <> "s"
  end
  defp format_delta_ms(ms) do
    format_elapsed_ms(ms)
  end

  defp format_journey_event(%{"type" => "zone_entry"} = event) do
    zone = get_in(event, ["data", "zone"]) || "?"
    cond do
      String.starts_with?(zone, "POS") ->
        {"→", "text-blue-600", "#{zone}"}
      String.starts_with?(zone, "GATE") ->
        {"→", "text-purple-500", "#{zone}"}
      true ->
        {"→", "text-gray-400", "#{zone}"}
    end
  end

  defp format_journey_event(%{"type" => "zone_exit"} = event) do
    zone = get_in(event, ["data", "zone"]) || "?"
    dwell = get_in(event, ["data", "dwell_ms"])
    is_pos = String.starts_with?(zone, "POS")
    dwell_str = if dwell && dwell > 0, do: " #{format_duration(dwell)}", else: ""

    cond do
      is_pos and dwell && dwell >= 7000 ->
        {"←", "text-green-600", "#{zone}#{dwell_str} ✓"}
      is_pos ->
        {"←", "text-blue-500", "#{zone}#{dwell_str}"}
      true ->
        {"←", "text-gray-400", "#{zone}#{dwell_str}"}
    end
  end

  # ACC payment event from Rust backend
  defp format_journey_event(%{"type" => "acc"} = event) do
    zone = get_in(event, ["data", "zone"]) || "?"
    zone_display = format_zone_name(zone)
    kiosk = get_in(event, ["data", "kiosk"])
    kiosk_str = if kiosk, do: " (kiosk #{String.split(kiosk, ".") |> List.last()})", else: ""
    group = get_in(event, ["data", "group"])
    group_str = if group && group > 1, do: " [group: #{group}]", else: ""
    {"💳", "text-green-600", "Payment at #{zone_display}#{kiosk_str}#{group_str}"}
  end

  # Legacy payment event format
  defp format_journey_event(%{"type" => "payment"} = event) do
    zone = get_in(event, ["data", "zone"]) || "?"
    zone_display = format_zone_name(zone)
    receipt_id = get_in(event, ["data", "receipt_id"])
    receipt_str = if receipt_id, do: " (#{String.slice(receipt_id, -8..-1)})", else: ""
    {"💳", "text-green-600", "Payment at #{zone_display}#{receipt_str}"}
  end

  defp format_journey_event(%{"type" => "dwell_threshold"} = event) do
    zone = get_in(event, ["data", "zone"]) || "?"
    zone_display = format_zone_name(zone)
    {"⏱", "text-orange-500", "Dwell threshold at #{zone_display}"}
  end

  # Legacy support for old event format
  defp format_journey_event(%{"type" => "dwell_met"} = event) do
    zone = get_in(event, ["data", "zone"]) || "?"
    zone_display = format_zone_name(zone)
    {"⏱", "text-orange-500", "Dwell threshold at #{zone_display}"}
  end

  defp format_journey_event(%{"type" => "state_change"} = event) do
    from = get_in(event, ["data", "from_state"])
    to = get_in(event, ["data", "to_state"]) || get_in(event, ["data", "to"]) || "?"
    if from do
      {"◆", "text-purple-500", "#{from} → #{to}"}
    else
      {"◆", "text-purple-500", "State: #{to}"}
    end
  end

  defp format_journey_event(%{"type" => "line_cross"} = event) do
    line = get_in(event, ["data", "line"]) || "?"
    direction = get_in(event, ["data", "direction"]) || ""
    {"↗", "text-indigo-500", "Crossed #{line} (#{direction})"}
  end

  # Entry line crossing
  defp format_journey_event(%{"type" => "entry_cross"} = event) do
    direction = get_in(event, ["data", "dir"]) || get_in(event, ["data", "direction"]) || ""
    {"⊕", "text-green-600", "ENTRY #{direction}"}
  end

  # Exit line crossing
  defp format_journey_event(%{"type" => "exit_cross"} = event) do
    direction = get_in(event, ["data", "dir"]) || get_in(event, ["data", "direction"]) || ""
    {"✓", "text-green-600", "EXIT #{direction}"}
  end

  # Approach line crossing
  defp format_journey_event(%{"type" => "approach_cross"} = event) do
    direction = get_in(event, ["data", "dir"]) || get_in(event, ["data", "direction"]) || ""
    {"↗", "text-purple-500", "APPROACH #{direction}"}
  end

  # Track created
  defp format_journey_event(%{"type" => "track_create"} = _event) do
    {"○", "text-gray-500", "Track started"}
  end

  # Track pending stitch
  defp format_journey_event(%{"type" => "pending"} = _event) do
    {"◌", "text-yellow-500", "Pending stitch"}
  end

  # Legacy exit event format
  defp format_journey_event(%{"type" => "exit"} = event) do
    authorized = get_in(event, ["data", "authorized"])
    tailgated = get_in(event, ["data", "tailgated"])
    gate_opened_by = get_in(event, ["data", "gate_opened_by"])
    gate_opener_person = get_in(event, ["data", "gate_opener_person"])

    cond do
      tailgated ->
        # Only show person ID if it's a valid positive integer (not 0 or nil)
        opener_info = cond do
          is_integer(gate_opener_person) and gate_opener_person > 0 ->
            " (gate opened by ##{gate_opener_person})"
          gate_opened_by == "sensor" ->
            " (sensor-triggered)"
          true ->
            ""
        end
        {"⚠", "text-orange-600", "Tailgated exit#{opener_info}"}
      authorized ->
        opener = if gate_opened_by, do: " (#{gate_opened_by})", else: ""
        {"✓", "text-green-600", "Exited through gate#{opener}"}
      true ->
        {"✗", "text-red-600", "Unauthorized exit"}
    end
  end

  defp format_journey_event(%{"type" => "returned_to_store"} = _event) do
    {"↩", "text-blue-500", "Returned to store"}
  end

  # Gate command sent - from Rust backend
  defp format_journey_event(%{"type" => "gate_cmd"} = event) do
    cmd_us = get_in(event, ["data", "cmd_us"])
    e2e_us = get_in(event, ["data", "e2e_us"])
    latency_str = cond do
      e2e_us && e2e_us > 0 -> " (#{e2e_us}µs)"
      cmd_us && cmd_us > 0 -> " (#{cmd_us}µs)"
      true -> ""
    end
    {"🚪", "text-indigo-500", "Gate open command sent#{latency_str}"}
  end

  # Legacy gate_open_requested format
  defp format_journey_event(%{"type" => "gate_open_requested"} = event) do
    gate_id = get_in(event, ["data", "gate_id"])
    gate_str = if gate_id, do: " (Gate #{gate_id})", else: ""
    {"🚪", "text-indigo-500", "Gate open command sent#{gate_str}"}
  end

  # Gate opened confirmation (RS485)
  defp format_journey_event(%{"type" => "gate_open"} = _event) do
    {"✓", "text-green-500", "Gate opened (RS485 confirmed)"}
  end

  # Legacy gate_opened format
  defp format_journey_event(%{"type" => "gate_opened"} = event) do
    source = get_in(event, ["data", "source"]) || "RS485"
    {"✓", "text-green-500", "Gate opened (#{source})"}
  end

  defp format_journey_event(%{"type" => type}) do
    {"•", "text-gray-400", type}
  end

  defp format_journey_event(_), do: {"•", "text-gray-400", "Unknown event"}

  defp format_time(nil), do: "-"
  defp format_time(%DateTime{} = dt) do
    Calendar.strftime(dt, "%H:%M:%S")
  end
  defp format_time(_), do: "-"

  defp format_duration(nil), do: "-"
  defp format_duration(ms) when is_integer(ms) do
    seconds = div(ms, 1000)
    cond do
      seconds < 60 -> "#{seconds}s"
      seconds < 3600 -> "#{div(seconds, 60)}m #{rem(seconds, 60)}s"
      true -> "#{div(seconds, 3600)}h #{rem(div(seconds, 60), 60)}m"
    end
  end
  defp format_duration(_), do: "-"

  # Format zone name for display (removes common prefixes)
  defp format_zone_name(nil), do: "?"
  defp format_zone_name(zone) when is_binary(zone) do
    zone
    |> String.replace(~r/^ZONE[-_]?/i, "")
    |> String.replace("_", " ")
  end
  defp format_zone_name(zone), do: to_string(zone)

  # Site selector component
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

  defp format_site_label(sites) when length(sites) == 1, do: format_site_name(hd(sites))
  defp format_site_label(sites), do: "#{length(sites)} selected"

  defp format_site_name("AP-NETTO-GR-01"), do: "Netto"
  defp format_site_name("AP-AVERO-GR-01"), do: "Avero"
  defp format_site_name("docker-gateway"), do: "Docker"
  defp format_site_name(site), do: site
end
