defmodule AveroCommandWeb.IncidentFeedLive do
  use AveroCommandWeb, :live_view

  alias AveroCommand.Incidents

  @impl true
  def mount(_params, _session, socket) do
    if connected?(socket) do
      # Subscribe to real-time incident updates
      Phoenix.PubSub.subscribe(AveroCommand.PubSub, "incidents")
    end

    incidents = load_incidents(socket.assigns.selected_sites, :all)

    {:ok,
     socket
     |> assign(:incidents, incidents)
     |> assign(:filter, :all)
     |> assign(:page_title, "Incidents")
     # Journey modal state
     |> assign(:journey_modal_open, false)
     |> assign(:journey_session_id, nil)
     |> assign(:journey_person_id, nil)
     |> assign(:journey_data, nil)
     |> assign(:journey_loading, false)}
  end

  @impl true
  def handle_info({:incident_created, incident}, socket) do
    # Only add if incident is from selected sites
    if incident.site in socket.assigns.selected_sites do
      incidents = [incident | socket.assigns.incidents]
      {:noreply, assign(socket, :incidents, incidents)}
    else
      {:noreply, socket}
    end
  end

  @impl true
  def handle_info({:incident_updated, incident}, socket) do
    incidents =
      Enum.map(socket.assigns.incidents, fn i ->
        if i.id == incident.id, do: incident, else: i
      end)

    {:noreply, assign(socket, :incidents, incidents)}
  end

  @impl true
  def handle_info({:fetch_journey, session_id}, socket) do
    # Fetch journey from gateway API
    journey_data = fetch_journey_from_gateway(session_id)

    {:noreply,
     socket
     |> assign(:journey_loading, false)
     |> assign(:journey_data, journey_data)}
  end

  @impl true
  def handle_event("acknowledge", %{"id" => id}, socket) do
    case Incidents.acknowledge(id) do
      {:ok, _incident} ->
        {:noreply, put_flash(socket, :info, "Incident acknowledged")}

      {:error, _} ->
        {:noreply, put_flash(socket, :error, "Failed to acknowledge incident")}
    end
  end

  @impl true
  def handle_event("dismiss", %{"id" => id}, socket) do
    case Incidents.resolve(id, "dismissed") do
      {:ok, _incident} ->
        incidents = Enum.reject(socket.assigns.incidents, &(&1.id == id))
        {:noreply,
         socket
         |> assign(:incidents, incidents)
         |> put_flash(:info, "Incident dismissed")}

      {:error, _} ->
        {:noreply, put_flash(socket, :error, "Failed to dismiss incident")}
    end
  end

  @impl true
  def handle_event("dismiss-all", _params, socket) do
    case Incidents.dismiss_all(socket.assigns.selected_sites) do
      {:ok, count} ->
        {:noreply,
         socket
         |> assign(:incidents, [])
         |> put_flash(:info, "#{count} incidents dismissed")}

      {:error, _} ->
        {:noreply, put_flash(socket, :error, "Failed to dismiss incidents")}
    end
  end

  @impl true
  def handle_event("filter", %{"filter" => filter}, socket) do
    filter = String.to_existing_atom(filter)
    incidents = load_incidents(socket.assigns.selected_sites, filter)

    {:noreply,
     socket
     |> assign(:filter, filter)
     |> assign(:incidents, incidents)}
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

    incidents = load_incidents(selected, socket.assigns.filter)

    {:noreply,
     socket
     |> assign(:selected_sites, selected)
     |> assign(:incidents, incidents)}
  end

  @impl true
  def handle_event("show-journey", %{"session-id" => session_id, "person-id" => person_id}, socket) do
    # Open modal and start loading journey data
    socket =
      socket
      |> assign(:journey_modal_open, true)
      |> assign(:journey_session_id, session_id)
      |> assign(:journey_person_id, person_id)
      |> assign(:journey_loading, true)
      |> assign(:journey_data, nil)

    # Fetch journey data async
    send(self(), {:fetch_journey, session_id})

    {:noreply, socket}
  end

  @impl true
  def handle_event("close-journey-modal", _params, socket) do
    {:noreply,
     socket
     |> assign(:journey_modal_open, false)
     |> assign(:journey_session_id, nil)
     |> assign(:journey_person_id, nil)
     |> assign(:journey_data, nil)
     |> assign(:journey_loading, false)}
  end

  defp fetch_journey_from_gateway(session_id) when is_binary(session_id) and session_id != "" do
    # Get gateway URL from config
    gateway_url = Application.get_env(:avero_command, :gateway_url, "http://localhost:8080")
    url = "#{gateway_url}/api/v1/journey/session/#{session_id}"

    # Use Erlang's built-in :httpc module
    :inets.start()
    :ssl.start()

    case :httpc.request(:get, {String.to_charlist(url), []}, [{:timeout, 5000}], []) do
      {:ok, {{_, 200, _}, _, body}} ->
        case Jason.decode(List.to_string(body)) do
          {:ok, data} -> data
          _ -> nil
        end

      _ ->
        nil
    end
  rescue
    _ -> nil
  end

  defp fetch_journey_from_gateway(_), do: nil

  defp load_incidents(sites, filter) do
    case filter do
      :all -> Incidents.list_active(sites: sites)
      :high -> Incidents.list_by_severity("high", sites: sites)
      :new -> Incidents.list_by_status("new", sites: sites)
    end
  end

  @impl true
  def render(assigns) do
    ~H"""
    <div class="incident-feed">
      <div class="mb-4 sm:mb-6 flex flex-col sm:flex-row sm:items-center sm:justify-between gap-3">
        <div class="flex items-center justify-between sm:justify-start space-x-4">
          <h2 class="text-base sm:text-lg font-semibold text-gray-900 dark:text-white">Active Incidents</h2>
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
            class={[
              "px-2 sm:px-3 py-1 rounded-md text-xs sm:text-sm font-medium",
              @filter == :all && "bg-blue-600 text-white",
              @filter != :all && "bg-gray-200 text-gray-700 hover:bg-gray-300 dark:bg-gray-700 dark:text-gray-300 dark:hover:bg-gray-600"
            ]}
          >
            All
          </button>
          <button
            phx-click="filter"
            phx-value-filter="high"
            class={[
              "px-2 sm:px-3 py-1 rounded-md text-xs sm:text-sm font-medium",
              @filter == :high && "bg-red-600 text-white",
              @filter != :high && "bg-gray-200 text-gray-700 hover:bg-gray-300 dark:bg-gray-700 dark:text-gray-300 dark:hover:bg-gray-600"
            ]}
          >
            High
          </button>
          <button
            phx-click="filter"
            phx-value-filter="new"
            class={[
              "px-2 sm:px-3 py-1 rounded-md text-xs sm:text-sm font-medium",
              @filter == :new && "bg-yellow-600 text-white",
              @filter != :new && "bg-gray-200 text-gray-700 hover:bg-gray-300 dark:bg-gray-700 dark:text-gray-300 dark:hover:bg-gray-600"
            ]}
          >
            New
          </button>
          <%= if length(@incidents) > 0 do %>
            <button
              phx-click="dismiss-all"
              data-confirm="Dismiss all visible incidents?"
              class="px-2 sm:px-3 py-1 rounded-md text-xs sm:text-sm font-medium bg-gray-500 text-white hover:bg-gray-600"
            >
              Dismiss (<%= length(@incidents) %>)
            </button>
          <% end %>
        </div>
      </div>

      <div class="space-y-4">
        <%= if banner = gate_offline_banner(@incidents) do %>
          <div class="rounded-lg border border-red-200 bg-red-50 p-4">
            <div class="flex items-start justify-between gap-4">
              <div>
                <p class="text-sm font-semibold text-red-800">Gate Offline</p>
                <p class="mt-1 text-sm text-red-700"><%= banner_message(banner) %></p>
                <p class="mt-1 text-xs text-red-600">
                  <%= banner.site %> • <%= format_time(banner.created_at) %>
                </p>
              </div>
              <.link
                navigate={~p"/incidents/#{banner.id}"}
                class="self-center px-3 py-1.5 text-sm font-medium bg-red-600 text-white rounded hover:bg-red-700"
              >
                View
              </.link>
            </div>
          </div>
        <% end %>
        <%= if Enum.empty?(@incidents) do %>
          <div class="text-center py-12 bg-white rounded-lg shadow dark:bg-gray-800">
            <p class="text-gray-500 dark:text-gray-400">No active incidents</p>
            <p class="text-sm text-gray-400 dark:text-gray-500 mt-2">Incidents will appear here when detected</p>
          </div>
        <% else %>
          <%= for incident <- @incidents do %>
            <%= if incident.type == "tailgating_detected" and has_enriched_context?(incident) do %>
              <.tailgate_incident_card incident={incident} />
            <% else %>
              <.incident_card incident={incident} />
            <% end %>
          <% end %>
        <% end %>
      </div>

      <%!-- Journey Modal --%>
      <.journey_modal
        :if={@journey_modal_open}
        session_id={@journey_session_id}
        person_id={@journey_person_id}
        journey_data={@journey_data}
        loading={@journey_loading}
      />
    </div>
    """
  end

  defp has_enriched_context?(incident) do
    ctx = incident.context || %{}
    Map.has_key?(ctx, "authorized_person_id") and Map.has_key?(ctx, "unauthorized_person_id")
  end

  defp incident_card(assigns) do
    ~H"""
    <div class={[
      "bg-white shadow rounded-lg p-4 border-l-4 dark:bg-gray-800",
      severity_border(@incident.severity)
    ]}>
      <div class="flex items-start justify-between">
        <div class="flex-1">
          <div class="flex items-center space-x-2">
            <span class={["px-2 py-1 text-xs font-medium rounded", severity_badge(@incident.severity)]}>
              <%= String.upcase(@incident.severity) %>
            </span>
            <span class="text-sm font-medium text-gray-900 dark:text-white">
              <%= format_type(@incident.type) %>
            </span>
            <span class="text-sm text-gray-500 dark:text-gray-400">
              <%= @incident.site %>
            </span>
          </div>
          <p class="mt-2 text-sm text-gray-600 dark:text-gray-300">
            <%= format_context(@incident.context) %>
          </p>
          <p class="mt-1 text-xs text-gray-400 dark:text-gray-500">
            <%= format_time(@incident.created_at) %>
          </p>
        </div>
        <div class="flex space-x-2">
          <%= if @incident.status == "new" do %>
            <button
              phx-click="acknowledge"
              phx-value-id={@incident.id}
              class="px-3 py-1 bg-blue-600 text-white text-sm rounded hover:bg-blue-700"
            >
              Acknowledge
            </button>
          <% end %>
          <.link
            navigate={~p"/incidents/#{@incident.id}"}
            class="px-3 py-1 bg-gray-200 text-gray-700 text-sm rounded hover:bg-gray-300 dark:bg-gray-700 dark:text-gray-300 dark:hover:bg-gray-600"
          >
            Details
          </.link>
        </div>
      </div>
    </div>
    """
  end

  # Specialized tailgate incident card with clear authorized/unauthorized display
  defp tailgate_incident_card(assigns) do
    ctx = assigns.incident.context || %{}
    assigns = assign(assigns, :ctx, ctx)
    ~H"""
    <div class={[
      "bg-white shadow rounded-lg overflow-hidden border-l-4 dark:bg-gray-800",
      severity_border(@incident.severity)
    ]}>
      <%!-- Header --%>
      <div class="px-4 py-3 bg-gray-50 border-b border-gray-200 dark:bg-gray-700/50 dark:border-gray-700">
        <div class="flex items-center justify-between">
          <div class="flex items-center space-x-2">
            <span class={["px-2 py-1 text-xs font-medium rounded", severity_badge(@incident.severity)]}>
              <%= String.upcase(@incident.severity) %>
            </span>
            <span class="text-sm font-bold text-gray-900 dark:text-white">TAILGATE DETECTED</span>
            <span class="text-sm text-gray-500 dark:text-gray-400"><%= @incident.site %></span>
          </div>
          <span class="text-xs text-gray-400 dark:text-gray-500"><%= format_time(@incident.created_at) %></span>
        </div>
      </div>

      <%!-- Person Details --%>
      <div class="px-4 py-3 space-y-3">
        <%!-- Authorized Person --%>
        <div class="flex items-start space-x-3 p-2 bg-green-50 rounded-lg border border-green-200">
          <div class="flex-shrink-0">
            <span class="inline-flex items-center justify-center w-6 h-6 rounded-full bg-green-500 text-white text-xs font-bold">
              ✓
            </span>
          </div>
          <div class="flex-1 min-w-0">
            <div class="flex items-center space-x-2">
              <span class="px-2 py-0.5 text-xs font-medium bg-green-100 text-green-800 rounded">AUTHORIZED</span>
              <.person_link
                person_id={@ctx["authorized_person_id"]}
                session_id={@ctx["authorized_session_id"]}
              />
            </div>
            <p class="mt-1 text-xs text-gray-600">
              Last zone: <span class="font-medium"><%= @ctx["authorized_last_zone"] || "Unknown" %></span>
              <%= if @ctx["authorized_method"] do %>
                • Auth: <span class="font-medium"><%= @ctx["authorized_method"] %></span>
              <% end %>
            </p>
          </div>
        </div>

        <%!-- Unauthorized Person --%>
        <div class="flex items-start space-x-3 p-2 bg-red-50 rounded-lg border border-red-200">
          <div class="flex-shrink-0">
            <span class="inline-flex items-center justify-center w-6 h-6 rounded-full bg-red-500 text-white text-xs font-bold">
              ✗
            </span>
          </div>
          <div class="flex-1 min-w-0">
            <div class="flex items-center space-x-2">
              <span class="px-2 py-0.5 text-xs font-medium bg-red-100 text-red-800 rounded">UNAUTHORIZED</span>
              <.person_link
                person_id={@ctx["unauthorized_person_id"]}
                session_id={@ctx["unauthorized_session_id"]}
              />
            </div>
            <p class="mt-1 text-xs text-gray-600">
              Last zone: <span class="font-medium"><%= @ctx["unauthorized_last_zone"] || "Unknown" %></span>
              <%= cond do %>
                <% @ctx["unauthorized_paid"] == true -> %>
                  • <span class="text-green-600 font-medium">Paid at <%= @ctx["unauthorized_last_pos_zone"] %></span>
                <% @ctx["unauthorized_visited_pos"] == true -> %>
                  • <span class="text-yellow-600 font-medium">Visited <%= @ctx["unauthorized_last_pos_zone"] %> (unpaid)</span>
                <% true -> %>
                  • <span class="text-red-600 font-medium">No POS visit</span>
              <% end %>
            </p>
          </div>
        </div>

        <%!-- Context Indicators --%>
        <%= if @ctx["same_group"] == true or @ctx["same_pos_zone"] == true do %>
          <div class="flex flex-wrap gap-2 pt-2 border-t border-gray-100">
            <%= if @ctx["same_group"] == true do %>
              <span class="inline-flex items-center px-2 py-1 text-xs bg-blue-100 text-blue-800 rounded">
                <svg class="w-3 h-3 mr-1" fill="currentColor" viewBox="0 0 20 20">
                  <path d="M13 6a3 3 0 11-6 0 3 3 0 016 0zM18 8a2 2 0 11-4 0 2 2 0 014 0zM14 15a4 4 0 00-8 0v3h8v-3zM6 8a2 2 0 11-4 0 2 2 0 014 0zM16 18v-3a5.972 5.972 0 00-.75-2.906A3.005 3.005 0 0119 15v3h-3zM4.75 12.094A5.973 5.973 0 004 15v3H1v-3a3 3 0 013.75-2.906z"/>
                </svg>
                Same Group (tagging issue)
              </span>
            <% end %>
            <%= if @ctx["same_pos_zone"] == true do %>
              <span class="inline-flex items-center px-2 py-1 text-xs bg-purple-100 text-purple-800 rounded">
                <svg class="w-3 h-3 mr-1" fill="currentColor" viewBox="0 0 20 20">
                  <path fill-rule="evenodd" d="M5.05 4.05a7 7 0 119.9 9.9L10 18.9l-4.95-4.95a7 7 0 010-9.9zM10 11a2 2 0 100-4 2 2 0 000 4z" clip-rule="evenodd"/>
                </svg>
                Same POS: <%= @ctx["shared_pos_zone"] %>
              </span>
            <% end %>
          </div>
        <% end %>

        <%!-- Gate Info --%>
        <div class="flex items-center justify-between pt-2 border-t border-gray-100 dark:border-gray-700 text-xs text-gray-500 dark:text-gray-400">
          <span>Gate: <%= @ctx["gate_id"] || @incident.gate_id %></span>
          <%= if @ctx["gate_open_duration_ms"] do %>
            <span>Gate open: <%= Float.round(@ctx["gate_open_duration_ms"] / 1000, 1) %>s</span>
          <% end %>
        </div>
      </div>

      <%!-- Actions --%>
      <div class="px-4 py-2 bg-gray-50 border-t border-gray-200 dark:bg-gray-700/50 dark:border-gray-700 flex justify-end space-x-2">
        <%= if @incident.status == "new" do %>
          <button
            phx-click="acknowledge"
            phx-value-id={@incident.id}
            class="px-3 py-1 bg-blue-600 text-white text-sm rounded hover:bg-blue-700"
          >
            Acknowledge
          </button>
        <% end %>
        <.link
          navigate={~p"/incidents/#{@incident.id}"}
          class="px-3 py-1 bg-gray-200 text-gray-700 text-sm rounded hover:bg-gray-300 dark:bg-gray-600 dark:text-gray-300 dark:hover:bg-gray-500"
        >
          Details
        </.link>
      </div>
    </div>
    """
  end

  # Clickable person link that opens journey modal
  defp person_link(assigns) do
    ~H"""
    <%= if @session_id do %>
      <button
        phx-click="show-journey"
        phx-value-session-id={@session_id}
        phx-value-person-id={@person_id}
        class="text-blue-600 hover:text-blue-800 font-medium text-sm underline decoration-dotted"
      >
        Person <%= @person_id %>
      </button>
    <% else %>
      <span class="font-medium text-sm text-gray-900">Person <%= @person_id %></span>
    <% end %>
    """
  end

  # Journey modal component
  defp journey_modal(assigns) do
    ~H"""
    <div class="fixed inset-0 z-50 overflow-y-auto" aria-labelledby="modal-title" role="dialog" aria-modal="true">
      <div class="flex items-end justify-center min-h-screen pt-4 px-4 pb-20 text-center sm:block sm:p-0">
        <%!-- Background overlay --%>
        <div
          class="fixed inset-0 bg-gray-500 bg-opacity-75 dark:bg-gray-900/80 transition-opacity"
          phx-click="close-journey-modal"
        ></div>

        <%!-- Modal panel --%>
        <div class="inline-block align-bottom bg-white dark:bg-gray-800 rounded-lg text-left overflow-hidden shadow-xl transform transition-all sm:my-8 sm:align-middle sm:max-w-lg sm:w-full">
          <div class="bg-white dark:bg-gray-800 px-4 pt-5 pb-4 sm:p-6 sm:pb-4">
            <div class="flex items-center justify-between mb-4">
              <h3 class="text-lg font-medium text-gray-900 dark:text-white" id="modal-title">
                Journey: Person <%= @person_id %>
              </h3>
              <button
                phx-click="close-journey-modal"
                class="text-gray-400 hover:text-gray-500 dark:text-gray-500 dark:hover:text-gray-400"
              >
                <svg class="h-6 w-6" fill="none" viewBox="0 0 24 24" stroke="currentColor">
                  <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 18L18 6M6 6l12 12" />
                </svg>
              </button>
            </div>

            <%= if @loading do %>
              <div class="flex items-center justify-center py-8">
                <svg class="animate-spin h-8 w-8 text-blue-600" xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24">
                  <circle class="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" stroke-width="4"></circle>
                  <path class="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4zm2 5.291A7.962 7.962 0 014 12H0c0 3.042 1.135 5.824 3 7.938l3-2.647z"></path>
                </svg>
                <span class="ml-2 text-gray-600 dark:text-gray-400">Loading journey...</span>
              </div>
            <% else %>
              <%= if @journey_data do %>
                <div class="space-y-3">
                  <%!-- Journey Summary --%>
                  <div class="bg-gray-50 dark:bg-gray-700/50 rounded-lg p-3">
                    <div class="grid grid-cols-2 gap-2 text-sm">
                      <div>
                        <span class="text-gray-500 dark:text-gray-400">Session:</span>
                        <span class="ml-1 font-mono text-xs dark:text-gray-300"><%= String.slice(@session_id || "", 0..7) %>...</span>
                      </div>
                      <div>
                        <span class="text-gray-500 dark:text-gray-400">State:</span>
                        <span class="ml-1 font-medium dark:text-white"><%= @journey_data["state"] || "Unknown" %></span>
                      </div>
                    </div>
                  </div>

                  <%!-- Events Timeline --%>
                  <div class="border dark:border-gray-700 rounded-lg max-h-64 overflow-y-auto">
                    <div class="px-3 py-2 bg-gray-50 dark:bg-gray-700/50 border-b dark:border-gray-700 text-xs font-medium text-gray-500 dark:text-gray-400 uppercase">
                      Events Timeline
                    </div>
                    <%= if events = @journey_data["events"] do %>
                      <div class="divide-y divide-gray-100 dark:divide-gray-700">
                        <%= for event <- events do %>
                          <div class="px-3 py-2 text-sm">
                            <div class="flex justify-between items-start">
                              <span class={["font-medium", journey_event_color(event["type"])]}>
                                <%= event["type"] %>
                              </span>
                              <span class="text-xs text-gray-400 dark:text-gray-500">
                                <%= format_journey_time(event["timestamp"]) %>
                              </span>
                            </div>
                            <%= if event["zone"] do %>
                              <div class="text-xs text-gray-500 dark:text-gray-400">Zone: <%= event["zone"] %></div>
                            <% end %>
                          </div>
                        <% end %>
                      </div>
                    <% else %>
                      <p class="p-3 text-sm text-gray-500 dark:text-gray-400">No events recorded</p>
                    <% end %>
                  </div>
                </div>
              <% else %>
                <div class="text-center py-8">
                  <p class="text-gray-500 dark:text-gray-400">No journey data available</p>
                  <p class="text-sm text-gray-400 dark:text-gray-500 mt-1">Session ID may not be available or journey has expired</p>
                </div>
              <% end %>
            <% end %>
          </div>

          <div class="bg-gray-50 dark:bg-gray-700/50 px-4 py-3 sm:px-6 sm:flex sm:flex-row-reverse">
            <button
              type="button"
              phx-click="close-journey-modal"
              class="w-full inline-flex justify-center rounded-md border border-gray-300 dark:border-gray-600 shadow-sm px-4 py-2 bg-white dark:bg-gray-700 text-base font-medium text-gray-700 dark:text-gray-200 hover:bg-gray-50 dark:hover:bg-gray-600 focus:outline-none sm:ml-3 sm:w-auto sm:text-sm"
            >
              Close
            </button>
          </div>
        </div>
      </div>
    </div>
    """
  end

  defp journey_event_color("zone.entry"), do: "text-blue-600"
  defp journey_event_color("zone.exit"), do: "text-purple-600"
  defp journey_event_color("payment.received"), do: "text-green-600"
  defp journey_event_color("line.cross"), do: "text-orange-600"
  defp journey_event_color("state.changed"), do: "text-gray-600"
  defp journey_event_color(_), do: "text-gray-600"

  defp format_journey_time(nil), do: ""
  defp format_journey_time(ts) when is_binary(ts) do
    case DateTime.from_iso8601(ts) do
      {:ok, dt, _} -> Calendar.strftime(dt, "%H:%M:%S")
      _ -> ts
    end
  end
  defp format_journey_time(_), do: ""

  defp severity_border("high"), do: "border-red-500"
  defp severity_border("medium"), do: "border-yellow-500"
  defp severity_border(_), do: "border-blue-500"

  defp severity_badge("high"), do: "bg-red-100 text-red-800"
  defp severity_badge("medium"), do: "bg-yellow-100 text-yellow-800"
  defp severity_badge(_), do: "bg-blue-100 text-blue-800"

  defp format_type(type) when is_binary(type) do
    type
    |> String.replace("_", " ")
    |> String.split(" ")
    |> Enum.map(&String.capitalize/1)
    |> Enum.join(" ")
  end

  defp format_type(_), do: "Unknown"

  defp gate_offline_banner(incidents) do
    Enum.find(incidents, fn incident ->
      incident.type == "gate_offline" and incident.status == "new"
    end)
  end

  defp banner_message(incident) do
    ctx = incident.context || %{}
    ctx["message"] || "Gate offline"
  end

  defp format_context(%{"person_id" => pid, "gate_id" => gid}) do
    "Person #{pid} at Gate #{gid}"
  end

  defp format_context(context) when is_map(context) do
    context
    |> Map.take(["person_id", "gate_id", "zone", "message"])
    |> Enum.map(fn {k, v} -> "#{k}: #{v}" end)
    |> Enum.join(", ")
  end

  defp format_context(_), do: ""

  defp format_time(nil), do: ""

  defp format_time(datetime) do
    Calendar.strftime(datetime, "%Y-%m-%d %H:%M:%S")
  end

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
