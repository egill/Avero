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

  alias AveroCommand.Scenarios.UnusualGateOpening

  @registry AveroCommand.EntityRegistry

  # Timeout after 30 minutes of inactivity
  @idle_timeout 30 * 60 * 1000

  # Threshold for unusual gate opening incident (2 minutes)
  @unusual_threshold_ms UnusualGateOpening.threshold_ms()

  defstruct [
    :site,
    :gate_id,
    :started_at,
    :state,
    :last_opened_at,
    :last_closed_at,
    :unusual_open_timer_ref,
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

  @impl true
  def handle_info(:unusual_gate_opening_check, state) do
    # Timer fired - check if gate is still open
    if state.state == :open do
      Logger.info("Gate #{state.site}:#{state.gate_id} has been open for #{div(@unusual_threshold_ms, 1000)}s - creating incident")
      UnusualGateOpening.create_incident(state.site, state.gate_id, state.last_opened_at)
    end

    {:noreply, %{state | unusual_open_timer_ref: nil}, @idle_timeout}
  end

  # ============================================
  # Event Handlers
  # ============================================

  defp apply_event(state, %{event_type: "gate.opened"} = event) do
    # Cancel any existing timer
    if state.unusual_open_timer_ref, do: Process.cancel_timer(state.unusual_open_timer_ref)

    # Schedule timer for unusual gate opening detection
    timer_ref = Process.send_after(self(), :unusual_gate_opening_check, @unusual_threshold_ms)

    %{state |
      state: :open,
      last_opened_at: event.time,
      unusual_open_timer_ref: timer_ref,
      events: add_event(state.events, event)
    }
  end

  defp apply_event(state, %{event_type: "gate.closed"} = event) do
    # Cancel unusual gate opening timer if set
    if state.unusual_open_timer_ref, do: Process.cancel_timer(state.unusual_open_timer_ref)

    %{state |
      state: :closed,
      last_closed_at: event.time,
      unusual_open_timer_ref: nil,
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
