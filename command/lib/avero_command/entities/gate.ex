defmodule AveroCommand.Entities.Gate do
  @moduledoc """
  GenServer representing a gate's state.

  Tracks:
  - Gate open/closed state
  - People in gate zone
  - Recent events
  - Error conditions
  """
  use GenServer
  require Logger

  @registry AveroCommand.EntityRegistry

  # Timeout after 30 minutes of inactivity
  @idle_timeout 30 * 60 * 1000

  defstruct [
    :site,
    :gate_id,
    :started_at,
    :state,
    :last_opened_at,
    :last_closed_at,
    persons_in_zone: [],
    events: [],
    fault: false
  ]

  # ============================================
  # Public API
  # ============================================

  def start_link({site, gate_id}) do
    name = via_tuple(site, gate_id)
    GenServer.start_link(__MODULE__, {site, gate_id}, name: name)
  end

  def get_state(pid) when is_pid(pid) do
    GenServer.call(pid, :get_state, 5000)
  catch
    :exit, _ -> nil
  end

  def via_tuple(site, gate_id) do
    {:via, Registry, {@registry, {:gate, {site, gate_id}}}}
  end

  # ============================================
  # GenServer Callbacks
  # ============================================

  @impl true
  def init({site, gate_id}) do
    Logger.debug("Gate #{site}:#{gate_id} started")

    state = %__MODULE__{
      site: site,
      gate_id: gate_id,
      started_at: DateTime.utc_now(),
      state: :closed
    }

    {:ok, state, @idle_timeout}
  end

  @impl true
  def handle_call(:get_state, _from, state) do
    reply = %{
      site: state.site,
      gate_id: state.gate_id,
      state: state.state,
      persons_in_zone: length(state.persons_in_zone),
      last_opened_at: state.last_opened_at,
      last_closed_at: state.last_closed_at,
      fault: state.fault,
      started_at: state.started_at
    }

    {:reply, reply, state, @idle_timeout}
  end

  @impl true
  def handle_cast({:event, event}, state) do
    new_state = apply_event(state, event)
    {:noreply, new_state, @idle_timeout}
  end

  @impl true
  def handle_info(:timeout, state) do
    Logger.debug("Gate #{state.site}:#{state.gate_id} idle timeout")
    {:stop, :normal, state}
  end

  # ============================================
  # Event Handlers
  # ============================================

  defp apply_event(state, %{event_type: "gate.opened"} = event) do
    %{state |
      state: :open,
      last_opened_at: event.time,
      events: add_event(state.events, event)
    }
  end

  defp apply_event(state, %{event_type: "gate.closed"} = event) do
    %{state |
      state: :closed,
      last_closed_at: event.time,
      events: add_event(state.events, event)
    }
  end

  defp apply_event(state, %{event_type: "gate.fault"} = event) do
    %{state |
      fault: true,
      events: add_event(state.events, event)
    }
  end

  defp apply_event(state, %{event_type: "gate.zone_entry", person_id: person_id} = event) when not is_nil(person_id) do
    %{state |
      persons_in_zone: [person_id | state.persons_in_zone] |> Enum.uniq(),
      events: add_event(state.events, event)
    }
  end

  defp apply_event(state, %{event_type: "gate.zone_exit", person_id: person_id} = event) when not is_nil(person_id) do
    %{state |
      persons_in_zone: Enum.reject(state.persons_in_zone, &(&1 == person_id)),
      events: add_event(state.events, event)
    }
  end

  defp apply_event(state, event) do
    # Unknown event type, just log it
    %{state | events: add_event(state.events, event)}
  end

  # ============================================
  # Helper Functions
  # ============================================

  defp add_event(events, event) do
    # Keep last 100 events for gates
    Enum.take([event | events], 100)
  end
end
