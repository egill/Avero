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
  @idle_timeout :timer.minutes(30)
  @unusual_threshold_ms UnusualGateOpening.threshold_ms()
  @max_events 100

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
    fault: false,
    exits_this_cycle: 0,
    last_open_duration_ms: nil,
    max_open_duration_ms: nil,
    min_open_duration_ms: nil
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
      started_at: state.started_at,
      exits_this_cycle: state.exits_this_cycle,
      last_open_duration_ms: state.last_open_duration_ms,
      max_open_duration_ms: state.max_open_duration_ms,
      min_open_duration_ms: state.min_open_duration_ms
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
      Logger.info(
        "Gate #{state.site}:#{state.gate_id} has been open for #{div(@unusual_threshold_ms, 1000)}s - creating incident"
      )

      UnusualGateOpening.create_incident(state.site, state.gate_id, state.last_opened_at)
    end

    {:noreply, %{state | unusual_open_timer_ref: nil}, @idle_timeout}
  end

  # ============================================
  # Event Handlers
  # ============================================

  # Already open - don't reset the timer (prevents metrics heartbeats from resetting)
  defp apply_event(%{state: :open} = state, %{data: %{"type" => "gate.opened"}} = event) do
    %{state | events: add_event(state.events, event)}
  end

  # Transitioning to open - start unusual opening timer
  defp apply_event(state, %{data: %{"type" => "gate.opened"}} = event) do
    cancel_timer(state.unusual_open_timer_ref)
    timer_ref = Process.send_after(self(), :unusual_gate_opening_check, @unusual_threshold_ms)

    %{
      state
      | state: :open,
        last_opened_at: event.time,
        unusual_open_timer_ref: timer_ref,
        exits_this_cycle: 0,
        events: add_event(state.events, event)
    }
  end

  defp apply_event(state, %{data: %{"type" => "gate.closed"}} = event) do
    cancel_timer(state.unusual_open_timer_ref)
    duration_ms = calculate_duration(state.last_opened_at, event.time)
    {new_max, new_min} = update_duration_stats(state, duration_ms)

    %{
      state
      | state: :closed,
        last_closed_at: event.time,
        last_open_duration_ms: duration_ms,
        max_open_duration_ms: new_max,
        min_open_duration_ms: new_min,
        unusual_open_timer_ref: nil,
        events: add_event(state.events, event)
    }
  end

  defp apply_event(state, %{data: %{"type" => "gate.moving"}} = event) do
    %{state | state: :moving, events: add_event(state.events, event)}
  end

  defp apply_event(state, %{data: %{"type" => "gate.fault"}} = event) do
    %{state | fault: true, events: add_event(state.events, event)}
  end

  # Legacy format (direct event_type)
  defp apply_event(state, %{event_type: "gate.fault"} = event) do
    %{state | fault: true, events: add_event(state.events, event)}
  end

  defp apply_event(state, %{event_type: "gate.zone_entry", person_id: person_id} = event)
       when is_binary(person_id) or is_integer(person_id) do
    persons =
      if person_id in state.persons_in_zone,
        do: state.persons_in_zone,
        else: [person_id | state.persons_in_zone]

    %{state | persons_in_zone: persons, events: add_event(state.events, event)}
  end

  defp apply_event(
         %{state: :open} = state,
         %{event_type: "gate.zone_exit", person_id: person_id} = event
       )
       when is_binary(person_id) or is_integer(person_id) do
    %{
      state
      | persons_in_zone: List.delete(state.persons_in_zone, person_id),
        exits_this_cycle: state.exits_this_cycle + 1,
        events: add_event(state.events, event)
    }
  end

  defp apply_event(state, %{event_type: "gate.zone_exit", person_id: person_id} = event)
       when is_binary(person_id) or is_integer(person_id) do
    %{
      state
      | persons_in_zone: List.delete(state.persons_in_zone, person_id),
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

  defp add_event(events, event), do: Enum.take([event | events], @max_events)

  defp cancel_timer(nil), do: :ok
  defp cancel_timer(ref), do: Process.cancel_timer(ref)

  defp calculate_duration(nil, _closed_at), do: nil

  defp calculate_duration(opened_at, closed_at),
    do: DateTime.diff(closed_at, opened_at, :millisecond)

  defp update_duration_stats(state, nil) do
    {state.max_open_duration_ms, state.min_open_duration_ms}
  end

  defp update_duration_stats(state, duration_ms) do
    {
      max(state.max_open_duration_ms || 0, duration_ms),
      min(state.min_open_duration_ms || duration_ms, duration_ms)
    }
  end
end
