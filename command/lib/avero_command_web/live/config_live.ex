defmodule AveroCommandWeb.ConfigLive do
  use AveroCommandWeb, :live_view

  @impl true
  def mount(_params, _session, socket) do
    sites = AveroCommand.Store.list_site_configs()

    {:ok,
     socket
     |> assign(:sites, sites)
     |> assign(:page_title, "Configuration")}
  end

  @impl true
  def render(assigns) do
    ~H"""
    <div class="config">
      <.header>
        Configuration
        <:subtitle>Manage site settings and scenario thresholds</:subtitle>
      </.header>

      <div class="mt-6 space-y-6">
        <%= for site <- @sites do %>
          <div class="bg-white shadow rounded-lg overflow-hidden">
            <div class="px-6 py-4 border-b border-gray-200">
              <h3 class="text-lg font-medium text-gray-900"><%= site.name %></h3>
              <p class="text-sm text-gray-500"><%= site.site %> - <%= site.timezone %></p>
            </div>
            <div class="px-6 py-4">
              <dl class="grid grid-cols-2 gap-4">
                <div>
                  <dt class="text-sm font-medium text-gray-500">Operating Hours</dt>
                  <dd class="mt-1 text-sm text-gray-900">
                    <%= format_hours(site.operating_hours) %>
                  </dd>
                </div>
                <div>
                  <dt class="text-sm font-medium text-gray-500">Scenario Config</dt>
                  <dd class="mt-1 text-sm text-gray-900">
                    <%= format_config(site.scenario_config) %>
                  </dd>
                </div>
              </dl>
            </div>
          </div>
        <% end %>

        <%= if Enum.empty?(@sites) do %>
          <div class="bg-white shadow rounded-lg p-6 text-center text-gray-500">
            No sites configured yet.
          </div>
        <% end %>
      </div>
    </div>
    """
  end

  defp format_hours(%{"start" => start, "end" => end_time}) do
    "#{start} - #{end_time}"
  end

  defp format_hours(_), do: "Not configured"

  defp format_config(config) when map_size(config) == 0 do
    "Using defaults"
  end

  defp format_config(config) do
    config
    |> Map.keys()
    |> Enum.join(", ")
  end
end
