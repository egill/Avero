defmodule AveroCommand.MQTT.EventRouter do
  @moduledoc """
  Routes incoming MQTT events to the appropriate handlers.
  - Person events -> PersonSupervisor (creates/updates Person GenServer)
  - Gate events -> GateSupervisor (creates/updates Gate GenServer)
  - All events -> Store (persisted to TimescaleDB)
  """
  require Logger

  alias AveroCommand.Store
  alias AveroCommand.Entities.{PersonRegistry, GateRegistry}
  alias AveroCommand.Scenarios.Evaluator
  alias AveroCommand.Journeys

  @doc """
  Route an event from MQTT to the appropriate handlers.

  Topic formats supported:
  - Gateway-PoC: ["gateway", "journeys"] or ["gateway", "events"]
  - Legacy: ["avero", "events", event_type]
  """
  def route_event(topic, event_data) when is_list(topic) and is_map(event_data) do
    # Handle gateway-poc journey topic directly
    case topic do
      ["gateway", "journeys"] ->
        # Journey JSON from gateway-poc - create journey directly
        Logger.debug("EventRouter: received journey from gateway-poc")
        Task.start(fn -> Journeys.create_from_gateway_json(event_data) end)
        :ok

      ["gateway", topic_name] ->
        # Other gateway-poc topics (events, gate, acc, metrics)
        event = normalize_gateway_event(topic_name, event_data)

        # 1. Persist to store (async)
        Task.start(fn -> Store.insert_event(event) end)

        # 2. Transform to scenario-compatible format and evaluate
        scenario_event = normalize_for_scenarios(topic_name, event, event_data)
        if scenario_event do
          route_to_entities(scenario_event)
          Evaluator.evaluate(scenario_event)
        end

        :ok

      _ ->
        # Legacy avero/events format
        route_legacy_event(topic, event_data)
    end
  end

  def route_event(topic, event_data) do
    Logger.warning("Invalid event format: topic=#{inspect(topic)}, data=#{inspect(event_data)}")
    :error
  end

  # Legacy routing for old avero/events/* format
  defp route_legacy_event(topic, event_data) do
    event_type = extract_event_type(topic)
    event = normalize_event(event_type, event_data)

    # 1. Persist to store (async)
    Task.start(fn -> Store.insert_event(event) end)

    # 2. Route to entity handlers
    route_to_entities(event)

    # 3. Handle journey completion events
    if event_data["type"] == "journey.completed" do
      Task.start(fn -> Journeys.create_from_event(event) end)
    end

    # 4. Evaluate scenarios
    if event_type == "gates" do
      Logger.info("EventRouter: calling Evaluator for gates event type=#{event.data["type"]}")
    end
    Evaluator.evaluate(event)

    :ok
  end

  # Extract event type from topic
  # ["avero", "events", "zone.entry"] -> "zone.entry"
  defp extract_event_type(["avero", "events" | rest]) do
    Enum.join(rest, ".")
  end

  defp extract_event_type(topic) do
    Enum.join(topic, ".")
  end

  # Normalize gateway-poc events (events, gate, acc topics)
  # These use short keys: tid, ts, t, z, etc.
  defp normalize_gateway_event(topic_name, data) do
    time = case data["ts"] do
      ts when is_integer(ts) -> DateTime.from_unix!(ts, :millisecond)
      _ -> DateTime.utc_now()
    end

    %{
      event_type: "gateway.#{topic_name}",
      time: time,
      site: data["site"] || "unknown",
      person_id: data["tid"],  # track ID
      gate_id: nil,
      sensor_id: nil,
      zone: data["z"],
      authorized: data["auth"],
      auth_method: nil,
      duration_ms: data["dwell_ms"],
      data: data
    }
  end

  # Normalize event data into a consistent structure
  defp normalize_event(event_type, data) do
    # Handle compact keys from gate.status.heartbeat events
    # Compact format: g=gate_id, s=status, d=duration_ms, c=crossing_count
    data = expand_compact_keys(data)

    %{
      event_type: event_type,
      time: parse_timestamp(data["timestamp"]) || DateTime.utc_now(),
      site: data["site"] || data["site_id"] || data["gateway_id"] || "unknown",
      person_id: data["person_id"],
      gate_id: data["gate_id"] || data["g"],
      sensor_id: data["sensor_id"],
      zone: data["zone"],
      authorized: data["authorized"],
      auth_method: data["auth_method"],
      duration_ms: data["duration_ms"] || data["dwell_ms"] || data["d"],
      data: data
    }
  end

  # Expand compact keys used in gate.status.heartbeat events
  defp expand_compact_keys(data) when is_map(data) do
    case data["type"] do
      "gate.status.heartbeat" ->
        data
        |> Map.put_new("gate_id", data["g"])
        |> Map.put_new("status", data["s"])
        |> Map.put_new("duration_ms", data["d"])
        |> Map.put_new("crossing_count", data["c"])

      _ ->
        data
    end
  end

  defp expand_compact_keys(data), do: data

  defp parse_timestamp(nil), do: nil

  defp parse_timestamp(ts) when is_binary(ts) do
    case DateTime.from_iso8601(ts) do
      {:ok, dt, _} -> dt
      _ -> nil
    end
  end

  defp parse_timestamp(_), do: nil

  # Route event to Person/Gate GenServers
  defp route_to_entities(%{person_id: person_id} = event) when not is_nil(person_id) do
    # Get or create Person GenServer
    case PersonRegistry.get_or_create(event.site, person_id) do
      {:ok, pid} ->
        GenServer.cast(pid, {:event, event})

      {:error, reason} ->
        Logger.warning("Failed to route to person #{person_id}: #{inspect(reason)}")
    end

    # Also route gate events to gate GenServer
    if event.gate_id do
      route_to_gate(event)
    end
  end

  defp route_to_entities(%{gate_id: gate_id} = event) when not is_nil(gate_id) do
    route_to_gate(event)
  end

  defp route_to_entities(_event) do
    # Event doesn't have person_id or gate_id, skip entity routing
    :ok
  end

  defp route_to_gate(%{gate_id: gate_id, site: site} = event) do
    case GateRegistry.get_or_create(site, gate_id) do
      {:ok, pid} ->
        GenServer.cast(pid, {:event, event})

      {:error, reason} ->
        Logger.warning("Failed to route to gate #{gate_id}: #{inspect(reason)}")
    end
  end

  # =============================================================================
  # Gateway-PoC to Scenario Event Normalization
  # =============================================================================
  # Maps gateway event types to formats expected by scenario detectors.
  # Gateway uses short keys (t, ts, tid) while scenarios expect (event_type, data["type"])

  # gateway/gate: state = cmd_sent | open | closed | moving
  defp normalize_for_scenarios("gate", event, data) do
    gate_type = case data["state"] do
      "open" -> "gate.opened"
      "closed" -> "gate.closed"
      "cmd_sent" -> "gate.cmd"
      "moving" -> "gate.moving"
      _ -> "gate.status"
    end

    gate_id = data["gate_id"] || 1

    %{event |
      event_type: "gates",
      gate_id: gate_id,
      data: Map.merge(event.data, %{
        "type" => gate_type,
        "gate_id" => gate_id,
        "open_duration_ms" => data["duration_ms"],
        "_source" => "gateway"
      })
    }
  end

  # gateway/events: t = zone_entry | zone_exit | line_cross
  defp normalize_for_scenarios("events", event, data) do
    case data["t"] do
      "zone_entry" ->
        # Broadcast zone event for dashboard POS zones
        zone_id = to_string(data["z"])
        Phoenix.PubSub.broadcast(AveroCommand.PubSub, "gateway:events", {:zone_event, %{zone_id: zone_id, event_type: :zone_entry}})

        %{event |
          event_type: "sensors",
          data: Map.merge(event.data, %{
            "type" => "xovis.zone.entry",
            "zone" => data["z"],
            "_source" => "gateway"
          })
        }

      "zone_exit" ->
        # Broadcast zone event for dashboard POS zones
        zone_id = to_string(data["z"])
        Phoenix.PubSub.broadcast(AveroCommand.PubSub, "gateway:events", {:zone_event, %{zone_id: zone_id, event_type: :zone_exit}})

        %{event |
          event_type: "sensors",
          data: Map.merge(event.data, %{
            "type" => "xovis.zone.exit",
            "zone" => data["z"],
            "dwell_ms" => data["dwell_ms"],
            "_source" => "gateway"
          })
        }

      "line_cross" ->
        # Check if this is an exit line crossing
        zone = data["z"] || ""
        is_exit = String.contains?(String.upcase(zone), "EXIT")

        if is_exit do
          %{event |
            event_type: "exits",
            data: Map.merge(event.data, %{
              "type" => "exit.confirmed",
              "authorized" => data["auth"] || false,
              "zone" => zone,
              "_source" => "gateway"
            })
          }
        else
          %{event |
            event_type: "sensors",
            data: Map.merge(event.data, %{
              "type" => "xovis.line.cross",
              "zone" => zone,
              "_source" => "gateway"
            })
          }
        end

      _ ->
        nil
    end
  end

  # gateway/acc: t = received | matched | unmatched
  defp normalize_for_scenarios("acc", event, data) do
    case data["t"] do
      "matched" ->
        # Broadcast payment event for dashboard POS zones
        pos_zone = data["pos"]
        if pos_zone do
          zone_id = to_string(pos_zone)
          Phoenix.PubSub.broadcast(AveroCommand.PubSub, "gateway:events", {:zone_event, %{zone_id: zone_id, event_type: :payment}})
        end

        %{event |
          event_type: "people",
          data: Map.merge(event.data, %{
            "type" => "person.payment.received",
            "person_id" => data["tid"],
            "pos_zone" => data["pos"],
            "_source" => "gateway"
          })
        }

      "received" ->
        %{event |
          event_type: "acc",
          data: Map.merge(event.data, %{
            "type" => "acc.received",
            "pos_zone" => data["pos"],
            "_source" => "gateway"
          })
        }

      "unmatched" ->
        %{event |
          event_type: "acc",
          data: Map.merge(event.data, %{
            "type" => "acc.unmatched",
            "pos_zone" => data["pos"],
            "_source" => "gateway"
          })
        }

      _ ->
        nil
    end
  end

  # gateway/tracks: t = create | delete | stitch | lost | reentry
  defp normalize_for_scenarios("tracks", event, data) do
    case data["t"] do
      "delete" ->
        # Track deletion can be treated as exit for some scenarios
        %{event |
          event_type: "exits",
          data: Map.merge(event.data, %{
            "type" => "exit.confirmed",
            "authorized" => data["auth"] || false,
            "_source" => "gateway"
          })
        }

      "create" ->
        %{event |
          event_type: "tracking",
          data: Map.merge(event.data, %{
            "type" => "track.created",
            "_source" => "gateway"
          })
        }

      "stitch" ->
        %{event |
          event_type: "tracking",
          data: Map.merge(event.data, %{
            "type" => "track.stitched",
            "prev_track_id" => data["prev_tid"],
            "_source" => "gateway"
          })
        }

      _ ->
        # lost, reentry - pass through but don't evaluate
        nil
    end
  end

  # gateway/metrics: metrics snapshots - no scenario evaluation needed
  defp normalize_for_scenarios("metrics", _event, _data), do: nil

  # Unknown topic - skip
  defp normalize_for_scenarios(_topic, _event, _data), do: nil
end
