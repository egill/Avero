defmodule AveroCommand.Repo.Migrations.AddGatewayPocFields do
  use Ecto.Migration

  def change do
    alter table(:person_journeys) do
      # ACC (payment terminal) correlation flag
      add :acc_matched, :boolean, default: false

      # Gate timing fields from gateway-poc
      add :gate_cmd_at, :utc_datetime_usec
      add :gate_opened_at, :utc_datetime_usec
    end

    # Index for querying by ACC match status
    create index(:person_journeys, [:acc_matched])
  end
end
