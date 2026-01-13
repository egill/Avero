defmodule AveroCommand.Scenarios.EventProcessingLag do
  @moduledoc """
  Scenario #19: Event Processing Lag

  Detects when the gateway is experiencing high queue depths or
  latency spikes, indicating processing bottlenecks.

  Trigger: system.metrics event with high queue depth or latency
  Severity: MEDIUM
  """
  require Logger

  # Thresholds
  @queue_depth_warning 100
  @queue_depth_critical 500
  # 100ms in microseconds
  @latency_warning_us 100_000
  # 500ms in microseconds
  @latency_critical_us 500_000

  @doc """
  Evaluate if this event triggers the event-processing-lag scenario.
  """
  def evaluate(%{event_type: "system.metrics", data: data} = event) do
    queue_depth = data["event_queue_depth"] || data["persister_queue_depth"] || 0
    avg_latency = data["bus_avg_latency_us"] || 0
    max_latency = data["bus_max_latency_us"] || 0

    cond do
      queue_depth >= @queue_depth_critical ->
        {:match, build_incident(event, data, :queue_critical, queue_depth, avg_latency)}

      max_latency >= @latency_critical_us ->
        {:match, build_incident(event, data, :latency_critical, queue_depth, max_latency)}

      queue_depth >= @queue_depth_warning ->
        {:match, build_incident(event, data, :queue_warning, queue_depth, avg_latency)}

      avg_latency >= @latency_warning_us ->
        {:match, build_incident(event, data, :latency_warning, queue_depth, avg_latency)}

      true ->
        :no_match
    end
  end

  def evaluate(_event), do: :no_match

  defp build_incident(event, data, issue_type, queue_depth, latency) do
    {severity, message} =
      case issue_type do
        :queue_critical ->
          {"high", "Critical queue depth: #{queue_depth} (threshold: #{@queue_depth_critical})"}

        :latency_critical ->
          {"high",
           "Critical latency: #{div(latency, 1000)}ms (threshold: #{div(@latency_critical_us, 1000)}ms)"}

        :queue_warning ->
          {"medium", "High queue depth: #{queue_depth} (threshold: #{@queue_depth_warning})"}

        :latency_warning ->
          {"medium",
           "High latency: #{div(latency, 1000)}ms (threshold: #{div(@latency_warning_us, 1000)}ms)"}
      end

    %{
      type: "event_processing_lag",
      severity: severity,
      category: "equipment",
      site: event.site,
      gate_id: 0,
      context: %{
        issue_type: issue_type,
        queue_depth: queue_depth,
        latency_us: latency,
        latency_ms: div(latency, 1000),
        persister_queue: data["persister_queue_depth"] || 0,
        worker_pool_queue: data["worker_pool_queue_depth"] || 0,
        events_dropped: data["events_dropped"] || 0,
        message: message
      },
      suggested_actions: [
        %{"id" => "check_gateway", "label" => "Check Gateway Health", "auto" => false},
        %{"id" => "check_db", "label" => "Check Database", "auto" => false},
        %{"id" => "restart_gateway", "label" => "Restart Gateway", "auto" => false},
        %{"id" => "acknowledge", "label" => "Acknowledge", "auto" => false}
      ]
    }
  end
end
