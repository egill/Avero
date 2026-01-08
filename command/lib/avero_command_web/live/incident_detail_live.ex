defmodule AveroCommandWeb.IncidentDetailLive do
  use AveroCommandWeb, :live_view

  alias AveroCommand.Incidents
  alias AveroCommand.Store

  @dashboard_uid "NETTO-GRANDI-timescale"

  @impl true
  def mount(%{"id" => id}, _session, socket) do
    case Incidents.get(id) do
      nil ->
        {:ok,
         socket
         |> put_flash(:error, "Incident not found")
         |> push_navigate(to: ~p"/")}

      incident ->
        if connected?(socket) do
          Phoenix.PubSub.subscribe(AveroCommand.PubSub, "incidents:#{id}")
        end

        # Fetch related events around the incident time
        related_events = fetch_related_events(incident)

        # Fetch person journeys for ANY incident with person_id in context
        person_journeys = fetch_person_journeys(incident)

        # For tailgating incidents: fetch gate opener and follower journeys
        {gate_opener_journey, follower_journeys} =
          if incident.type == "tailgating_detected" do
            fetch_tailgating_journeys(incident)
          else
            {[], %{}}
          end

        # Get group info if available
        group_info = get_group_info(incident)

        # Get the host from the socket's URI for Grafana links
        grafana_base_url = get_grafana_base_url(socket)

        {:ok,
         socket
         |> assign(:incident, incident)
         |> assign(:related_events, related_events)
         |> assign(:gate_opener_journey, gate_opener_journey)
         |> assign(:follower_journeys, follower_journeys)
         |> assign(:person_journeys, person_journeys)
         |> assign(:group_info, group_info)
         |> assign(:grafana_url, build_grafana_url(grafana_base_url, incident))
         |> assign(:page_title, "Incident #{String.slice(id, 0..7)}")}
    end
  end

  defp get_grafana_base_url(socket) do
    # Use configured Grafana URL or derive from host
    case Application.get_env(:avero_command, :grafana_url) do
      url when is_binary(url) and url != "" ->
        url

      _ ->
        # Derive from current host: command.example.com -> grafana.example.com
        case socket.host_uri do
          %URI{host: host, scheme: scheme} when is_binary(host) ->
            # Replace "command." prefix with "grafana." if present
            grafana_host =
              if String.starts_with?(host, "command.") do
                String.replace_prefix(host, "command.", "grafana.")
              else
                "grafana.#{host}"
              end

            "#{scheme || "https"}://#{grafana_host}"

          _ ->
            # Fallback
            "https://grafana.e18n.net"
        end
    end
  end

  defp fetch_related_events(incident) do
    # Get events 30 seconds before and after the incident
    from_time = DateTime.add(incident.created_at, -30, :second)
    to_time = DateTime.add(incident.created_at, 30, :second)

    Store.get_events_in_range(incident.site, from_time, to_time, 50)
  end

  defp fetch_tailgating_journeys(incident) do
    require Logger
    context = incident.context || %{}
    site = incident.site

    # New field names: gate_opener_id and follower_ids (array)
    gate_opener_id = context["gate_opener_id"]

    # Handle follower_ids as array, with fallback to old field name
    follower_ids = case context["follower_ids"] do
      ids when is_list(ids) -> ids
      nil -> if context["person_id"], do: [context["person_id"]], else: []
      _ -> []
    end

    Logger.info("Tailgating journey lookup: site=#{site}, gate_opener=#{inspect(gate_opener_id)}, followers=#{inspect(follower_ids)}")

    gate_opener_journey =
      if gate_opener_id do
        events = Store.events_for_person_extended(site, gate_opener_id, 100)
        Logger.info("Gate opener #{gate_opener_id}: found #{length(events)} events")
        Enum.reverse(events)
      else
        Logger.warning("Tailgating incident missing gate_opener_id")
        []
      end

    # Fetch journey for each follower
    follower_journeys =
      follower_ids
      |> Enum.map(fn id ->
        events = Store.events_for_person_extended(site, id, 100)
        Logger.info("Follower #{id}: found #{length(events)} events")
        {id, Enum.reverse(events)}
      end)
      |> Map.new()

    {gate_opener_journey, follower_journeys}
  end

  # Fetch journeys for ALL person_ids found in incident context
  defp fetch_person_journeys(incident) do
    require Logger
    context = incident.context || %{}
    site = incident.site

    # Extract all person_id related fields from context
    person_ids = extract_person_ids(context)

    Logger.info("Person journey lookup for incident #{incident.id}: found #{length(person_ids)} person(s)")

    Enum.map(person_ids, fn {label, person_id} ->
      events = Store.events_for_person_extended(site, person_id, 100)
      Logger.info("#{label} (#{person_id}): found #{length(events)} events")
      {label, person_id, Enum.reverse(events)}
    end)
  end

  defp extract_person_ids(context) do
    []
    |> maybe_add_person(context, "authorized_person_id", "Authorized")
    |> maybe_add_person(context, "unauthorized_person_id", "Unauthorized")
    |> maybe_add_person(context, "person_id", "Person")
    |> maybe_add_person(context, "triggering_person_id", "Triggering")
    |> maybe_add_person(context, "related_person_id", "Related")
  end

  defp maybe_add_person(list, context, key, label) do
    case context[key] do
      nil -> list
      "" -> list
      id -> [{label, id} | list]
    end
  end

  defp get_group_info(incident) do
    context = incident.context || %{}

    case context do
      %{"group_id" => group_id} when not is_nil(group_id) and group_id != "" ->
        %{
          group_id: group_id,
          same_group: context["same_group"] || false,
          group_size: context["group_size"]
        }

      _ ->
        nil
    end
  end

  defp build_grafana_url(base_url, incident) do
    # Create time range: 2 minutes before to 1 minute after
    from_ms = DateTime.add(incident.created_at, -120, :second) |> DateTime.to_unix(:millisecond)
    to_ms = DateTime.add(incident.created_at, 60, :second) |> DateTime.to_unix(:millisecond)

    "#{base_url}/d/#{@dashboard_uid}?from=#{from_ms}&to=#{to_ms}&var-site=#{incident.site}"
  end

  @impl true
  def handle_info({:incident_updated, incident}, socket) do
    {:noreply, assign(socket, :incident, incident)}
  end

  @impl true
  def handle_event("acknowledge", _params, socket) do
    case Incidents.acknowledge(socket.assigns.incident.id) do
      {:ok, incident} ->
        {:noreply,
         socket
         |> assign(:incident, incident)
         |> put_flash(:info, "Incident acknowledged")}

      {:error, _} ->
        {:noreply, put_flash(socket, :error, "Failed to acknowledge")}
    end
  end

  @impl true
  def handle_event("resolve", %{"resolution" => resolution}, socket) do
    case Incidents.resolve(socket.assigns.incident.id, resolution) do
      {:ok, incident} ->
        {:noreply,
         socket
         |> assign(:incident, incident)
         |> put_flash(:info, "Incident resolved")}

      {:error, _} ->
        {:noreply, put_flash(socket, :error, "Failed to resolve")}
    end
  end

  @impl true
  def render(assigns) do
    ~H"""
    <div class="max-w-6xl mx-auto">
      <.back navigate={~p"/"}>Back to Incidents</.back>

      <div class="mt-6 grid grid-cols-3 gap-6">
        <%!-- Main incident details --%>
        <div class="col-span-2 bg-white shadow rounded-lg overflow-hidden">
          <div class="px-6 py-4 border-b border-gray-200">
            <div class="flex items-center justify-between">
              <div class="flex items-center space-x-3">
                <span class={["px-3 py-1 text-sm font-medium rounded", severity_badge(@incident.severity)]}>
                  <%= String.upcase(@incident.severity) %>
                </span>
                <h1 class="text-xl font-semibold text-gray-900">
                  <%= format_type(@incident.type) %>
                </h1>
              </div>
              <span class={["px-3 py-1 text-sm rounded", status_badge(@incident.status)]}>
                <%= String.capitalize(@incident.status) %>
              </span>
            </div>
          </div>

          <div class="px-6 py-4">
            <dl class="grid grid-cols-2 gap-4">
              <div>
                <dt class="text-sm font-medium text-gray-500">Site</dt>
                <dd class="mt-1 text-sm text-gray-900"><%= @incident.site %></dd>
              </div>
              <div>
                <dt class="text-sm font-medium text-gray-500">Gate</dt>
                <dd class="mt-1 text-sm text-gray-900"><%= @incident.gate_id || "N/A" %></dd>
              </div>
              <div>
                <dt class="text-sm font-medium text-gray-500">Created</dt>
                <dd class="mt-1 text-sm text-gray-900"><%= format_time(@incident.created_at) %></dd>
              </div>
              <div>
                <dt class="text-sm font-medium text-gray-500">Category</dt>
                <dd class="mt-1 text-sm text-gray-900"><%= format_category(@incident.category) %></dd>
              </div>
            </dl>
          </div>

          <%!-- Group info badge (if applicable) --%>
          <%= if @group_info do %>
            <div class="px-6 py-2 border-t border-gray-200">
              <div class="inline-flex items-center px-3 py-1 bg-purple-100 text-purple-800 text-sm rounded-full">
                <svg class="w-4 h-4 mr-2" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                  <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M17 20h5v-2a3 3 0 00-5.356-1.857M17 20H7m10 0v-2c0-.656-.126-1.283-.356-1.857M7 20H2v-2a3 3 0 015.356-1.857M7 20v-2c0-.656.126-1.283.356-1.857m0 0a5.002 5.002 0 019.288 0M15 7a3 3 0 11-6 0 3 3 0 016 0zm6 3a2 2 0 11-4 0 2 2 0 014 0zM7 10a2 2 0 11-4 0 2 2 0 014 0z"/>
                </svg>
                Part of Group <%= @group_info.group_id %>
                <%= if @group_info.group_size do %>
                  (<%= @group_info.group_size %> people)
                <% end %>
                <%= if @group_info.same_group do %>
                  - Same group as related person
                <% end %>
              </div>
            </div>
          <% end %>

          <%!-- Context details (only for non-tailgating incidents) --%>
          <%= if @incident.type != "tailgating_detected" do %>
            <div class="px-6 py-4 border-t border-gray-200">
              <h3 class="text-sm font-medium text-gray-500 mb-3">Incident Details</h3>
              <dl class="grid grid-cols-2 gap-3">
                <%= for {key, value} <- format_context_items(@incident.context) do %>
                  <div class="bg-gray-50 p-3 rounded">
                    <dt class="text-xs font-medium text-gray-500 uppercase"><%= key %></dt>
                    <dd class="mt-1 text-sm font-medium text-gray-900"><%= value %></dd>
                  </div>
                <% end %>
              </dl>
            </div>

            <%!-- Person journey for non-tailgating incidents --%>
            <%= if length(@person_journeys) > 0 do %>
              <div class="px-6 py-4 border-t border-gray-200 bg-gray-50">
                <h3 class="text-sm font-medium text-gray-700 mb-3">Person Journey</h3>
                <div class="grid grid-cols-1 gap-4">
                  <%= for {label, person_id, events} <- @person_journeys do %>
                    <.person_journey
                      title={"#{label} - Person #{person_id}"}
                      subtitle={"#{length(events)} events"}
                      events={events}
                      highlight_color="blue"
                    />
                  <% end %>
                </div>
              </div>
            <% end %>
          <% end %>

          <%!-- Tailgating-specific details --%>
          <%= if @incident.type == "tailgating_detected" do %>
            <.tailgating_detail
              incident={@incident}
              gate_opener_journey={@gate_opener_journey}
              follower_journeys={@follower_journeys}
            />
          <% end %>

          <%!-- Investigation Links --%>
          <div class="px-6 py-4 border-t border-gray-200 bg-blue-50">
            <h3 class="text-sm font-medium text-gray-700 mb-3">Investigation</h3>
            <div class="flex space-x-3">
              <a
                href={@grafana_url}
                target="_blank"
                class="inline-flex items-center px-4 py-2 bg-orange-500 text-white text-sm rounded hover:bg-orange-600"
              >
                <svg class="w-4 h-4 mr-2" fill="currentColor" viewBox="0 0 24 24">
                  <path d="M12 2C6.48 2 2 6.48 2 12s4.48 10 10 10 10-4.48 10-10S17.52 2 12 2zm-1 17.93c-3.95-.49-7-3.85-7-7.93 0-.62.08-1.21.21-1.79L9 15v1c0 1.1.9 2 2 2v1.93zm6.9-2.54c-.26-.81-1-1.39-1.9-1.39h-1v-3c0-.55-.45-1-1-1H8v-2h2c.55 0 1-.45 1-1V7h2c1.1 0 2-.9 2-2v-.41c2.93 1.19 5 4.06 5 7.41 0 2.08-.8 3.97-2.1 5.39z"/>
                </svg>
                Open in Grafana
              </a>
              <a
                href={@grafana_url <> "&viewPanel=15"}
                target="_blank"
                class="inline-flex items-center px-4 py-2 bg-purple-500 text-white text-sm rounded hover:bg-purple-600"
              >
                <svg class="w-4 h-4 mr-2" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                  <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M9 19v-6a2 2 0 00-2-2H5a2 2 0 00-2 2v6a2 2 0 002 2h2a2 2 0 002-2zm0 0V9a2 2 0 012-2h2a2 2 0 012 2v10m-6 0a2 2 0 002 2h2a2 2 0 002-2m0 0V5a2 2 0 012-2h2a2 2 0 012 2v14a2 2 0 01-2 2h-2a2 2 0 01-2-2z"/>
                </svg>
                Gate State History
              </a>
            </div>
          </div>

          <%!-- Actions --%>
          <div class="px-6 py-4 border-t border-gray-200">
            <h3 class="text-sm font-medium text-gray-500 mb-2">Actions</h3>
            <div class="flex space-x-3">
              <%= if @incident.status == "new" do %>
                <button
                  phx-click="acknowledge"
                  class="px-4 py-2 bg-blue-600 text-white text-sm rounded hover:bg-blue-700"
                >
                  Acknowledge
                </button>
              <% end %>
              <%= if @incident.status in ["new", "acknowledged", "in_progress"] do %>
                <button
                  phx-click="resolve"
                  phx-value-resolution="resolved"
                  class="px-4 py-2 bg-green-600 text-white text-sm rounded hover:bg-green-700"
                >
                  Resolve
                </button>
                <button
                  phx-click="resolve"
                  phx-value-resolution="dismissed"
                  class="px-4 py-2 bg-gray-600 text-white text-sm rounded hover:bg-gray-700"
                >
                  Dismiss
                </button>
              <% end %>
            </div>
          </div>
        </div>

        <%!-- Related Events Timeline --%>
        <div class="col-span-1 bg-white shadow rounded-lg overflow-hidden">
          <div class="px-4 py-3 border-b border-gray-200 bg-gray-50">
            <h3 class="text-sm font-medium text-gray-700">Related Events</h3>
            <p class="text-xs text-gray-500">±30 seconds around incident</p>
          </div>
          <div class="divide-y divide-gray-100 max-h-[600px] overflow-y-auto">
            <%= if Enum.empty?(@related_events) do %>
              <p class="p-4 text-sm text-gray-500">No related events found</p>
            <% else %>
              <%= for event <- @related_events do %>
                <div class={[
                  "px-4 py-2 text-xs",
                  event_highlight(event, @incident)
                ]}>
                  <div class="flex justify-between items-start">
                    <span class={["font-medium", event_type_color(event.event_type)]}>
                      <%= event.data["type"] || event.event_type %>
                    </span>
                    <span class="text-gray-400">
                      <%= format_event_time(event.time, @incident.created_at) %>
                    </span>
                  </div>
                  <%= if event.person_id do %>
                    <div class="text-gray-500">Person: <%= event.person_id %></div>
                  <% end %>
                  <%= if event.data["open_duration_ms"] do %>
                    <div class="text-gray-500">Duration: <%= event.data["open_duration_ms"] %>ms</div>
                  <% end %>
                </div>
              <% end %>
            <% end %>
          </div>
        </div>
      </div>
    </div>
    """
  end

  # Tailgating-specific components
  defp tailgating_detail(assigns) do
    context = assigns.incident.context || %{}

    # Extract gate opener (single person who triggered the open)
    gate_opener_id = context["gate_opener_id"]

    # Extract followers (array of tailgaters) - handle both old and new format
    follower_ids = case context["follower_ids"] do
      ids when is_list(ids) -> ids
      nil -> if context["person_id"], do: [context["person_id"]], else: []
      _ -> []
    end

    assigns =
      assigns
      |> assign(:context, context)
      |> assign(:gate_opener_id, gate_opener_id)
      |> assign(:gate_opener_method, context["gate_opener_method"])
      |> assign(:follower_ids, follower_ids)
      |> assign(:follower_paid, context["follower_paid"])
      |> assign(:follower_visited_pos, context["follower_visited_pos"])
      |> assign(:same_group, context["same_group"])
      |> assign(:group_id, context["group_id"])
      |> assign(:gate_duration, context["gate_open_duration_ms"])

    ~H"""
    <div class="px-6 py-4 border-t border-gray-200 bg-gray-50">
      <h3 class="text-sm font-medium text-gray-700 mb-4">Persons Involved</h3>

      <div class="grid grid-cols-2 gap-4 mb-6">
        <%!-- Gate Opener (authorized person who triggered the open) --%>
        <div class="bg-white border-2 border-green-200 rounded-lg p-4">
          <div class="flex items-center justify-between mb-3">
            <span class="text-xs font-semibold uppercase text-green-700">Gate Opener</span>
            <span class="px-2 py-1 text-xs font-medium bg-green-100 text-green-800 rounded">
              ✓ Authorized
            </span>
          </div>
          <div class="space-y-2">
            <div>
              <span class="text-xs text-gray-500">Person ID</span>
              <p class="text-lg font-bold text-gray-900"><%= @gate_opener_id || "Unknown" %></p>
            </div>
            <div>
              <span class="text-xs text-gray-500">Auth Method</span>
              <p class="text-sm text-gray-700"><%= @gate_opener_method || "N/A" %></p>
            </div>
            <%= if @gate_duration do %>
              <div>
                <span class="text-xs text-gray-500">Gate Open Duration</span>
                <p class="text-sm text-gray-700"><%= Float.round(@gate_duration / 1000, 1) %>s</p>
              </div>
            <% end %>
          </div>
        </div>

        <%!-- Followers (tailgaters - can be multiple) --%>
        <div class="bg-white border-2 border-red-200 rounded-lg p-4">
          <div class="flex items-center justify-between mb-3">
            <span class="text-xs font-semibold uppercase text-red-700">
              Tailgater<%= if length(@follower_ids) > 1, do: "s (#{length(@follower_ids)})", else: "" %>
            </span>
            <span class="px-2 py-1 text-xs font-medium bg-red-100 text-red-800 rounded">
              ✗ Unauthorized
            </span>
          </div>
          <div class="space-y-2">
            <div>
              <span class="text-xs text-gray-500">Person ID<%= if length(@follower_ids) > 1, do: "s", else: "" %></span>
              <p class="text-lg font-bold text-gray-900">
                <%= if @follower_ids == [], do: "Unknown", else: Enum.join(@follower_ids, ", ") %>
              </p>
            </div>
            <div>
              <span class="text-xs text-gray-500">Visited POS?</span>
              <p class="text-sm text-gray-700"><%= if @follower_visited_pos, do: "Yes", else: "No" %></p>
            </div>
            <div>
              <span class="text-xs text-gray-500">Made Payment?</span>
              <p class={["text-sm", if(@follower_paid, do: "text-green-600", else: "text-red-600")]}>
                <%= if @follower_paid, do: "Yes", else: "No" %>
              </p>
            </div>
            <%= if @same_group do %>
              <div class="mt-2 px-2 py-1 bg-yellow-100 text-yellow-800 text-xs rounded">
                ⚠ Same group as gate opener<%= if @group_id, do: " (Group #{@group_id})", else: "" %>
              </div>
            <% end %>
          </div>
        </div>
      </div>

      <%!-- Person Journeys --%>
      <h3 class="text-sm font-medium text-gray-700 mb-3">Person Journeys</h3>
      <div class="grid grid-cols-2 gap-4">
        <.person_journey
          title={"Person #{@gate_opener_id || "Unknown"}"}
          subtitle="Gate Opener"
          events={@gate_opener_journey}
          highlight_color="green"
        />
        <%= for follower_id <- @follower_ids do %>
          <.person_journey
            title={"Person #{follower_id}"}
            subtitle="Tailgater"
            events={Map.get(@follower_journeys, follower_id, [])}
            highlight_color="red"
          />
        <% end %>
        <%= if @follower_ids == [] do %>
          <.person_journey
            title="Person Unknown"
            subtitle="Tailgater"
            events={[]}
            highlight_color="red"
          />
        <% end %>
      </div>
    </div>
    """
  end

  defp person_journey(assigns) do
    # Filter out events where format_journey_event returns nil (e.g., heartbeats)
    filtered_events = Enum.filter(assigns.events, fn event ->
      format_journey_event(event) != nil
    end)

    assigns = assign(assigns, :filtered_events, filtered_events)

    ~H"""
    <div class="bg-white rounded-lg border border-gray-200 overflow-hidden">
      <div class={[
        "px-3 py-2 border-b",
        case @highlight_color do
          "green" -> "bg-green-50"
          "red" -> "bg-red-50"
          "blue" -> "bg-blue-50"
          _ -> "bg-gray-50"
        end
      ]}>
        <h4 class="text-sm font-medium text-gray-900"><%= @title %></h4>
        <p class="text-xs text-gray-500"><%= @subtitle %></p>
      </div>
      <div class="max-h-64 overflow-y-auto divide-y divide-gray-100">
        <%= if Enum.empty?(@filtered_events) do %>
          <p class="p-3 text-sm text-gray-500">No journey data available</p>
        <% else %>
          <%= for event <- @filtered_events do %>
            <div class="px-3 py-2 text-xs">
              <div class="flex justify-between">
                <span class={journey_event_class(event)}>
                  <%= format_journey_event(event) %>
                </span>
                <span class="text-gray-400">
                  <%= Calendar.strftime(event.time, "%H:%M:%S") %>
                </span>
              </div>
              <%= if event.zone do %>
                <span class="text-gray-500">Zone: <%= event.zone %></span>
              <% end %>
            </div>
          <% end %>
        <% end %>
      </div>
    </div>
    """
  end

  defp journey_event_class(event) do
    type = (event.data || %{})["type"] || event.event_type

    case type do
      "exit.confirmed" ->
        if event.authorized, do: "text-green-600 font-medium", else: "text-red-600 font-medium"

      "payment.received" ->
        "text-blue-600 font-medium"

      "person.state.changed" ->
        "text-orange-600"

      "gate.opened" ->
        "text-green-600 font-medium"

      "gate.closed" ->
        "text-gray-600 font-medium"

      "gate.status_changed" ->
        "text-purple-600"

      "journey.started" ->
        "text-indigo-600"

      "journey" ->
        "text-indigo-600 font-medium"

      "exit.line_crossed" ->
        "text-green-600"

      _ ->
        "text-gray-600"
    end
  end

  defp format_journey_event(event) do
    type = (event.data || %{})["type"] || event.event_type
    data = event.data || %{}

    case type do
      "person.state.changed" ->
        to_state = data["to_state"] || "unknown"
        "State → #{String.capitalize(to_state)}"

      "payment.received" ->
        "💳 Payment received"

      "exit.confirmed" ->
        if event.authorized, do: "✓ Exit Authorized", else: "✗ Exit Unauthorized"

      "gate.opened" ->
        reason = data["reason"] || "unknown"
        person = data["person_id"]
        reason_emoji = case reason do
          "payment" -> "💳"
          "sensor_triggered" -> "🔔"
          _ -> "🚪"
        end
        if person, do: "#{reason_emoji} Gate opened (#{reason}) for ##{person}", else: "#{reason_emoji} Gate opened (#{reason})"

      "gate.closed" ->
        duration = data["open_duration_ms"] || data["duration_ms"]
        if duration do
          secs = Float.round(duration / 1000, 1)
          "🚪 Gate closed (#{secs}s cycle)"
        else
          "🚪 Gate closed"
        end

      "gate.status_changed" ->
        from = data["from_status"] || "?"
        to = data["to_status"] || "?"
        "⚙️ #{from} → #{to}"

      "gate.status.heartbeat" ->
        # Skip heartbeats in display - too verbose
        nil

      "journey.started" ->
        "🚶 Journey started"

      "journey" ->
        duration = data["duration_ms"] || data["total_dwell_ms"]
        if duration do
          secs = Float.round(duration / 1000, 1)
          "🏁 Journey completed (#{secs}s)"
        else
          "🏁 Journey completed"
        end

      "exit.line_crossed" ->
        person = data["person_id"]
        if person, do: "↗️ Exit line crossed ##{person}", else: "↗️ Exit line crossed"

      "xovis.zone.entry" ->
        zone = data["zone"] || event.zone || "unknown"
        "→ Entered #{zone}"

      "xovis.zone.exit" ->
        zone = data["zone"] || event.zone || "unknown"
        dwell = data["dwell_ms"]

        if dwell do
          secs = Float.round(dwell / 1000, 1)
          "← Exited #{zone} (#{secs}s dwell)"
        else
          "← Exited #{zone}"
        end

      "xovis.line.cross" ->
        line = data["line"] || "unknown"
        direction = data["direction"] || "forward"
        "Line cross: #{line} (#{direction})"

      _ ->
        type
    end
  end

  defp format_context_items(context) when is_map(context) do
    context
    |> Enum.map(fn {k, v} ->
      formatted_key = k |> to_string() |> String.replace("_", " ") |> String.capitalize()
      formatted_value = format_context_value(v)
      {formatted_key, formatted_value}
    end)
    |> Enum.reject(fn {_k, v} -> v == "" or is_nil(v) end)
  end

  defp format_context_items(_), do: []

  defp format_context_value(v) when is_float(v), do: Float.round(v, 2)
  defp format_context_value(v) when is_integer(v), do: v
  defp format_context_value(v) when is_binary(v), do: v
  defp format_context_value(v) when is_map(v), do: Jason.encode!(v)
  defp format_context_value(_), do: ""

  defp event_highlight(event, incident) do
    # Highlight the triggering event
    if event.data["type"] == "gate.closed" and
       abs(DateTime.diff(event.time, incident.created_at, :second)) < 2 do
      "bg-yellow-50 border-l-2 border-yellow-400"
    else
      ""
    end
  end

  defp event_type_color("gates"), do: "text-purple-600"
  defp event_type_color("exits"), do: "text-green-600"
  defp event_type_color("payments"), do: "text-blue-600"
  defp event_type_color("people"), do: "text-orange-600"
  defp event_type_color(_), do: "text-gray-600"

  defp format_event_time(event_time, incident_time) do
    diff = DateTime.diff(event_time, incident_time, :millisecond)
    sign = if diff >= 0, do: "+", else: ""
    "#{sign}#{div(diff, 1000)}.#{rem(abs(diff), 1000) |> Integer.to_string() |> String.pad_leading(3, "0")}s"
  end

  defp severity_badge("high"), do: "bg-red-100 text-red-800"
  defp severity_badge("medium"), do: "bg-yellow-100 text-yellow-800"
  defp severity_badge(_), do: "bg-blue-100 text-blue-800"

  defp status_badge("new"), do: "bg-yellow-100 text-yellow-800"
  defp status_badge("acknowledged"), do: "bg-blue-100 text-blue-800"
  defp status_badge("resolved"), do: "bg-green-100 text-green-800"
  defp status_badge("dismissed"), do: "bg-gray-100 text-gray-800"
  defp status_badge(_), do: "bg-gray-100 text-gray-800"

  defp format_type(type) when is_binary(type) do
    type
    |> String.replace("_", " ")
    |> String.split(" ")
    |> Enum.map(&String.capitalize/1)
    |> Enum.join(" ")
  end

  defp format_type(_), do: "Unknown"

  defp format_category(cat) when is_binary(cat) do
    cat
    |> String.replace("_", " ")
    |> String.split(" ")
    |> Enum.map(&String.capitalize/1)
    |> Enum.join(" ")
  end

  defp format_category(_), do: "Unknown"

  defp format_time(nil), do: "N/A"

  defp format_time(datetime) do
    Calendar.strftime(datetime, "%Y-%m-%d %H:%M:%S UTC")
  end
end
