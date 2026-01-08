defmodule AveroCommand.Scenarios.StaleReceipt do
  @moduledoc """
  Scenario #5: Stale Receipt

  Detects when a barcode/receipt being scanned is older than
  the configured threshold, indicating potential misuse.

  Trigger: barcode.scanned event with old receipt timestamp
  Severity: INFO (configurable alert)
  """
  require Logger

  # Receipt age threshold in hours
  @stale_threshold_hours 24

  @doc """
  Evaluate if this event triggers the stale-receipt scenario.
  Event comes through as event_type: "barcodes" with data: %{"type" => "barcode.scanned", ...}
  """
  def evaluate(%{event_type: "barcodes", data: %{"type" => "barcode.scanned"} = data} = event) do
    receipt_time = parse_receipt_time(data)

    if receipt_time && is_stale?(receipt_time) do
      age_hours = calculate_age_hours(receipt_time)
      Logger.info("StaleReceipt: receipt is #{age_hours} hours old")
      {:match, build_incident(event, data, receipt_time, age_hours)}
    else
      :no_match
    end
  end

  def evaluate(_event), do: :no_match

  defp parse_receipt_time(data) do
    # Try to extract receipt timestamp from various possible fields
    timestamp = data["receipt_timestamp"] || data["receipt_time"] || data["timestamp"]

    case timestamp do
      nil -> nil
      ts when is_binary(ts) -> parse_timestamp_string(ts)
      ts when is_integer(ts) -> DateTime.from_unix(ts) |> elem(1)
      _ -> nil
    end
  end

  defp parse_timestamp_string(ts) do
    case DateTime.from_iso8601(ts) do
      {:ok, dt, _} -> dt
      _ -> nil
    end
  end

  defp is_stale?(receipt_time) do
    age_seconds = DateTime.diff(DateTime.utc_now(), receipt_time, :second)
    age_seconds > @stale_threshold_hours * 3600
  end

  defp calculate_age_hours(receipt_time) do
    age_seconds = DateTime.diff(DateTime.utc_now(), receipt_time, :second)
    Float.round(age_seconds / 3600, 1)
  end

  defp build_incident(event, data, receipt_time, age_hours) do
    gate_id = data["gate_id"] || 0
    barcode = data["barcode"] || "unknown"

    %{
      type: "stale_receipt",
      severity: "info",
      category: "loss_prevention",
      site: event.site,
      gate_id: gate_id,
      context: %{
        gate_id: gate_id,
        barcode: mask_barcode(barcode),
        receipt_time: receipt_time,
        age_hours: age_hours,
        threshold_hours: @stale_threshold_hours,
        message: "Receipt is #{age_hours} hours old (threshold: #{@stale_threshold_hours}h)"
      },
      suggested_actions: [
        %{"id" => "allow", "label" => "Allow Exit", "auto" => false},
        %{"id" => "verify", "label" => "Verify with Customer", "auto" => false},
        %{"id" => "dismiss", "label" => "Dismiss", "auto" => false}
      ]
    }
  end

  defp mask_barcode(barcode) when is_binary(barcode) and byte_size(barcode) > 8 do
    first = String.slice(barcode, 0, 4)
    last = String.slice(barcode, -4, 4)
    "#{first}...#{last}"
  end

  defp mask_barcode(barcode), do: barcode
end
