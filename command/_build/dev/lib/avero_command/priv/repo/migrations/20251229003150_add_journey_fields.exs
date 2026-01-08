defmodule AveroCommand.Repo.Migrations.AddJourneyFields do
  use Ecto.Migration

  def change do
    alter table(:person_journeys) do
      # Session tracking
      add :session_id, :text

      # Exit details
      add :exit_type, :text  # exit_confirmed, tracking_lost, returned_to_store
      add :gate_opened_by, :text  # xovis or sensor
      add :tailgated, :boolean, default: false

      # Payment details
      add :payment_zone, :text  # Which POS zone they paid at
      add :total_pos_dwell_ms, :integer  # Sum of all POS zone dwell times

      # Dwell tracking
      add :dwell_threshold_met, :boolean, default: false
      add :dwell_zone, :text  # Zone where dwell threshold was met (if different from payment zone)
    end

    # Index for filtering by exit type
    create index(:person_journeys, [:exit_type, :time])
  end
end
