defmodule AveroCommandWeb.PosTestLive do
  @moduledoc """
  Simple POS test page with a single button and clear visual feedback.
  """
  use AveroCommandWeb, :live_view

  require Logger

  @pos_zone "POS_1"
  @min_dwell_ms 7000
  @tick_ms 1000

  @impl true
  def mount(_params, session, socket) do
    site = socket.assigns[:selected_site] || session["selected_site"] || "avero"

    if connected?(socket) do
      Phoenix.PubSub.subscribe(AveroCommand.PubSub, "gateway:events")
      Phoenix.PubSub.subscribe(AveroCommand.PubSub, "acc_events")
      Process.send_after(self(), :tick, @tick_ms)
    end

    {:ok,
     assign(socket,
       page_title: "POS Test",
       site: site,
       pos_zone: @pos_zone,
       min_dwell_ms: @min_dwell_ms,
       zone_count: 0,
       occupied_since: nil,
       last_zone_event_at: nil,
       loading: false,
       request_status: :idle,
       request_message: nil,
       request_details: nil,
       request_zone_count: nil,
       paid_count: nil,
       acc_status: :idle,
       acc_message: nil,
       acc_details: nil,
       acc_updated_at: nil,
       now: DateTime.utc_now()
     )}
  end

  @impl true
  def render(assigns) do
    ~H"""
    <% zone_present = @zone_count > 0 %>
    <% current_dwell_ms = current_dwell_ms(@now, @occupied_since) %>
    <% dwell_met = zone_present && current_dwell_ms >= @min_dwell_ms %>
    <% dwell_progress = dwell_progress(current_dwell_ms, @min_dwell_ms) %>
    <div class="min-h-screen bg-gray-900 flex items-center justify-center p-4">
      <div class="w-full max-w-3xl space-y-6">
        <div class="text-center space-y-1">
          <h1 class="text-2xl font-bold text-white">POS Test</h1>
          <div class="text-sm text-gray-400">
            Site: <span class="text-gray-200"><%= @site %></span>
          </div>
        </div>

        <div class="grid gap-6 md:grid-cols-2">
          <div class="rounded-2xl border border-gray-800 bg-gray-900/40 p-6">
            <div class="flex items-start justify-between">
              <div>
                <div class="text-xs uppercase tracking-wide text-gray-500">Zone <%= @pos_zone %></div>
                <div class="text-2xl font-semibold text-white">
                  <%= if zone_present, do: "Occupied", else: "Empty" %>
                </div>
              </div>
              <div class={[
                "mt-1 h-4 w-4 rounded-full",
                zone_present && "bg-emerald-400 ring-2 ring-emerald-300/60",
                !zone_present && "bg-gray-600"
              ]} />
            </div>

            <div class="mt-4 space-y-2 text-sm text-gray-400">
              <div>
                People in zone:
                <span class="text-gray-200 font-medium"><%= @zone_count %></span>
              </div>
              <div>
                Current dwell:
                <span class="text-gray-200 font-medium"><%= format_duration(current_dwell_ms) %></span>
              </div>
            </div>

            <div class="mt-4">
              <div class="flex items-center justify-between text-xs text-gray-500">
                <span>Min dwell <%= format_duration(@min_dwell_ms) %></span>
                <span class={[
                  "font-semibold",
                  dwell_met && "text-emerald-300",
                  !dwell_met && "text-amber-300"
                ]}>
                  <%= if dwell_met, do: "met", else: "not met" %>
                </span>
              </div>
              <div class="mt-2 h-2 w-full rounded-full bg-gray-800 overflow-hidden">
                <div
                  class={[
                    "h-full transition-all duration-500",
                    dwell_met && "bg-emerald-500",
                    !dwell_met && "bg-amber-500"
                  ]}
                  style={"width: #{dwell_progress}%"}
                />
              </div>
            </div>
          </div>

          <div class="rounded-2xl border border-gray-800 bg-gray-900/40 p-6 space-y-5">
            <button
              phx-click="simulate_acc"
              disabled={@loading || !zone_present}
              class={[
                "w-full py-6 text-xl font-bold rounded-2xl transition-all duration-200",
                "focus:outline-none focus:ring-4 focus:ring-offset-2 focus:ring-offset-gray-900",
                button_state_class(@loading, zone_present)
              ]}
            >
              <%= if @loading do %>
                Sending...
              <% else %>
                Pay <%= @pos_zone %>
              <% end %>
            </button>

            <div class="space-y-2 text-sm text-gray-400">
              <div class="flex items-center justify-between">
                <span>Request</span>
                <span class={[
                  "px-2 py-1 rounded-full text-xs font-semibold",
                  request_badge_class(@request_status)
                ]}>
                  <%= request_label(@request_status) %>
                </span>
              </div>
              <%= if @request_message do %>
                <div class="text-xs text-gray-500"><%= @request_message %></div>
              <% end %>
              <%= if @request_details do %>
                <div class="text-xs text-gray-600"><%= @request_details %></div>
              <% end %>
              <%= if @request_zone_count != nil do %>
                <div class="text-xs text-gray-500">
                  Mark paid:
                  <span class="text-gray-200 font-semibold"><%= @request_zone_count %></span>
                </div>
              <% end %>
            </div>

            <div class={["rounded-xl border p-4 space-y-2", acc_panel_class(@acc_status)]}>
              <div class="flex items-start gap-3">
                <div class={["text-2xl font-bold", acc_icon_class(@acc_status)]}>
                  <%= acc_icon(@acc_status) %>
                </div>
                <div>
                  <div class="text-base font-semibold text-white">
                    <%= acc_status_label(@acc_status) %>
                  </div>
                  <%= if @acc_message do %>
                    <div class="text-sm text-gray-300"><%= @acc_message %></div>
                  <% end %>
                  <%= if @acc_details do %>
                    <div class="text-xs text-gray-400"><%= @acc_details %></div>
                  <% end %>
                  <%= if @paid_count != nil do %>
                    <div class="text-xs text-gray-400">
                      Paid count: <span class="text-gray-200 font-semibold"><%= @paid_count %></span>
                    </div>
                  <% end %>
                </div>
              </div>
              <%= if @acc_updated_at do %>
                <div class="text-xs text-gray-500">
                  Updated <%= format_time(@acc_updated_at) %>
                </div>
              <% end %>
            </div>
          </div>
        </div>
      </div>
    </div>
    """
  end

  @impl true
  def handle_event("simulate_acc", _params, socket) do
    send(self(), {:do_simulate, socket.assigns.site})

    {:noreply,
     assign(socket,
       loading: true,
       request_status: :sending,
       request_message: "Sending ACC request to gateway",
       request_details: nil,
       request_zone_count: socket.assigns.zone_count,
       paid_count: nil,
       acc_status: :pending,
       acc_message: "Waiting for ACC feedback",
       acc_details: nil,
       acc_updated_at: DateTime.utc_now()
     )}
  end

  @impl true
  def handle_info({:do_simulate, site}, socket) do
    result = call_gateway_acc(site, socket.assigns.pos_zone)

    {:noreply,
     assign(socket,
       loading: false,
       request_status: result.status,
       request_message: result.message,
       request_details: result.details
     )}
  end

  @impl true
  def handle_info(:tick, socket) do
    Process.send_after(self(), :tick, @tick_ms)
    {:noreply, assign(socket, :now, DateTime.utc_now())}
  end

  @impl true
  def handle_info({:zone_event, %{site: event_site, zone_id: zone_id, event_type: type}}, socket) do
    if event_site == socket.assigns.site and zone_id == socket.assigns.pos_zone do
      {:noreply, handle_zone_event(socket, type)}
    else
      {:noreply, socket}
    end
  end

  @impl true
  def handle_info({:acc_event, event}, socket) do
    pos = event.pos && to_string(event.pos)

    if event.site == socket.assigns.site and pos == socket.assigns.pos_zone do
      {:noreply, handle_acc_event(socket, event)}
    else
      {:noreply, socket}
    end
  end

  @impl true
  def handle_info(_msg, socket), do: {:noreply, socket}

  # Zone event handlers

  defp handle_zone_event(socket, :zone_entry) do
    now = DateTime.utc_now()
    new_count = socket.assigns.zone_count + 1
    was_empty = socket.assigns.zone_count == 0

    socket
    |> assign(
      zone_count: new_count,
      occupied_since: socket.assigns.occupied_since || now,
      last_zone_event_at: now
    )
    |> maybe_reset_status(was_empty)
  end

  defp handle_zone_event(socket, :zone_exit) do
    now = DateTime.utc_now()
    new_count = max(socket.assigns.zone_count - 1, 0)
    occupied_since = if new_count == 0, do: nil, else: socket.assigns.occupied_since

    assign(socket,
      zone_count: new_count,
      occupied_since: occupied_since,
      last_zone_event_at: now
    )
  end

  defp handle_zone_event(socket, :payment) do
    assign(socket,
      acc_status: :matched,
      acc_message: "Payment confirmed",
      acc_details: nil,
      paid_count: 1,
      acc_updated_at: DateTime.utc_now()
    )
  end

  defp handle_zone_event(socket, _other), do: socket

  defp handle_acc_event(socket, event) do
    now = event.time || DateTime.utc_now()
    {status, message, details} = acc_status_from_event(event, socket.assigns.pos_zone)
    paid_count = paid_count_from_event(event)

    base_assigns = [
      acc_status: status,
      acc_message: message,
      acc_details: details,
      acc_updated_at: now
    ]

    assigns =
      case paid_count do
        nil -> base_assigns
        count -> Keyword.put(base_assigns, :paid_count, count)
      end

    assign(socket, assigns)
  end

  # Gateway API

  defp call_gateway_acc(site, pos) do
    gateway_url = AveroCommand.Sites.gateway_url(site, "/acc/simulate?pos=#{pos}")

    if gateway_url do
      :inets.start()
      :ssl.start()

      case :httpc.request(
             :post,
             {String.to_charlist(gateway_url), [], ~c"application/json", ~c""},
             [{:timeout, 5000}],
             []
           ) do
        {:ok, {{_, status, _}, _, body}} when status in [200, 201] ->
          message =
            case Jason.decode(to_string(body)) do
              {:ok, %{"message" => msg}} when is_binary(msg) -> msg
              _ -> "ACC request sent"
            end

          %{status: :sent, message: message, details: nil}

        {:ok, {{_, status, _}, _, _body}} ->
          %{status: :error, message: "Gateway error", details: "Status #{status}"}

        {:error, reason} ->
          %{status: :error, message: "Connection failed", details: inspect(reason)}
      end
    else
      %{status: :error, message: "Unknown site", details: site}
    end
  end

  defp maybe_reset_status(socket, true) do
    assign(socket,
      request_status: :idle,
      request_message: nil,
      request_details: nil,
      request_zone_count: nil,
      paid_count: nil,
      acc_status: :idle,
      acc_message: nil,
      acc_details: nil,
      acc_updated_at: nil
    )
  end

  defp maybe_reset_status(socket, _false), do: socket

  defp acc_status_from_event(event, pos_zone) do
    details = build_acc_details(event)

    case event.type do
      "matched" -> {:matched, "Payment authorized", details}
      "matched_no_journey" -> {:matched, "Payment matched (no journey)", details}
      "unmatched" -> {:unmatched, "ACC failed to match", details || "No person in #{pos_zone}"}
      "late_after_gate" -> {:late, "ACC late after gate", details}
      "received" -> {:pending, "ACC received", details}
      _ -> {:pending, "ACC update received", details}
    end
  end

  defp build_acc_details(event) do
    [
      event.tid && "Track #{event.tid}",
      event.dwell_ms && "Dwell #{format_duration(event.dwell_ms)}"
    ]
    |> Enum.reject(&is_nil/1)
    |> case do
      [] -> nil
      parts -> Enum.join(parts, " · ")
    end
  end

  defp paid_count_from_event(event) do
    case event.type do
      type when type in ["matched", "matched_no_journey", "late_after_gate"] -> 1
      "unmatched" -> 0
      _ -> nil
    end
  end

  defp current_dwell_ms(_now, nil), do: 0

  defp current_dwell_ms(now, occupied_since) do
    max(DateTime.diff(now, occupied_since, :millisecond), 0)
  end

  defp dwell_progress(ms, min_dwell_ms)
       when is_integer(ms) and is_integer(min_dwell_ms) and min_dwell_ms > 0 and ms > 0 do
    min(100, round(ms * 100 / min_dwell_ms))
  end

  defp dwell_progress(_ms, _min_dwell_ms), do: 0

  defp format_duration(ms) when is_integer(ms) do
    seconds = ms / 1000
    "#{:erlang.float_to_binary(seconds, decimals: 1)}s"
  end

  defp format_duration(_), do: "-"

  defp format_time(%DateTime{} = time) do
    Calendar.strftime(time, "%H:%M:%S")
  end

  defp format_time(_), do: "-"

  # Button state classes - extracted to avoid nested ternary
  defp button_state_class(true, _zone_present) do
    "bg-gray-600 text-gray-400 cursor-wait"
  end

  defp button_state_class(false, true) do
    "bg-emerald-500 hover:bg-emerald-400 active:bg-emerald-600 text-gray-900 focus:ring-emerald-500"
  end

  defp button_state_class(false, false) do
    "bg-gray-700 text-gray-400 cursor-not-allowed"
  end

  defp request_label(:idle), do: "Idle"
  defp request_label(:sending), do: "Sending"
  defp request_label(:sent), do: "Sent"
  defp request_label(:error), do: "Error"
  defp request_label(_), do: "Unknown"

  defp request_badge_class(:idle), do: "bg-gray-800 text-gray-300"
  defp request_badge_class(:sending), do: "bg-blue-900/60 text-blue-200"
  defp request_badge_class(:sent), do: "bg-emerald-900/60 text-emerald-200"
  defp request_badge_class(:error), do: "bg-red-900/60 text-red-200"
  defp request_badge_class(_), do: "bg-gray-800 text-gray-300"

  defp acc_status_label(:idle), do: "No ACC feedback yet"
  defp acc_status_label(:pending), do: "Waiting for ACC feedback"
  defp acc_status_label(:matched), do: "Payment successful"
  defp acc_status_label(:unmatched), do: "ACC failed"
  defp acc_status_label(:late), do: "ACC late after gate"
  defp acc_status_label(_), do: "ACC update"

  defp acc_panel_class(:matched), do: "border-emerald-500/60 bg-emerald-900/20"
  defp acc_panel_class(:unmatched), do: "border-red-500/60 bg-red-900/20"
  defp acc_panel_class(:late), do: "border-amber-500/60 bg-amber-900/20"
  defp acc_panel_class(:pending), do: "border-blue-500/50 bg-blue-900/10"
  defp acc_panel_class(:idle), do: "border-gray-700 bg-gray-900/40"
  defp acc_panel_class(_), do: "border-gray-700 bg-gray-900/40"

  defp acc_icon(:matched), do: "✓"
  defp acc_icon(:unmatched), do: "✗"
  defp acc_icon(:late), do: "!"
  defp acc_icon(:pending), do: "..."
  defp acc_icon(_), do: "*"

  defp acc_icon_class(:matched), do: "text-emerald-300"
  defp acc_icon_class(:unmatched), do: "text-red-300"
  defp acc_icon_class(:late), do: "text-amber-300"
  defp acc_icon_class(:pending), do: "text-blue-300"
  defp acc_icon_class(_), do: "text-gray-400"
end
