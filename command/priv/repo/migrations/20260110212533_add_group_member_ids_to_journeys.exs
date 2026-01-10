defmodule AveroCommand.Repo.Migrations.AddGroupMemberIdsToJourneys do
  use Ecto.Migration

  def change do
    alter table(:person_journeys) do
      add :group_member_ids, {:array, :integer}, default: []
    end
  end
end
