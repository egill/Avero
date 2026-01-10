defmodule AveroCommand.Reports.SiteDiscovery do
  @moduledoc """
  Helpers for finding sites to include in scheduled reports.
  """
  alias AveroCommand.Store
  alias AveroCommand.Journeys

  @default_limit 5000

  @doc """
  List recent sites from both stored events and journeys.
  """
  def list_recent_sites(limit \\ @default_limit) do
    event_sites =
      Store.recent_events(limit, nil)
      |> Enum.map(& &1.site)

    journey_sites =
      Journeys.list_recent(limit: limit)
      |> Enum.map(& &1.site)

    (event_sites ++ journey_sites)
    |> Enum.uniq()
    |> Enum.reject(&is_nil/1)
  rescue
    _ -> []
  end
end
