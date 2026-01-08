defmodule AveroCommand.Repo.Migrations.PersonIdToBigint do
  use Ecto.Migration

  def change do
    alter table(:person_journeys) do
      modify :person_id, :bigint, from: :integer
    end
  end
end
