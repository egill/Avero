defmodule AveroCommand.Entities.Person do
  @moduledoc """
  GenServer representing a tracked person's state machine.

  States: :tracking -> :in_zone -> :at_gate -> :exited

  Tracks:
  - Zones visited with dwell times
  - Payment status
  - Current state
  - Events history
  """
  use GenServer
  require Logger

  @registry AveroCommand.EntityRegistry

  # Timeout after 5 minutes of inactivity
  @idle_timeout 5 * 60 * 1000

  defstruct [
    :site,
    :person_id,
    :started_at,
    :state,
    :current_zone,
    :current_gate,
    zones_visited: [],
    payments: [],
    dwelled_at_pos: false,
    has_payment: false,
    events: []
  ]

  # ============================================
  # Public API
  # ============================================

  def start_link({site, person_id}) do
    name = via_tuple(site, person_id)
    GenServer.start_link(__MODULE__, {site, person_id}, name: name)
  end

  def get_state(pid) when is_pid(pid) do
    GenServer.call(pid, :get_state, 5000)
  catch
    :exit, _ -> nil
  end

  def via_tuple(site, person_id) do
    {:via, Registry, {@registry, {:person, {site, person_id}}}}
  end

  # ============================================
  # GenServer Callbacks
  # ============================================

  @impl true
  def init({site, person_id}) do
    Logger.debug("Person #{site}:#{person_id} started")

    state = %__MODULE__{
      site: site,
      person_id: person_id,
      started_at: DateTime.utc_now(),
      state: :tracking
    }

    {:ok, state, @idle_timeout}
  end

  @impl true
  def handle_call(:get_state, _from, state) do
    reply = %{
      site: state.site,
      person_id: state.person_id,
      state: state.state,
      current_zone: state.current_zone,
      current_gate: state.current_gate,
      zones_visited: state.zones_visited,
      zones_visited_count: length(state.zones_visited),
      has_payment: state.has_payment,
      dwelled_at_pos: state.dwelled_at_pos,
      started_at: state.started_at
    }

    {:reply, reply, state, @idle_timeout}
  end

  @impl true
  def handle_cast({:event, event}, state) do
    new_state = apply_event(state, event)

    # Check if person has exited
    if new_state.state == :exited do
      Logger.debug("Person #{state.site}:#{state.person_id} exited, stopping")
      persist_journey(new_state)
      {:stop, :normal, new_state}
    else
      {:noreply, new_state, @idle_timeout}
    end
  end

  @impl true
  def handle_info(:timeout, state) do
    Logger.debug("Person #{state.site}:#{state.person_id} idle timeout")
    {:stop, :normal, state}
  end

  # ============================================
  # Event Handlers
  # ============================================

  defp apply_event(state, %{event_type: "zone.entry"} = event) do
    zone_visit = %{
      zone: event.zone,
      enter_time: event.time,
      exit_time: nil,
      dwell_ms: nil
    }

    %{
      state
      | current_zone: event.zone,
        state: :in_zone,
        zones_visited: [zone_visit | state.zones_visited],
        events: add_event(state.events, event)
    }
  end

  defp apply_event(state, %{event_type: "zone.exit"} = event) do
    zones = update_zone_exit(state.zones_visited, event.zone, event.time, event.duration_ms)
    dwelled = dwelled_at_pos?(zones)

    %{
      state
      | current_zone: nil,
        zones_visited: zones,
        dwelled_at_pos: dwelled,
        events: add_event(state.events, event)
    }
  end

  defp apply_event(state, %{event_type: "person.state_changed"} = event) do
    new_state_atom =
      case event.data["to"] || event.data["state"] do
        "at_gate" -> :at_gate
        "tracking" -> :tracking
        "in_zone" -> :in_zone
        "exited" -> :exited
        _ -> state.state
      end

    gate_id = event.data["gate_id"] || event.gate_id

    %{
      state
      | state: new_state_atom,
        current_gate: gate_id,
        events: add_event(state.events, event)
    }
  end

  defp apply_event(state, %{event_type: "payment.received"} = event) do
    payment = %{
      receipt_id: event.data["receipt_id"],
      zone: event.zone,
      time: event.time
    }

    %{
      state
      | payments: [payment | state.payments],
        has_payment: true,
        events: add_event(state.events, event)
    }
  end

  defp apply_event(state, %{event_type: "exit.confirmed"} = event) do
    %{state | state: :exited, events: add_event(state.events, event)}
  end

  defp apply_event(state, %{event_type: "exit.rejected"} = event) do
    # Exit was rejected, person stays at gate
    %{state | events: add_event(state.events, event)}
  end

  defp apply_event(state, event) do
    # Unknown event type, just log it
    %{state | events: add_event(state.events, event)}
  end

  # ============================================
  # Helper Functions
  # ============================================

  defp add_event(events, event) do
    # Keep last 50 events
    Enum.take([event | events], 50)
  end

  defp update_zone_exit(zones, zone, exit_time, dwell_ms) do
    Enum.map(zones, fn z ->
      if z.zone == zone and is_nil(z.exit_time) do
        %{z | exit_time: exit_time, dwell_ms: dwell_ms}
      else
        z
      end
    end)
  end

  defp dwelled_at_pos?(zones) do
    zones
    |> Enum.filter(fn z -> String.starts_with?(z.zone || "", "POS") end)
    |> Enum.any?(fn z -> z.dwell_ms && z.dwell_ms > 30_000 end)
  end

  defp persist_journey(state) do
    # Persist the completed journey for analytics
    journey = %{
      time: DateTime.utc_now(),
      site: state.site,
      person_id: state.person_id,
      started_at: state.started_at,
      ended_at: DateTime.utc_now(),
      duration_ms: DateTime.diff(DateTime.utc_now(), state.started_at, :millisecond),
      outcome: determine_outcome(state),
      authorized: state.has_payment,
      zones_visited: state.zones_visited,
      events: length(state.events)
    }

    Logger.debug("Journey complete: #{inspect(journey)}")

    # Could persist to person_journeys table here
    :ok
  end

  defp determine_outcome(state) do
    cond do
      state.state == :exited and state.has_payment -> "paid_exit"
      state.state == :exited -> "unpaid_exit"
      true -> "abandoned"
    end
  end
end
