defmodule AveroCommandWeb.PosTestLive do
  @moduledoc """
  Simple POS test page with a single button and clear visual feedback.
  """
  use AveroCommandWeb, :live_view

  require Logger

  @impl true
  def mount(_params, session, socket) do
    site = session["selected_site"] || "avero"

    {:ok,
     assign(socket,
       site: site,
       result: nil,
       loading: false
     )}
  end

  @impl true
  def render(assigns) do
    ~H"""
    <div class="min-h-screen bg-gray-900 flex items-center justify-center p-4">
      <div class="w-full max-w-md">
        <h1 class="text-2xl font-bold text-white text-center mb-8">POS Test</h1>

        <button
          phx-click="simulate_acc"
          disabled={@loading}
          class={[
            "w-full py-8 text-2xl font-bold rounded-2xl transition-all duration-200",
            "focus:outline-none focus:ring-4 focus:ring-offset-2 focus:ring-offset-gray-900",
            if(@loading,
              do: "bg-gray-600 text-gray-400 cursor-wait",
              else: "bg-blue-600 hover:bg-blue-500 active:bg-blue-700 text-white focus:ring-blue-500"
            )
          ]}
        >
          <%= if @loading do %>
            Sending...
          <% else %>
            Simulate Payment
          <% end %>
        </button>

        <%= if @result do %>
          <div class={[
            "mt-8 p-6 rounded-xl text-center",
            if(@result.ok, do: "bg-green-900/50 border-2 border-green-500", else: "bg-red-900/50 border-2 border-red-500")
          ]}>
            <div class={[
              "text-4xl mb-2",
              if(@result.ok, do: "text-green-400", else: "text-red-400")
            ]}>
              <%= if @result.ok do %>
                ✓
              <% else %>
                ✗
              <% end %>
            </div>
            <div class={[
              "text-xl font-semibold",
              if(@result.ok, do: "text-green-300", else: "text-red-300")
            ]}>
              <%= @result.message %>
            </div>
            <%= if @result[:details] do %>
              <div class="mt-2 text-sm text-gray-400">
                <%= @result.details %>
              </div>
            <% end %>
          </div>
        <% end %>

        <div class="mt-8 text-center text-gray-500 text-sm">
          Site: <span class="text-gray-300"><%= @site %></span>
        </div>
      </div>
    </div>
    """
  end

  @impl true
  def handle_event("simulate_acc", _params, socket) do
    site = socket.assigns.site

    # Set loading state
    socket = assign(socket, loading: true, result: nil)

    # Send the request
    send(self(), {:do_simulate, site})

    {:noreply, socket}
  end

  @impl true
  def handle_info({:do_simulate, site}, socket) do
    result = call_gateway_acc(site, "POS_1")

    {:noreply, assign(socket, loading: false, result: result)}
  end

  defp call_gateway_acc(site, pos) do
    gateway_url = AveroCommand.Sites.gateway_url(site, "/acc/simulate?pos=#{pos}")

    if gateway_url do
      :inets.start()
      :ssl.start()

      case :httpc.request(:post, {String.to_charlist(gateway_url), [], ~c"application/json", ~c""}, [{:timeout, 5000}], []) do
        {:ok, {{_, status, _}, _, body}} when status in [200, 201] ->
          # Parse the response to get details
          case Jason.decode(to_string(body)) do
            {:ok, %{"matched" => true, "track_id" => tid}} ->
              %{ok: true, message: "Authorized!", details: "Track #{tid}"}

            {:ok, %{"matched" => true}} ->
              %{ok: true, message: "Authorized!"}

            {:ok, %{"matched" => false}} ->
              %{ok: false, message: "No one found", details: "No person in POS zone"}

            {:ok, _} ->
              %{ok: true, message: "Sent to gateway"}

            {:error, _} ->
              %{ok: true, message: "Sent to gateway"}
          end

        {:ok, {{_, status, _}, _, _body}} ->
          %{ok: false, message: "Gateway error", details: "Status #{status}"}

        {:error, reason} ->
          %{ok: false, message: "Connection failed", details: inspect(reason)}
      end
    else
      %{ok: false, message: "Unknown site", details: site}
    end
  end
end
