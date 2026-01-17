defmodule AveroCommand.MQTT.EventRouter do
  @moduledoc """
  Routes incoming MQTT events to the appropriate handlers.
  - Person events -> PersonRegistry (creates/updates Person GenServer)
  - Gate events -> GateRegistry (creates/updates Gate GenServer)
  - All events -> Store (persisted to TimescaleDB)
  """
  require Logger

  alias AveroCommand.Entities.{GateRegistry, PersonRegistry}
  alias AveroCommand.Journeys
  alias AveroCommand.Scenarios.{Evaluator, UnusualGateOpening}
  alias AveroCommand.Sites
  alias AveroCommand.Store

  @doc """
  Route an event from MQTT to the appropriate handlers.

  Topic formats:
  - Gateway-PoC: ["gateway", "journeys"] or ["gateway", "events"]
  - Legacy: ["avero", "events", event_type]
  """
  def route_event(topic, event_data) when is_list(topic) and is_map(event_data) do
    case topic do
      ["gateway", "journeys"] ->
        Logger.debug("EventRouter: received journey from gateway-poc")
        Task.start(fn -> Journeys.create_from_gateway_json(event_data) end)
        :ok

      ["gateway", "positions"] ->
        route_gateway_positions(event_data)

      ["gateway", topic_name] ->
        route_gateway_event(topic_name, event_data)

      ["xovis", "sensor"] ->
        route_xovis_sensor(event_data)

      _ ->
        route_legacy_event(topic, event_data)
    end
  end

  def route_event(topic, event_data) do
    Logger.warning("Invalid event format: topic=#{inspect(topic)}, data=#{inspect(event_data)}")
    :error
  end

  defp route_gateway_event(topic_name, event_data) do
    event = normalize_gateway_event(topic_name, event_data)
    Task.start(fn -> Store.insert_event(event) end)

    case normalize_for_scenarios(topic_name, event, event_data) do
      nil ->
        :ok

      scenario_event ->
        Task.start(fn -> Store.insert_event(scenario_event) end)
        route_to_entities(scenario_event)
        Evaluator.evaluate(scenario_event)
        :ok
    end
  end

  defp route_legacy_event(topic, event_data) do
    event_type = extract_event_type(topic)
    event = normalize_event(event_type, event_data)

    Task.start(fn -> Store.insert_event(event) end)
    route_to_entities(event)

    if event_data["type"] == "journey.completed" do
      Task.start(fn -> Journeys.create_from_event(event) end)
    end

    if event_type == "gates" do
      Logger.info("EventRouter: calling Evaluator for gates event type=#{event.data["type"]}")
    end

    Evaluator.evaluate(event)
    :ok
  end

  defp extract_event_type(["avero", "events" | rest]), do: Enum.join(rest, ".")
  defp extract_event_type(topic), do: Enum.join(topic, ".")

  defp normalize_gateway_event(topic_name, data) do
    %{
      event_type: "gateway.#{topic_name}",
      time: parse_unix_timestamp(data["ts"]),
      site: data["site"] || "unknown",
      person_id: data["tid"],
      gate_id: nil,
      sensor_id: nil,
      zone: data["z"],
      authorized: data["auth"],
      auth_method: nil,
      duration_ms: data["dwell_ms"],
      data: data
    }
  end

  defp normalize_event(event_type, data) do
    data = expand_compact_keys(data)

    %{
      event_type: event_type,
      time: parse_iso_timestamp(data["timestamp"]) || DateTime.utc_now(),
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

  defp expand_compact_keys(%{"type" => "gate.status.heartbeat"} = data) do
    data
    |> Map.put_new("gate_id", data["g"])
    |> Map.put_new("status", data["s"])
    |> Map.put_new("duration_ms", data["d"])
    |> Map.put_new("crossing_count", data["c"])
  end

  defp expand_compact_keys(data), do: data

  defp parse_unix_timestamp(ts) when is_integer(ts), do: DateTime.from_unix!(ts, :millisecond)
  defp parse_unix_timestamp(_), do: DateTime.utc_now()

  defp parse_iso_timestamp(ts) when is_binary(ts) do
    case DateTime.from_iso8601(ts) do
      {:ok, dt, _} -> dt
      _ -> nil
    end
  end

  defp parse_iso_timestamp(_), do: nil

  defp route_to_entities(%{person_id: person_id} = event) when not is_nil(person_id) do
    route_to_person(event.site, person_id, event)

    if event.gate_id do
      route_to_gate(event)
    end
  end

  defp route_to_entities(%{gate_id: gate_id} = event) when not is_nil(gate_id) do
    route_to_gate(event)
  end

  defp route_to_entities(_event), do: :ok

  defp route_to_person(site, person_id, event) do
    case PersonRegistry.get_or_create(site, person_id) do
      {:ok, pid} ->
        GenServer.cast(pid, {:event, event})

      {:error, reason} ->
        Logger.warning("Failed to route to person #{person_id}: #{inspect(reason)}")
    end
  end

  defp route_to_gate(%{gate_id: gate_id, site: site} = event) do
    case GateRegistry.get_or_create(site, gate_id) do
      {:ok, pid} -> GenServer.cast(pid, {:event, event})
      {:error, reason} -> Logger.warning("Failed to route to gate #{gate_id}: #{inspect(reason)}")
    end
  end

  # Gateway-PoC to Scenario Event Normalization
  # Maps gateway event types to formats expected by scenario detectors.

  defp normalize_for_scenarios("gate", event, data) do
    gate_id = data["gate_id"] || 1
    gate_type = gate_state_to_type(data["state"])

    broadcast_gate_event(event.site, gate_id, data["state"], event.time)

    if data["state"] == "closed" do
      Task.start(fn -> UnusualGateOpening.maybe_resolve(event.site, gate_id) end)
    end

    update_event(event,
      event_type: "gates",
      gate_id: gate_id,
      extra_data: %{
        "type" => gate_type,
        "gate_id" => gate_id,
        "open_duration_ms" => data["duration_ms"]
      }
    )
  end

  defp normalize_for_scenarios("events", event, data) do
    case data["t"] do
      "zone_entry" ->
        broadcast_zone_event(event.site, data["z"], :zone_entry)

        update_event(event,
          event_type: "sensors",
          extra_data: %{"type" => "xovis.zone.entry", "zone" => data["z"]}
        )

      "zone_exit" ->
        broadcast_zone_event(event.site, data["z"], :zone_exit)

        update_event(event,
          event_type: "sensors",
          extra_data: %{
            "type" => "xovis.zone.exit",
            "zone" => data["z"],
            "dwell_ms" => data["dwell_ms"]
          }
        )

      "line_cross" ->
        normalize_line_cross(event, data)

      _ ->
        nil
    end
  end

  defp normalize_for_scenarios("acc", event, data) do
    broadcast_acc_event(event, data)

    case data["t"] do
      "matched" ->
        broadcast_payment_event(event.site, data["pos"])

        update_event(event,
          event_type: "people",
          extra_data: %{
            "type" => "person.payment.received",
            "person_id" => data["tid"],
            "pos_zone" => data["pos"]
          }
        )

      "received" ->
        update_event(event,
          event_type: "acc",
          extra_data: %{"type" => "acc.received", "pos_zone" => data["pos"]}
        )

      "unmatched" ->
        update_event(event,
          event_type: "acc",
          extra_data: %{"type" => "acc.unmatched", "pos_zone" => data["pos"]}
        )

      "matched_no_journey" ->
        update_event(event,
          event_type: "acc",
          extra_data: %{
            "type" => "acc.matched_no_journey",
            "pos_zone" => data["pos"],
            "person_id" => data["tid"]
          }
        )

      "late_after_gate" ->
        update_event(event,
          event_type: "acc",
          extra_data: %{
            "type" => "acc.late_after_gate",
            "pos_zone" => data["pos"],
            "person_id" => data["tid"],
            "delta_ms" => data["delta_ms"]
          }
        )

      _ ->
        nil
    end
  end

  defp normalize_for_scenarios("tracks", event, data) do
    case data["t"] do
      "delete" ->
        update_event(event,
          event_type: "exits",
          extra_data: %{"type" => "exit.confirmed", "authorized" => data["auth"] || false}
        )

      "create" ->
        update_event(event,
          event_type: "tracking",
          extra_data: %{"type" => "track.created"}
        )

      "stitch" ->
        update_event(event,
          event_type: "tracking",
          extra_data: %{"type" => "track.stitched", "prev_track_id" => data["prev_tid"]}
        )

      _ ->
        nil
    end
  end

  defp normalize_for_scenarios("metrics", event, data) do
    site = data["site"]

    if site do
      gate_id = data["gate_id"] || 1
      gate_type = gate_state_to_type(data["gate_state"] || "unknown")

      case GateRegistry.get_or_create(site, gate_id) do
        {:ok, pid} ->
          synthetic_event = %{
            event_type: "gates",
            site: site,
            gate_id: gate_id,
            time: event.time,
            data: %{"type" => gate_type}
          }

          GenServer.cast(pid, {:event, synthetic_event})

        {:error, _} ->
          :ok
      end
    end

    nil
  end

  defp normalize_for_scenarios(_topic, _event, _data), do: nil

  # Helper functions for normalize_for_scenarios

  defp gate_state_to_type("open"), do: "gate.opened"
  defp gate_state_to_type("closed"), do: "gate.closed"
  defp gate_state_to_type("cmd_sent"), do: "gate.cmd"
  defp gate_state_to_type("moving"), do: "gate.moving"
  defp gate_state_to_type(_), do: "gate.status"

  defp broadcast_gate_event(site, gate_id, state, time) do
    Phoenix.PubSub.broadcast(
      AveroCommand.PubSub,
      "gates",
      {:gate_event, %{site: site, gate_id: gate_id, state: state, time: time}}
    )
  end

  defp normalize_line_cross(event, data) do
    zone = data["z"] || ""

    if String.contains?(String.upcase(zone), "EXIT") do
      update_event(event,
        event_type: "exits",
        extra_data: %{
          "type" => "exit.confirmed",
          "authorized" => data["auth"] || false,
          "zone" => zone
        }
      )
    else
      update_event(event,
        event_type: "sensors",
        extra_data: %{"type" => "xovis.line.cross", "zone" => zone}
      )
    end
  end

  defp broadcast_zone_event(site, zone, event_type) do
    Phoenix.PubSub.broadcast(
      AveroCommand.PubSub,
      "gateway:events",
      {:zone_event, %{site: site, zone_id: zone, event_type: event_type}}
    )
  end

  defp broadcast_acc_event(event, data) do
    acc_event = %{
      type: data["t"],
      ts: data["ts"],
      ip: data["ip"],
      pos: data["pos"],
      tid: data["tid"],
      dwell_ms: data["dwell_ms"],
      gate_zone: data["gate_zone"],
      gate_entry_ts: data["gate_entry_ts"],
      delta_ms: data["delta_ms"],
      gate_cmd_at: data["gate_cmd_at"],
      debug_active: data["debug_active"],
      debug_pending: data["debug_pending"],
      site: event.site,
      time: event.time
    }

    Phoenix.PubSub.broadcast(AveroCommand.PubSub, "acc_events", {:acc_event, acc_event})
  end

  defp broadcast_payment_event(site, pos_zone) when not is_nil(pos_zone) do
    Phoenix.PubSub.broadcast(
      AveroCommand.PubSub,
      "gateway:events",
      {:zone_event, %{site: site, zone_id: to_string(pos_zone), event_type: :payment}}
    )
  end

  defp broadcast_payment_event(_, _), do: :ok

  defp update_event(event, opts) do
    event_type = Keyword.get(opts, :event_type, event.event_type)
    gate_id = Keyword.get(opts, :gate_id, event.gate_id)
    extra_data = Keyword.get(opts, :extra_data, %{})

    %{
      event
      | event_type: event_type,
        gate_id: gate_id,
        data: Map.merge(event.data, Map.put(extra_data, "_source", "gateway"))
    }
  end

  # Gateway position data routing for floor map visualization
  # Format: { ts, tid, obj_type, x, y, z, zone, auth, ctx }
  defp route_gateway_positions(data) do
    position = %{
      track_id: data["tid"],
      type: data["obj_type"] || "PERSON",
      x: data["x"],
      y: data["y"],
      z: data["z"]
    }

    # Skip GROUP tracks (high bit set)
    if Bitwise.band(position.track_id || 0, 0x80000000) == 0 do
      Phoenix.PubSub.broadcast(
        AveroCommand.PubSub,
        "positions",
        {:positions_update, %{positions: [position], timestamp: DateTime.utc_now()}}
      )
    end

    :ok
  end

  # Xovis sensor position data routing for floor map visualization
  defp route_xovis_sensor(data) do
    case data do
      %{"live_data" => %{"frames" => frames}} ->
        Enum.each(frames, &broadcast_positions/1)
        :ok

      _ ->
        :ok
    end
  end

  defp broadcast_positions(frame) do
    tracked_objects = frame["tracked_objects"] || []

    positions =
      Enum.map(tracked_objects, fn obj ->
        [x, y, z] = obj["position"] || [0, 0, 0]

        %{
          track_id: obj["track_id"],
          type: obj["type"] || "PERSON",
          x: x,
          y: y,
          z: z
        }
      end)

    if positions != [] do
      Phoenix.PubSub.broadcast(
        AveroCommand.PubSub,
        "positions",
        {:positions_update, %{positions: positions, timestamp: DateTime.utc_now()}}
      )
    end
  end
end
