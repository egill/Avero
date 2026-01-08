defmodule AveroCommandWeb.SessionController do
  use AveroCommandWeb, :controller

  @valid_username "admin"
  @valid_password "avero"

  def new(conn, _params) do
    # If already logged in, redirect to home
    if get_session(conn, :authenticated) do
      redirect(conn, to: "/")
    else
      render(conn, :new, error: nil, layout: false)
    end
  end

  def create(conn, %{"username" => username, "password" => password}) do
    if username == @valid_username and password == @valid_password do
      conn
      |> put_session(:authenticated, true)
      |> put_session(:username, username)
      |> redirect(to: "/")
    else
      render(conn, :new, error: "Invalid username or password", layout: false)
    end
  end

  def delete(conn, _params) do
    conn
    |> clear_session()
    |> redirect(to: "/login")
  end
end
