defmodule AveroCommand.Store.Event do
  @moduledoc """
  Ecto schema for events stored in TimescaleDB.
  """
  use Ecto.Schema
  import Ecto.Changeset

  @primary_key false
  schema "events" do
    field(:id, :integer, primary_key: true)
    field(:time, :utc_datetime_usec, primary_key: true)
    field(:site, :string)
    field(:event_type, :string)
    field(:person_id, :integer)
    field(:gate_id, :integer)
    field(:sensor_id, :string)
    field(:zone, :string)
    field(:authorized, :boolean)
    field(:auth_method, :string)
    field(:duration_ms, :integer)
    field(:data, :map)
  end

  def changeset(event, attrs) do
    event
    |> cast(attrs, [
      :time,
      :site,
      :event_type,
      :person_id,
      :gate_id,
      :sensor_id,
      :zone,
      :authorized,
      :auth_method,
      :duration_ms,
      :data
    ])
    |> validate_required([:time, :site, :event_type])
    |> put_default_time()
  end

  defp put_default_time(changeset) do
    if get_field(changeset, :time) do
      changeset
    else
      put_change(changeset, :time, DateTime.utc_now())
    end
  end
end
