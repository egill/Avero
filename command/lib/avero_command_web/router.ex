defmodule AveroCommandWeb.Router do
  use AveroCommandWeb, :router

  pipeline :browser do
    plug :accepts, ["html"]
    plug :fetch_session
    plug :fetch_live_flash
    plug :put_root_layout, html: {AveroCommandWeb.Layouts, :root}
    plug :protect_from_forgery
    plug :put_secure_browser_headers
  end

  pipeline :require_auth do
    plug AveroCommandWeb.AuthPlug
  end

  pipeline :api do
    plug :accepts, ["json"]
  end

  # Login page - no auth required
  scope "/", AveroCommandWeb do
    pipe_through :browser

    get "/login", SessionController, :new
    post "/login", SessionController, :create
    delete "/logout", SessionController, :delete
  end

  # Protected routes
  scope "/", AveroCommandWeb do
    pipe_through [:browser, :require_auth]

    live_session :default, on_mount: [{AveroCommandWeb.DashboardHook, :default}, {AveroCommandWeb.SiteFilterHook, :default}, {AveroCommandWeb.AuthHook, :default}] do
      # Main incident feed (LiveView)
      live "/", IncidentFeedLive, :index

      # Incident detail view
      live "/incidents/:id", IncidentDetailLive, :show

      # Customer journeys feed
      live "/journeys", JourneyFeedLive, :index

      # Incident explorer (daily/weekly view)
      live "/explorer", IncidentExplorerLive, :index

      # Debug view (raw events - formerly Explorer)
      live "/debug", ExplorerLive, :index

      # Configuration
      live "/config", ConfigLive, :index
    end
  end

  # Health check endpoint
  scope "/health", AveroCommandWeb do
    pipe_through :api

    get "/", HealthController, :index
  end

  # Prometheus metrics endpoint
  scope "/metrics", AveroCommandWeb do
    pipe_through :api

    get "/", MetricsController, :index
  end

  # API endpoints
  scope "/api", AveroCommandWeb do
    pipe_through :api

    # Review endpoint for automated anomaly detection
    get "/review", ReviewController, :index

    # Journey query API (for chat-journeys CLI tool)
    get "/journeys", JourneyController, :index
    get "/journeys/stats", JourneyController, :stats
    get "/journeys/by-session/:session_id", JourneyController, :by_session
    get "/journeys/:id", JourneyController, :show

    # Debug endpoints (development only)
    get "/debug/persons", DebugController, :persons
    get "/debug/gates", DebugController, :gates
    get "/debug/events", DebugController, :events
  end

  # Enable LiveDashboard in development
  if Application.compile_env(:avero_command, :dev_routes) do
    import Phoenix.LiveDashboard.Router

    scope "/dev" do
      pipe_through [:browser, :require_auth]

      live_dashboard "/dashboard", metrics: AveroCommandWeb.Telemetry
    end
  end
end
