defmodule AveroCommand.Repo.Migrations.AddGroupFields do
  use Ecto.Migration

  def change do
    alter table(:person_journeys) do
      # Group tracking (from Xovis GROUP tracks)
      add :is_group, :boolean, default: false  # True if this was a GROUP track
      add :member_count, :integer, default: 1  # Number of people in the group
      add :group_id, :integer  # Group track ID if part of a group
    end

    # Index for filtering group journeys
    create index(:person_journeys, [:is_group, :time])
  end
end
