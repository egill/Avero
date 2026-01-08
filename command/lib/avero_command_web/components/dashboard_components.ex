defmodule AveroCommandWeb.DashboardComponents do
  @moduledoc """
  Dashboard layout components - sidebar, header, and layout wrapper.
  """
  use Phoenix.Component
  use Phoenix.VerifiedRoutes, endpoint: AveroCommandWeb.Endpoint, router: AveroCommandWeb.Router

  @doc """
  Renders the main dashboard layout with sidebar and header.
  """
  attr :current_path, :string, required: true
  attr :sidebar_collapsed, :boolean, default: false
  attr :mobile_sidebar_open, :boolean, default: false
  slot :inner_block, required: true

  def dashboard_layout(assigns) do
    ~H"""
    <div class="flex h-screen overflow-hidden" id="dashboard-layout" phx-hook="DarkMode">
      <!-- Mobile sidebar backdrop -->
      <div
        :if={@mobile_sidebar_open}
        class="fixed inset-0 z-9999 bg-black/50 lg:hidden"
        phx-click="close-mobile-sidebar"
      >
      </div>

      <!-- Sidebar -->
      <.sidebar
        current_path={@current_path}
        collapsed={@sidebar_collapsed}
        mobile_open={@mobile_sidebar_open}
      />

      <!-- Main content area -->
      <div class={[
        "relative flex flex-1 flex-col overflow-x-hidden overflow-y-auto",
        "transition-all duration-300",
        !@sidebar_collapsed && "lg:ml-[290px]",
        @sidebar_collapsed && "lg:ml-[90px]"
      ]}>
        <.dashboard_header collapsed={@sidebar_collapsed} />

        <main class="flex-1">
          <div class="mx-auto max-w-screen-2xl p-4 md:p-6">
            <%= render_slot(@inner_block) %>
          </div>
        </main>
      </div>
    </div>
    """
  end

  @doc """
  Renders the sidebar navigation.
  """
  attr :current_path, :string, required: true
  attr :collapsed, :boolean, default: false
  attr :mobile_open, :boolean, default: false

  def sidebar(assigns) do
    ~H"""
    <aside
      id="sidebar"
      class={[
        "fixed top-0 left-0 z-9999 flex h-screen flex-col",
        "overflow-y-auto border-r border-gray-200 bg-white px-5",
        "transition-all duration-300",
        "dark:border-gray-800 dark:bg-gray-900",
        # Desktop: always visible, width changes
        @collapsed && "lg:w-[90px]",
        !@collapsed && "lg:w-[290px]",
        # Mobile: slide in/out
        @mobile_open && "translate-x-0 w-[290px]",
        !@mobile_open && "-translate-x-full lg:translate-x-0"
      ]}
      phx-hook="Sidebar"
      data-collapsed={to_string(@collapsed)}
    >
      <!-- Sidebar Header -->
      <div class={[
        "flex items-center gap-2 pt-8 pb-7",
        @collapsed && "justify-center",
        !@collapsed && "justify-between"
      ]}>
        <.link navigate={~p"/"} class="flex items-center gap-2">
          <span class={[
            "text-xl font-bold text-brand-500",
            @collapsed && "lg:hidden"
          ]}>
            Avero Command
          </span>
          <span :if={@collapsed} class="hidden lg:block text-xl font-bold text-brand-500">
            AC
          </span>
        </.link>

        <!-- Close button for mobile -->
        <button
          class="lg:hidden text-gray-500 hover:text-gray-700 dark:text-gray-400 dark:hover:text-gray-200"
          phx-click="close-mobile-sidebar"
        >
          <svg class="w-6 h-6" fill="none" stroke="currentColor" viewBox="0 0 24 24">
            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 18L18 6M6 6l12 12" />
          </svg>
        </button>
      </div>

      <!-- Navigation -->
      <nav class="flex flex-col gap-1 overflow-y-auto">
        <.nav_item
          path={~p"/dashboard"}
          label="Dashboard"
          current_path={@current_path}
          collapsed={@collapsed}
        >
          <:icon>
            <svg class="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M4 5a1 1 0 011-1h14a1 1 0 011 1v2a1 1 0 01-1 1H5a1 1 0 01-1-1V5zM4 13a1 1 0 011-1h6a1 1 0 011 1v6a1 1 0 01-1 1H5a1 1 0 01-1-1v-6zM16 13a1 1 0 011-1h2a1 1 0 011 1v6a1 1 0 01-1 1h-2a1 1 0 01-1-1v-6z" />
            </svg>
          </:icon>
        </.nav_item>

        <.nav_item
          path={~p"/"}
          label="Incidents"
          current_path={@current_path}
          collapsed={@collapsed}
        >
          <:icon>
            <svg class="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 9v2m0 4h.01m-6.938 4h13.856c1.54 0 2.502-1.667 1.732-3L13.732 4c-.77-1.333-2.694-1.333-3.464 0L3.34 16c-.77 1.333.192 3 1.732 3z" />
            </svg>
          </:icon>
        </.nav_item>

        <.nav_item
          path={~p"/journeys"}
          label="Journeys"
          current_path={@current_path}
          collapsed={@collapsed}
        >
          <:icon>
            <svg class="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M9 5H7a2 2 0 00-2 2v12a2 2 0 002 2h10a2 2 0 002-2V7a2 2 0 00-2-2h-2M9 5a2 2 0 002 2h2a2 2 0 002-2M9 5a2 2 0 012-2h2a2 2 0 012 2m-3 7h3m-3 4h3m-6-4h.01M9 16h.01" />
            </svg>
          </:icon>
        </.nav_item>

        <.nav_item
          path={~p"/explorer"}
          label="Explorer"
          current_path={@current_path}
          collapsed={@collapsed}
        >
          <:icon>
            <svg class="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M21 21l-6-6m2-5a7 7 0 11-14 0 7 7 0 0114 0z" />
            </svg>
          </:icon>
        </.nav_item>

        <.nav_item
          path={~p"/debug"}
          label="Debug"
          current_path={@current_path}
          collapsed={@collapsed}
        >
          <:icon>
            <svg class="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M10 20l4-16m4 4l4 4-4 4M6 16l-4-4 4-4" />
            </svg>
          </:icon>
        </.nav_item>

        <.nav_item
          path={~p"/config"}
          label="Config"
          current_path={@current_path}
          collapsed={@collapsed}
        >
          <:icon>
            <svg class="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M10.325 4.317c.426-1.756 2.924-1.756 3.35 0a1.724 1.724 0 002.573 1.066c1.543-.94 3.31.826 2.37 2.37a1.724 1.724 0 001.065 2.572c1.756.426 1.756 2.924 0 3.35a1.724 1.724 0 00-1.066 2.573c.94 1.543-.826 3.31-2.37 2.37a1.724 1.724 0 00-2.572 1.065c-.426 1.756-2.924 1.756-3.35 0a1.724 1.724 0 00-2.573-1.066c-1.543.94-3.31-.826-2.37-2.37a1.724 1.724 0 00-1.065-2.572c-1.756-.426-1.756-2.924 0-3.35a1.724 1.724 0 001.066-2.573c-.94-1.543.826-3.31 2.37-2.37.996.608 2.296.07 2.572-1.065z" />
              <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M15 12a3 3 0 11-6 0 3 3 0 016 0z" />
            </svg>
          </:icon>
        </.nav_item>

        <!-- Divider -->
        <div class="my-4 border-t border-gray-200 dark:border-gray-800"></div>

        <!-- Logout -->
        <.link
          href={~p"/logout"}
          method="delete"
          class={[
            "flex items-center gap-3 rounded-lg px-3 py-2.5",
            "text-red-600 hover:bg-red-50 dark:text-red-400 dark:hover:bg-red-900/20",
            "transition-colors duration-200"
          ]}
        >
          <svg class="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M17 16l4-4m0 0l-4-4m4 4H7m6 4v1a3 3 0 01-3 3H6a3 3 0 01-3-3V7a3 3 0 013-3h4a3 3 0 013 3v1" />
          </svg>
          <span class={[@collapsed && "lg:hidden"]}>Logout</span>
        </.link>
      </nav>
    </aside>
    """
  end

  # Renders a navigation item in the sidebar.
  attr :path, :string, required: true
  attr :label, :string, required: true
  attr :current_path, :string, required: true
  attr :collapsed, :boolean, default: false
  slot :icon, required: true

  defp nav_item(assigns) do
    is_active = assigns.current_path == assigns.path ||
      (assigns.path == "/" && assigns.current_path in ["/", "/incidents"]) ||
      String.starts_with?(assigns.current_path, assigns.path <> "/")

    assigns = assign(assigns, :is_active, is_active)

    ~H"""
    <.link
      navigate={@path}
      class={[
        "flex items-center gap-3 rounded-lg px-3 py-2.5",
        "transition-colors duration-200",
        @is_active && "bg-brand-500 text-white",
        !@is_active && "text-gray-700 hover:bg-gray-100 dark:text-gray-300 dark:hover:bg-gray-800"
      ]}
    >
      <span class={[@is_active && "text-white", !@is_active && "text-gray-500 dark:text-gray-400"]}>
        <%= render_slot(@icon) %>
      </span>
      <span class={[@collapsed && "lg:hidden"]}><%= @label %></span>
    </.link>
    """
  end

  @doc """
  Renders the dashboard header with hamburger menu and dark mode toggle.
  """
  attr :collapsed, :boolean, default: false

  def dashboard_header(assigns) do
    ~H"""
    <header class="sticky top-0 z-999 flex w-full border-b border-gray-200 bg-white dark:border-gray-800 dark:bg-gray-900">
      <div class="flex grow items-center justify-between px-4 py-3 lg:px-6">
        <!-- Left side: hamburger menu -->
        <div class="flex items-center gap-4">
          <!-- Mobile hamburger -->
          <button
            class="lg:hidden flex h-10 w-10 items-center justify-center rounded-lg text-gray-500 hover:bg-gray-100 dark:text-gray-400 dark:hover:bg-gray-800"
            phx-click="toggle-mobile-sidebar"
          >
            <svg class="w-6 h-6" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M4 6h16M4 12h16M4 18h16" />
            </svg>
          </button>

          <!-- Desktop collapse toggle -->
          <button
            class="hidden lg:flex h-10 w-10 items-center justify-center rounded-lg border border-gray-200 text-gray-500 hover:bg-gray-100 dark:border-gray-700 dark:text-gray-400 dark:hover:bg-gray-800"
            phx-click="toggle-sidebar"
          >
            <svg class="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M4 6h16M4 12h8M4 18h16" />
            </svg>
          </button>
        </div>

        <!-- Right side: dark mode toggle -->
        <div class="flex items-center gap-3">
          <button
            class="flex h-10 w-10 items-center justify-center rounded-lg text-gray-500 hover:bg-gray-100 dark:text-gray-400 dark:hover:bg-gray-800"
            phx-click="toggle-dark-mode"
            title="Toggle dark mode"
          >
            <!-- Sun icon (shown in dark mode) -->
            <svg class="hidden dark:block w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 3v1m0 16v1m9-9h-1M4 12H3m15.364 6.364l-.707-.707M6.343 6.343l-.707-.707m12.728 0l-.707.707M6.343 17.657l-.707.707M16 12a4 4 0 11-8 0 4 4 0 018 0z" />
            </svg>
            <!-- Moon icon (shown in light mode) -->
            <svg class="block dark:hidden w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M20.354 15.354A9 9 0 018.646 3.646 9.003 9.003 0 0012 21a9.003 9.003 0 008.354-5.646z" />
            </svg>
          </button>
        </div>
      </div>
    </header>
    """
  end
end
