defmodule AveroCommandWeb.AuthPlug do
  @moduledoc """
  Plug to verify the user is authenticated.
  Redirects to login page if not authenticated.
  """
  import Plug.Conn
  import Phoenix.Controller

  def init(opts), do: opts

  def call(conn, _opts) do
    if get_session(conn, :authenticated) do
      conn
    else
      conn
      |> put_flash(:error, "You must be logged in to access this page")
      |> redirect(to: "/login")
      |> halt()
    end
  end
end
