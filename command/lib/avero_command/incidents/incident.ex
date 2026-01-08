defmodule AveroCommand.Incidents.Incident do
  @moduledoc """
  Ecto schema for incidents.
  """
  use Ecto.Schema
  import Ecto.Changeset

  @primary_key {:id, :binary_id, autogenerate: true}
  schema "incidents" do
    field :type, :string
    field :severity, :string
    field :category, :string
    field :site, :string
    field :gate_id, :integer
    field :status, :string, default: "new"
    field :acknowledged_at, :utc_datetime
    field :acknowledged_by, :string
    field :resolved_at, :utc_datetime
    field :resolved_by, :string
    field :resolution, :string
    field :context, :map, default: %{}
    field :related_person_id, :integer
    field :related_events, {:array, :map}, default: []
    field :suggested_actions, {:array, :map}, default: []
    field :executed_actions, {:array, :map}, default: []

    timestamps(inserted_at: :created_at, type: :utc_datetime)
  end

  def changeset(incident, attrs) do
    incident
    |> cast(attrs, [
      :type,
      :severity,
      :category,
      :site,
      :gate_id,
      :status,
      :acknowledged_at,
      :acknowledged_by,
      :resolved_at,
      :resolved_by,
      :resolution,
      :context,
      :related_person_id,
      :related_events,
      :suggested_actions,
      :executed_actions
    ])
    |> validate_required([:type, :severity, :category, :site])
    |> validate_inclusion(:severity, ["high", "medium", "info"])
    |> validate_inclusion(:status, ["new", "acknowledged", "in_progress", "resolved", "dismissed"])
  end
end
