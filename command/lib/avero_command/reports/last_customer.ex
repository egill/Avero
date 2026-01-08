defmodule AveroCommand.Reports.LastCustomer do
  @moduledoc """
  Report #32: Last Customer of the Day

  Logs the last customer exit after store closing.
  Runs periodically after closing time to detect when activity stops.

  Trigger: Scheduled check after closing time
  Type: Info incident
  """
  require Logger

  alias AveroCommand.Store
  alias AveroCommand.Incidents
  alias AveroCommand.Incidents.Manager

  @doc """
  Scheduled job to check for last customer.
  Called by Quantum scheduler after closing time.
  """
  def run do
    Logger.info("LastCustomer: checking for last customer of the day")

    # Get all sites and check each
    sites = get_active_sites()

    Enum.each(sites, fn site ->
      check_last_customer(site)
    end)

    :ok
  end

  defp get_active_sites do
    # Get sites from recent events
    Store.recent_events(500, nil)
    |> Enum.map(& &1.site)
    |> Enum.uniq()
    |> Enum.reject(&is_nil/1)
  rescue
    _ -> []
  end

  defp check_last_customer(site) do
    # Check if we already logged last customer today
    if already_logged_today?(site) do
      :ok
    else
      # Find the last exit of the day
      last_exit = get_last_exit(site)

      if last_exit do
        create_incident(site, last_exit)
      end
    end
  end

  defp already_logged_today?(site) do
    Incidents.list_active()
    |> Enum.any?(fn inc ->
      inc.type == "last_customer" &&
        inc.site == site &&
        DateTime.to_date(inc.created_at) == Date.utc_today()
    end)
  rescue
    _ -> false
  end

  defp get_last_exit(site) do
    today_start = Date.utc_today() |> DateTime.new!(~T[00:00:00], "Etc/UTC")

    Store.recent_events(500, site)
    |> Enum.filter(fn e ->
      e.event_type == "exits" &&
        e.data["type"] == "exit.confirmed" &&
        DateTime.compare(e.time, today_start) == :gt
    end)
    |> Enum.sort_by(& &1.time, {:desc, DateTime})
    |> List.first()
  rescue
    _ -> nil
  end

  defp create_incident(site, last_exit) do
    person_id = last_exit.data["person_id"]
    gate_id = last_exit.data["gate_id"] || 0
    exit_time = last_exit.time

    incident_attrs = %{
      type: "last_customer",
      severity: "info",
      category: "business_intelligence",
      site: site,
      gate_id: gate_id,
      context: %{
        person_id: person_id,
        gate_id: gate_id,
        exit_time: exit_time,
        date: Date.utc_today(),
        message: "Last customer of the day exited at #{Calendar.strftime(exit_time, "%H:%M:%S")}"
      },
      suggested_actions: [
        %{"id" => "acknowledge", "label" => "Acknowledge", "auto" => true}
      ]
    }

    Manager.create_incident(incident_attrs)
    Logger.info("LastCustomer: logged last customer at #{site} - #{exit_time}")
  end
end
