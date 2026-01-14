defmodule AveroCommandWeb.SessionController do
  use AveroCommandWeb, :controller

  @valid_username "admin"
  @valid_password "avero"

  def new(conn, _params) do
    # If already logged in, redirect to home
    if get_session(conn, :authenticated) do
      redirect(conn, to: "/")
    else
      render(conn, :new, error: nil, layout: {AveroCommandWeb.Layouts, :auth})
    end
  end

  def create(conn, %{"username" => username, "password" => password}) do
    if username == @valid_username and password == @valid_password do
      conn
      |> put_session(:authenticated, true)
      |> put_session(:username, username)
      |> redirect(to: "/")
    else
      render(conn, :new,
        error: "Invalid username or password",
        layout: {AveroCommandWeb.Layouts, :auth}
      )
    end
  end

  def delete(conn, _params) do
    conn
    |> clear_session()
    |> redirect(to: "/login")
  end

  @doc """
  Set the selected site in the session and redirect back.
  """
  def set_site(conn, %{"site" => site}) do
    # Validate site is one of our known sites
    valid_sites = ["netto", "avero"]

    site = if site in valid_sites, do: site, else: "netto"

    # Get the referer to redirect back, default to dashboard
    redirect_to =
      case get_req_header(conn, "referer") do
        [referer] ->
          uri = URI.parse(referer)
          uri.path || "/dashboard"

        _ ->
          "/dashboard"
      end

    conn
    |> put_session(:selected_site, site)
    |> redirect(to: redirect_to)
  end
end
