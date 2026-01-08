defmodule AveroCommandWeb.AuthHook do
  @moduledoc """
  LiveView hook to verify the user is authenticated.
  Redirects to login page if session is not authenticated.
  """
  import Phoenix.LiveView
  import Phoenix.Component

  def on_mount(:default, _params, session, socket) do
    if session["authenticated"] do
      {:cont, assign(socket, :current_user, session["username"])}
    else
      {:halt, redirect(socket, to: "/login")}
    end
  end
end
