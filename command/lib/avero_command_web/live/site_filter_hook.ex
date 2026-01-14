defmodule AveroCommandWeb.SiteFilterHook do
  @moduledoc """
  LiveView hook that sets up site filtering for all views.

  Provides a global site selection that affects:
  - Dashboard (gates, Grafana panels, POS zones)
  - Journeys listing
  - Incidents
  - All site-specific data views

  The selected site is stored in the session and persisted across page navigations.
  """
  import Phoenix.Component

  alias AveroCommand.Sites

  def on_mount(:default, _params, session, socket) do
    # Get selected site from session, default to "netto"
    selected_site =
      case session["selected_site"] do
        site when is_binary(site) and site != "" ->
          if Sites.valid?(site), do: site, else: Sites.default_key()

        _ ->
          Sites.default_key()
      end

    # Get site config
    site_config = Sites.get(selected_site)

    # For backwards compatibility, also set selected_sites as a list
    # Note: Use site key ("netto") not site ID ("AP-NETTO-GR-01") because
    # database stores site as the key
    selected_sites = [selected_site]

    {:cont,
     socket
     |> assign(:selected_site, selected_site)
     |> assign(:site_config, site_config)
     |> assign(:selected_sites, selected_sites)
     |> assign(:available_sites, Sites.all())
     |> assign(:site_menu_open, false)}
  end
end
