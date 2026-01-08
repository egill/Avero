defmodule AveroCommand.Repo do
  use Ecto.Repo,
    otp_app: :avero_command,
    adapter: Ecto.Adapters.Postgres
end
