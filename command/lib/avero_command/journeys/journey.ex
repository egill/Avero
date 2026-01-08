defmodule AveroCommand.Journeys.Journey do
  @moduledoc """
  Ecto schema for person journeys.

  Tracks customer journeys through the store including:
  - Zone visits and dwell times
  - Payment information
  - Exit outcome (exited, lost, returned)
  """
  use Ecto.Schema
  import Ecto.Changeset

  @primary_key {:id, :id, autogenerate: true}
  schema "person_journeys" do
    field :time, :utc_datetime_usec
    field :site, :string
    field :person_id, :integer
    field :session_id, :string

    # Timing
    field :started_at, :utc_datetime_usec
    field :ended_at, :utc_datetime_usec
    field :duration_ms, :integer

    # Outcome
    field :outcome, :string  # paid_exit, unpaid_exit, abandoned
    field :exit_type, :string  # exit_confirmed, tracking_lost, returned_to_store
    field :authorized, :boolean
    field :auth_method, :string
    field :receipt_id, :string

    # Gate details
    field :gate_opened_by, :string  # xovis or sensor
    field :tailgated, :boolean, default: false
    field :gate_cmd_at, :utc_datetime_usec  # When gate command was sent
    field :gate_opened_at, :utc_datetime_usec  # When gate opened (from RS485)

    # ACC (payment terminal) correlation
    field :acc_matched, :boolean, default: false

    # Payment details
    field :payment_zone, :string  # Which POS zone they paid at
    field :total_pos_dwell_ms, :integer  # Sum of all POS zone dwell times

    # Dwell tracking
    field :dwell_threshold_met, :boolean, default: false
    field :dwell_zone, :string  # Zone where dwell threshold was met

    # Group tracking (from Xovis GROUP tracks)
    field :is_group, :boolean, default: false  # True if this was a GROUP track
    field :member_count, :integer, default: 1  # Number of people in the group
    field :group_id, :integer  # Group track ID if part of a group

    # Full path data
    field :zones_visited, {:array, :map}, default: []
    field :events, {:array, :map}, default: []
  end

  def changeset(journey, attrs) do
    journey
    |> cast(attrs, [
      :time,
      :site,
      :person_id,
      :session_id,
      :started_at,
      :ended_at,
      :duration_ms,
      :outcome,
      :exit_type,
      :authorized,
      :auth_method,
      :receipt_id,
      :gate_opened_by,
      :tailgated,
      :gate_cmd_at,
      :gate_opened_at,
      :acc_matched,
      :payment_zone,
      :total_pos_dwell_ms,
      :dwell_threshold_met,
      :dwell_zone,
      :is_group,
      :member_count,
      :group_id,
      :zones_visited,
      :events
    ])
    |> validate_required([:time, :site, :person_id, :exit_type])
  end
end
