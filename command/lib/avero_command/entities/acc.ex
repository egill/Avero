defmodule AveroCommand.Entities.Acc do
  @moduledoc """
  GenServer representing an ACC (payment terminal) zone's state.

  Tracks:
  - Payment events (received, matched, unmatched)
  - Counts per event type
  - Time since last event (health monitoring)
  - POS zone associations
  """
  use GenServer
  require Logger

  @registry AveroCommand.EntityRegistry

  # Timeout after 30 minutes of inactivity
  @idle_timeout 30 * 60 * 1000

  defstruct [
    :site,
    :pos_zone,
    :started_at,
    :last_event_at,
    received_count: 0,
    matched_count: 0,
    unmatched_count: 0,
    events: []
  ]

  # ============================================
  # Public API
  # ============================================

  def start_link({site, pos_zone}) do
    name = via_tuple(site, pos_zone)
    GenServer.start_link(__MODULE__, {site, pos_zone}, name: name)
  end

  def get_state(pid) when is_pid(pid) do
    GenServer.call(pid, :get_state, 5000)
  catch
    :exit, _ -> nil
  end

  def via_tuple(site, pos_zone) do
    {:via, Registry, {@registry, {:acc, {site, pos_zone}}}}
  end

  # ============================================
  # GenServer Callbacks
  # ============================================

  @impl true
  def init({site, pos_zone}) do
    Logger.debug("ACC #{site}:#{pos_zone} started")

    state = %__MODULE__{
      site: site,
      pos_zone: pos_zone,
      started_at: DateTime.utc_now()
    }

    {:ok, state, @idle_timeout}
  end

  @impl true
  def handle_call(:get_state, _from, state) do
    reply = %{
      site: state.site,
      pos_zone: state.pos_zone,
      received_count: state.received_count,
      matched_count: state.matched_count,
      unmatched_count: state.unmatched_count,
      last_event_at: state.last_event_at,
      started_at: state.started_at,
      match_rate: calc_match_rate(state)
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
    Logger.debug("ACC #{state.site}:#{state.pos_zone} idle timeout")
    {:stop, :normal, state}
  end

  # ============================================
  # Event Handlers
  # ============================================

  defp apply_event(state, %{data: %{"type" => "acc.received"}} = event) do
    %{state |
      received_count: state.received_count + 1,
      last_event_at: event.time,
      events: add_event(state.events, event)
    }
  end

  defp apply_event(state, %{data: %{"type" => "acc.matched"}} = event) do
    %{state |
      matched_count: state.matched_count + 1,
      last_event_at: event.time,
      events: add_event(state.events, event)
    }
  end

  defp apply_event(state, %{data: %{"type" => "person.payment.received"}} = event) do
    # This is also a matched payment (routed from "people" event type)
    %{state |
      matched_count: state.matched_count + 1,
      last_event_at: event.time,
      events: add_event(state.events, event)
    }
  end

  defp apply_event(state, %{data: %{"type" => "acc.unmatched"}} = event) do
    %{state |
      unmatched_count: state.unmatched_count + 1,
      last_event_at: event.time,
      events: add_event(state.events, event)
    }
  end

  defp apply_event(state, event) do
    # Unknown event type, just log it
    %{state |
      last_event_at: event.time,
      events: add_event(state.events, event)
    }
  end

  # ============================================
  # Helper Functions
  # ============================================

  defp add_event(events, event) do
    # Keep last 50 events for ACC
    Enum.take([event | events], 50)
  end

  defp calc_match_rate(%{received_count: 0}), do: nil
  defp calc_match_rate(%{received_count: received, matched_count: matched}) do
    Float.round(matched / received * 100, 1)
  end
end
