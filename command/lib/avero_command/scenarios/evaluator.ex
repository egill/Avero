defmodule AveroCommand.Scenarios.Evaluator do
  @moduledoc """
  Evaluates scenarios against incoming events.
  Routes events to appropriate scenario detectors.

  ## Scenario Categories

  - **Loss Prevention** - Theft/fraud detection
  - **Equipment** - Hardware and sensor issues
  - **Safety** - Customer safety concerns
  - **Customer Experience** - Service quality issues
  - **Operational** - System health monitoring

  ## Scheduled Reports (not event-driven)

  The following are handled by Quantum scheduler, not this evaluator:
  - HourlySummary, DailySummary, TrafficAnomaly
  - ShiftChange, LastCustomer, SiteComparison, SiteOffline
  """
  require Logger

  # Loss Prevention scenarios
  alias AveroCommand.Scenarios.{
    NoPaymentExit,
    Tailgating,
    BarcodeReuse,
    # SuspiciousReturn removed - checkout returns are tracked in journey log only
    StaleReceipt,
    QuickExit,
    GroupSplit,
    ExitLoitering,
    BackwardEntry,
    MultipleFailedExits
  }

  # Equipment/Ops scenarios
  alias AveroCommand.Scenarios.{
    GateStuck,
    LongGateDuration,
    GateOffline,
    GateFault,
    GateAlarm,
    SensorOffline,
    EventProcessingLag,
    ClockSkew,
    GateCycleFast,
    GateObstruction
  }

  # Safety scenarios
  alias AveroCommand.Scenarios.{
    PersonTrapped,
    EmergencyMode
  }

  # Customer Experience scenarios
  alias AveroCommand.Scenarios.{
    ConfusedCustomer,
    QueueBuildup
  }

  # Operational scenarios
  alias AveroCommand.Scenarios.HighTraffic

  alias AveroCommand.Incidents.Manager

  # Warmup period after server start (seconds) - ignore alerts during this time
  @warmup_seconds 120

  @scenarios [
    # Loss Prevention (9 scenarios)
    NoPaymentExit,        # #1: Person at gate without payment
    Tailgating,           # #2: Multiple exits in one gate cycle
    BarcodeReuse,         # #8: Same barcode used multiple times
    # SuspiciousReturn removed - checkout returns tracked in journey log only
    StaleReceipt,         # #5: Receipt timestamp > N hours old
    QuickExit,            # #6: Track duration < 30s or no POS zone
    GroupSplit,           # #7: 3+ exit, low payment ratio
    ExitLoitering,        # #9: >60s in exit zone without crossing
    BackwardEntry,        # #10: Wrong direction on exit line
    MultipleFailedExits,  # #11: 3+ gate attempts, 0 authorizations

    # Equipment (10 scenarios)
    GateStuck,            # #13: Gate open > 60 seconds
    LongGateDuration,     # Gate open 30-60 seconds
    GateOffline,          # #14: RS485 communication lost
    GateFault,            # #15: Gate hardware fault
    GateAlarm,            # #16: Gate alarm triggered
    SensorOffline,        # #17: Sensor not responding
    EventProcessingLag,   # #19: High queue depth or latency
    ClockSkew,            # #20: Sensor timestamp differs from gateway
    GateCycleFast,        # #21: Gate open/close < 1s
    GateObstruction,      # #22: Multiple empty gate cycles (no crossings)

    # Safety (2 scenarios)
    PersonTrapped,        # #23: Authorized person stuck at gate
    EmergencyMode,        # #26: Emergency mode activated

    # Customer Experience (2 scenarios)
    ConfusedCustomer,     # #28: 3+ zone cycles without exiting
    QueueBuildup,         # #29: 3+ people queued, gate not cycling

    # Operational (1 scenario)
    HighTraffic           # #30: Too many concurrent persons
  ]

  @doc """
  Initialize the evaluator - call this on application start.
  Sets the warmup start time.
  """
  def init do
    :persistent_term.put({__MODULE__, :start_time}, DateTime.utc_now())
    Logger.info("Evaluator initialized - warmup period: #{@warmup_seconds}s")
    :ok
  end

  @doc """
  Evaluate all applicable scenarios for an event.
  """
  def evaluate(event) do
    if in_warmup_period?() do
      Logger.debug("Evaluator: skipping during warmup period")
      :ok
    else
      do_evaluate(event)
    end
  end

  defp in_warmup_period? do
    case :persistent_term.get({__MODULE__, :start_time}, nil) do
      nil ->
        # If not initialized, initialize now and skip
        init()
        true

      start_time ->
        DateTime.diff(DateTime.utc_now(), start_time, :second) < @warmup_seconds
    end
  end

  defp do_evaluate(event) do
    Logger.debug("Evaluator: processing event_type=#{event.event_type}")
    Enum.each(@scenarios, fn scenario ->
      case scenario.evaluate(event) do
        {:match, incident_attrs} ->
          Logger.info("Scenario matched: #{scenario}")
          Manager.create_incident(incident_attrs)

        :no_match ->
          :ok

        {:error, reason} ->
          Logger.warning("Scenario evaluation error in #{scenario}: #{inspect(reason)}")
      end
    end)
  rescue
    e ->
      Logger.warning("Scenario evaluation failed: #{inspect(e)}")
      :error
  end
end
