defmodule AveroCommandWeb.FloorMapLive do
  @moduledoc """
  Real-time floor map visualization showing:
  - Store layout with zones from geometry JSON
  - Tracked people as moving circles
  - Updates in real-time from Xovis sensor data via MQTT
  """
  use AveroCommandWeb, :live_view

  require Logger

  @impl true
  def mount(_params, _session, socket) do
    if connected?(socket) do
      Phoenix.PubSub.subscribe(AveroCommand.PubSub, "positions")
      Phoenix.PubSub.subscribe(AveroCommand.PubSub, "acc_events")
      # Periodic cleanup timer every 200ms
      :timer.send_interval(200, self(), :cleanup_stale)
    end

    geometries = load_geometries()

    {:ok,
     socket
     |> assign(:page_title, "Floor Map")
     |> assign(:geometries, geometries)
     |> assign(:positions, %{})
     |> assign(:authorized_tracks, MapSet.new())
     |> assign(:last_update, nil)}
  end

  @impl true
  def handle_info({:positions_update, %{positions: positions}}, socket) do
    now = DateTime.utc_now()
    # Remove dots not updated in 500ms (publishing at 100ms intervals)
    cutoff = DateTime.add(now, -500, :millisecond)

    # Start with existing positions, remove stale ones first
    current =
      socket.assigns.positions
      |> Enum.reject(fn {_id, p} ->
        case p.last_seen do
          nil -> true
          ts -> DateTime.compare(ts, cutoff) == :lt
        end
      end)
      |> Map.new()

    # Update with new positions (only PERSON, no GROUP)
    updated_positions =
      Enum.reduce(positions, current, fn p, acc ->
        tid = p.track_id

        # Skip non-PERSON or GROUP tracks
        if p.type == "PERSON" && Bitwise.band(tid || 0, 0x80000000) == 0 do
          # Always replace position for this track_id
          Map.put(acc, tid, %{x: p.x, y: p.y, z: p.z, type: p.type, last_seen: now})
        else
          acc
        end
      end)

    # Clean up authorized_tracks for tracks no longer visible
    active_track_ids = Map.keys(updated_positions) |> MapSet.new()
    cleaned_authorized = MapSet.intersection(socket.assigns.authorized_tracks, active_track_ids)

    {:noreply,
     socket
     |> assign(:positions, updated_positions)
     |> assign(:authorized_tracks, cleaned_authorized)
     |> assign(:last_update, now)}
  end

  # Handle ACC events to track authorized people
  def handle_info({:acc_event, %{type: "matched", tid: tid}}, socket) when is_integer(tid) do
    authorized = MapSet.put(socket.assigns.authorized_tracks, tid)
    {:noreply, assign(socket, :authorized_tracks, authorized)}
  end

  # Periodic cleanup of stale positions
  def handle_info(:cleanup_stale, socket) do
    now = DateTime.utc_now()
    cutoff = DateTime.add(now, -500, :millisecond)

    updated_positions =
      socket.assigns.positions
      |> Enum.reject(fn {_id, p} ->
        case p.last_seen do
          nil -> true
          ts -> DateTime.compare(ts, cutoff) == :lt
        end
      end)
      |> Map.new()

    # Clean up authorized_tracks too
    active_track_ids = Map.keys(updated_positions) |> MapSet.new()
    cleaned_authorized = MapSet.intersection(socket.assigns.authorized_tracks, active_track_ids)

    {:noreply,
     socket
     |> assign(:positions, updated_positions)
     |> assign(:authorized_tracks, cleaned_authorized)}
  end

  def handle_info(_msg, socket), do: {:noreply, socket}

  defp load_geometries do
    path = Application.app_dir(:avero_command, "priv/geometries/netto.json")

    case File.read(path) do
      {:ok, content} ->
        case Jason.decode(content) do
          {:ok, %{"geometries" => geometries}} -> geometries
          _ -> []
        end

      _ ->
        Logger.warning("Could not load geometry file: #{path}")
        []
    end
  end

  # Coordinate transforms: Xovis meters -> SVG pixels
  # Simple transform: 100 pixels per meter, Y-axis flipped (SVG Y down, Xovis Y up)
  # Origin (0,0) in meters maps to (0,0) in SVG
  @pixels_per_meter 100

  defp to_svg_x(x), do: x * @pixels_per_meter
  defp to_svg_y(y), do: -y * @pixels_per_meter

  defp to_svg_points(geometry) do
    geometry
    |> Enum.map(fn [x, y] -> "#{to_svg_x(x)},#{to_svg_y(y)}" end)
    |> Enum.join(" ")
  end

  defp zone_color(name) do
    cond do
      String.starts_with?(name, "POS") -> "rgba(59, 130, 246, 0.3)"
      String.starts_with?(name, "GATE") -> "rgba(234, 179, 8, 0.3)"
      name == "STORE" -> "rgba(34, 197, 94, 0.1)"
      true -> "rgba(156, 163, 175, 0.2)"
    end
  end

  defp person_color(track_id, authorized_tracks) do
    if MapSet.member?(authorized_tracks, track_id) do
      "#22c55e"  # Green for authorized (ACC matched)
    else
      "#3b82f6"  # Blue for regular
    end
  end

  @impl true
  def render(assigns) do
    ~H"""
    <div class="p-4">
      <div class="flex justify-between items-center mb-4">
        <h1 class="text-2xl font-bold">Floor Map</h1>
        <div class="text-sm text-gray-500">
          <%= map_size(@positions) %> tracked
          <%= if @last_update do %>
            · Updated <%= Calendar.strftime(@last_update, "%H:%M:%S") %>
          <% end %>
        </div>
      </div>

      <div class="bg-gray-900 rounded-lg p-2 overflow-hidden">
        <svg viewBox="-900 -650 1400 1300" class="w-full h-auto" style="max-height: 80vh;">
          <!-- Dark background -->
          <rect x="-900" y="-650" width="1400" height="1300" fill="#1f2937" />
          <!-- Zones -->
          <%= for geom <- @geometries do %>
            <%= if geom["type"] == "ZONE" do %>
              <polygon
                points={to_svg_points(geom["geometry"])}
                fill={zone_color(geom["name"])}
                stroke="white"
                stroke-width="2"
                stroke-opacity="0.5"
              />
              <text
                x={to_svg_x(Enum.at(hd(geom["geometry"]), 0))}
                y={to_svg_y(Enum.at(hd(geom["geometry"]), 1)) + 20}
                fill="white"
                font-size="14"
                font-weight="bold"
              >
                <%= geom["name"] %>
              </text>
            <% else %>
              <!-- Lines -->
              <polyline
                points={to_svg_points(geom["geometry"])}
                fill="none"
                stroke={if String.contains?(geom["name"], "EXIT"), do: "#ef4444", else: "#22c55e"}
                stroke-width="4"
              />
            <% end %>
          <% end %>
          <!-- Tracked people -->
          <%= for {track_id, pos} <- @positions do %>
            <g>
              <circle
                cx={to_svg_x(pos.x)}
                cy={to_svg_y(pos.y)}
                r="15"
                fill={person_color(track_id, @authorized_tracks)}
                stroke="white"
                stroke-width="2"
                opacity="0.9"
              />
              <text
                x={to_svg_x(pos.x)}
                y={to_svg_y(pos.y) + 5}
                text-anchor="middle"
                fill="white"
                font-size="10"
                font-weight="bold"
              >
                <%= rem(track_id, 1000) %>
              </text>
            </g>
          <% end %>
        </svg>
      </div>

      <!-- Legend -->
      <div class="mt-4 flex gap-4 text-sm">
        <div class="flex items-center gap-2">
          <div class="w-4 h-4 rounded-full bg-blue-500"></div>
          <span>Person</span>
        </div>
        <div class="flex items-center gap-2">
          <div class="w-4 h-4 rounded-full bg-green-500"></div>
          <span>Paid (ACC)</span>
        </div>
        <div class="flex items-center gap-2">
          <div class="w-4 h-4 rounded bg-blue-500/30 border border-white/50"></div>
          <span>POS Zone</span>
        </div>
        <div class="flex items-center gap-2">
          <div class="w-4 h-4 rounded bg-yellow-500/30 border border-white/50"></div>
          <span>Gate Zone</span>
        </div>
        <div class="flex items-center gap-2">
          <div class="w-4 h-1 bg-red-500"></div>
          <span>Exit Line</span>
        </div>
      </div>
    </div>
    """
  end
end
