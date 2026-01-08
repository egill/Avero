defmodule AveroCommand.Repo.Migrations.EventsPersonIdToBigint do
  use Ecto.Migration

  def change do
    alter table(:events) do
      modify :person_id, :bigint, from: :integer
    end
  end
end
