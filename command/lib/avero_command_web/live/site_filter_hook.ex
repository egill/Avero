defmodule AveroCommandWeb.SiteFilterHook do
  @moduledoc """
  LiveView hook that sets up site filtering for all views.
  Initializes selected_sites from session or defaults to AVERO-HQ.
  """
  import Phoenix.Component

  alias AveroCommand.Store

  @default_site "AP-AVERO-GR-01"

  def on_mount(:default, _params, session, socket) do
    # Get available sites from recent events
    available_sites = get_available_sites()

    # Get selected sites from session or default to AVERO-HQ
    selected_sites =
      case session["selected_sites"] do
        nil -> [@default_site]
        sites when is_list(sites) -> sites
        _ -> [@default_site]
      end

    # Ensure selected sites are still valid
    selected_sites = Enum.filter(selected_sites, &(&1 in available_sites))
    selected_sites = if Enum.empty?(selected_sites), do: available_sites, else: selected_sites

    {:cont,
     socket
     |> assign(:available_sites, available_sites)
     |> assign(:selected_sites, selected_sites)
     |> assign(:site_menu_open, false)}
  end

  defp get_available_sites do
    Store.recent_events(500, nil)
    |> Enum.map(& &1.site)
    |> Enum.uniq()
    |> Enum.reject(&is_nil/1)
    |> Enum.sort()
  rescue
    _ -> [@default_site]
  end
end
