defmodule AveroCommandWeb.DashboardHook do
  @moduledoc """
  LiveView hook that sets up dashboard layout state.
  Handles sidebar collapse state and current path for navigation highlighting.
  """
  import Phoenix.Component
  import Phoenix.LiveView

  def on_mount(:default, _params, _session, socket) do
    # Get the current path from the socket's view module
    current_path = module_to_path(socket.view)

    socket =
      socket
      |> assign(:current_path, current_path)
      |> assign(:sidebar_collapsed, false)
      |> assign(:mobile_sidebar_open, false)
      |> attach_hook(:dashboard_events, :handle_event, &handle_dashboard_event/3)

    {:cont, socket}
  end

  # Map LiveView modules to their paths
  defp module_to_path(AveroCommandWeb.IncidentFeedLive), do: "/"
  defp module_to_path(AveroCommandWeb.IncidentDetailLive), do: "/incidents"
  defp module_to_path(AveroCommandWeb.JourneyFeedLive), do: "/journeys"
  defp module_to_path(AveroCommandWeb.IncidentExplorerLive), do: "/explorer"
  defp module_to_path(AveroCommandWeb.ExplorerLive), do: "/debug"
  defp module_to_path(AveroCommandWeb.ConfigLive), do: "/config"
  defp module_to_path(_), do: "/"

  # Handle dashboard-related events
  defp handle_dashboard_event("toggle-sidebar", _params, socket) do
    {:halt, assign(socket, :sidebar_collapsed, !socket.assigns.sidebar_collapsed)}
  end

  defp handle_dashboard_event("toggle-mobile-sidebar", _params, socket) do
    {:halt, assign(socket, :mobile_sidebar_open, !socket.assigns.mobile_sidebar_open)}
  end

  defp handle_dashboard_event("close-mobile-sidebar", _params, socket) do
    {:halt, assign(socket, :mobile_sidebar_open, false)}
  end

  defp handle_dashboard_event("toggle-dark-mode", _params, socket) do
    # Push event to JavaScript to toggle dark mode
    {:halt, push_event(socket, "toggle-dark-mode", %{})}
  end

  defp handle_dashboard_event("sidebar-init", %{"collapsed" => collapsed}, socket) do
    {:halt, assign(socket, :sidebar_collapsed, collapsed)}
  end

  defp handle_dashboard_event("sidebar-toggled", %{"collapsed" => collapsed}, socket) do
    {:halt, assign(socket, :sidebar_collapsed, collapsed)}
  end

  defp handle_dashboard_event("sidebar-mobile-closed", _params, socket) do
    {:halt, assign(socket, :mobile_sidebar_open, false)}
  end

  defp handle_dashboard_event("click-outside", _params, socket) do
    {:halt, assign(socket, :mobile_sidebar_open, false)}
  end

  # Pass through other events
  defp handle_dashboard_event(_event, _params, socket) do
    {:cont, socket}
  end
end
