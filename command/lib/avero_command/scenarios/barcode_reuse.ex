defmodule AveroCommand.Scenarios.BarcodeReuse do
  @moduledoc """
  Scenario #8: Barcode Reuse Detection

  Detects when the same barcode is scanned multiple times within
  a short time window, indicating potential fraud or shared barcode.

  Trigger: barcode.scanned event where barcode was recently used
  Severity: HIGH (potential fraud)
  """
  require Logger

  alias AveroCommand.Store

  # Time window to check for reuse (in seconds)
  @reuse_window_seconds 3600  # 1 hour

  @doc """
  Evaluate if this event triggers the barcode-reuse scenario.
  Event comes through as event_type: "barcodes" with data: %{"type" => "barcode.scanned", ...}
  """
  def evaluate(%{event_type: "barcodes", data: %{"type" => "barcode.scanned"} = data} = event) do
    barcode = data["barcode"]

    if barcode && barcode != "" do
      check_reuse(event, data, barcode)
    else
      :no_match
    end
  end

  def evaluate(_event), do: :no_match

  defp check_reuse(event, data, barcode) do
    site = event.site
    since = DateTime.add(DateTime.utc_now(), -@reuse_window_seconds, :second)

    # Query for recent uses of this barcode
    recent_uses = get_recent_barcode_uses(site, barcode, since)

    if length(recent_uses) > 0 do
      Logger.warning("BarcodeReuse: barcode #{barcode} used #{length(recent_uses) + 1} times in #{@reuse_window_seconds}s")
      {:match, build_incident(event, data, barcode, recent_uses)}
    else
      :no_match
    end
  end

  defp get_recent_barcode_uses(site, barcode, since) do
    # Query for barcode scan events
    Store.recent_events(100, site)
    |> Enum.filter(fn e ->
      e.event_type == "barcodes" &&
        e.data["type"] == "barcode.scanned" &&
        e.data["barcode"] == barcode &&
        DateTime.compare(e.time, since) == :gt
    end)
  rescue
    _ -> []
  end

  defp build_incident(event, data, barcode, previous_uses) do
    gate_id = data["gate_id"] || 0
    use_count = length(previous_uses) + 1
    barcode_type = data["barcode_type"] || "unknown"

    # Get the first use time
    first_use =
      case List.last(previous_uses) do
        nil -> nil
        e -> e.time
      end

    %{
      type: "barcode_reuse",
      severity: severity_for_count(use_count),
      category: "loss_prevention",
      site: event.site,
      gate_id: gate_id,
      context: %{
        barcode: mask_barcode(barcode),
        barcode_type: barcode_type,
        use_count: use_count,
        first_use: first_use,
        gate_id: gate_id,
        window_hours: div(@reuse_window_seconds, 3600),
        message: "Barcode #{mask_barcode(barcode)} used #{use_count} times in #{div(@reuse_window_seconds, 3600)} hour(s)"
      },
      suggested_actions: [
        %{"id" => "block_barcode", "label" => "Block Barcode", "auto" => false},
        %{"id" => "notify_security", "label" => "Notify Security", "auto" => true},
        %{"id" => "review_camera", "label" => "Review Camera Footage", "auto" => false},
        %{"id" => "dismiss", "label" => "Dismiss", "auto" => false}
      ]
    }
  end

  defp severity_for_count(count) when count >= 5, do: "critical"
  defp severity_for_count(count) when count >= 3, do: "high"
  defp severity_for_count(_count), do: "medium"

  # Mask barcode for display (show first 4 and last 4 chars)
  defp mask_barcode(barcode) when is_binary(barcode) and byte_size(barcode) > 8 do
    first = String.slice(barcode, 0, 4)
    last = String.slice(barcode, -4, 4)
    "#{first}...#{last}"
  end

  defp mask_barcode(barcode), do: barcode
end
