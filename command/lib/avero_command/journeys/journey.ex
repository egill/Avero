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
    field(:time, :utc_datetime_usec)
    field(:site, :string)
    field(:person_id, :integer)
    field(:session_id, :string)

    # Timing
    field(:started_at, :utc_datetime_usec)
    field(:ended_at, :utc_datetime_usec)
    field(:duration_ms, :integer)

    # Outcome
    # paid_exit, unpaid_exit, abandoned
    field(:outcome, :string)
    # exit_confirmed, tracking_lost, returned_to_store
    field(:exit_type, :string)
    field(:authorized, :boolean)
    field(:auth_method, :string)
    field(:receipt_id, :string)

    # Gate details
    # xovis or sensor
    field(:gate_opened_by, :string)
    field(:tailgated, :boolean, default: false)
    # When gate command was sent
    field(:gate_cmd_at, :utc_datetime_usec)
    # When gate opened (from RS485)
    field(:gate_opened_at, :utc_datetime_usec)

    # ACC (payment terminal) correlation
    field(:acc_matched, :boolean, default: false)

    # Payment details
    # Which POS zone they paid at
    field(:payment_zone, :string)
    # Sum of all POS zone dwell times
    field(:total_pos_dwell_ms, :integer)

    # Dwell tracking
    field(:dwell_threshold_met, :boolean, default: false)
    # Zone where dwell threshold was met
    field(:dwell_zone, :string)

    # Group tracking (ACC group = people at POS together when payment arrived)
    # True if part of ACC group (member_count > 1)
    field(:is_group, :boolean, default: false)
    # Number of people in the ACC group
    field(:member_count, :integer, default: 1)
    # Unused (legacy)
    field(:group_id, :integer)
    # Track IDs of all group members
    field(:group_member_ids, {:array, :integer}, default: [])

    # Full path data
    field(:zones_visited, {:array, :map}, default: [])
    field(:events, {:array, :map}, default: [])
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
      :group_member_ids,
      :zones_visited,
      :events
    ])
    |> validate_required([:time, :site, :person_id, :exit_type])
  end
end
