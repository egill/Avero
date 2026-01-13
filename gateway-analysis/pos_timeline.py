#!/usr/bin/env python3
"""POS Timeline Viewer - TUI for analyzing ACC and Xovis events."""

import argparse
import json
import subprocess
from datetime import datetime, timezone
from pathlib import Path

import polars as pl
from textual.app import App, ComposeResult
from textual.binding import Binding
from textual.widgets import DataTable, Footer, Header, Static

# Netto zone mappings (geometry_id -> name)
ZONE_NAMES = {
    "1000": "GATE_1",
    "1001": "POS_3",
    "1002": "POS_4",
    "1003": "POS_5",
    "1004": "POS_1",
    "1005": "POS_2",
    "1006": "EXIT_1",
    "1008": "ENTRY_1",
    "1009": "STORE",
    "1010": "APPROACH_1",
}

POS_ZONE_IDS = {name: int(zid) for zid, name in ZONE_NAMES.items() if name.startswith("POS_")}

GROUP_ID_BASE = 2_147_483_648  # 2^31 - IDs >= this are group tracks

EVENT_DISPLAY = {
    "ZONE_ENTRY": ">> ENTER",
    "ZONE_EXIT": "<< EXIT",
    "ACC": "$$$ ACC $$$",
    "TRACK_CREATE": "[+] CREATE",
    "TRACK_DELETE": "[-] DELETE",
}

# Common schema for timeline events
TIMELINE_SCHEMA = {
    "ts_recv": pl.Datetime("ms"),
    "ts_event": pl.Datetime("ms"),
    "latency_ms": pl.Int64,
    "source": pl.Utf8,
    "event_type": pl.Utf8,
    "track_id": pl.Int64,
    "zone_id": pl.Int64,
    "zone_name": pl.Utf8,
    "receipt_id": pl.Utf8,
    "position": pl.Utf8,
    "height": pl.Float64,
    "frame_number": pl.Int64,
}


def parse_iso_to_ms(ts_str: str) -> int:
    """Parse ISO8601 timestamp to milliseconds since epoch."""
    dt = datetime.fromisoformat(ts_str.replace("Z", "+00:00"))
    return int(dt.timestamp() * 1000)


def empty_timeline_df() -> pl.DataFrame:
    """Create an empty DataFrame with the timeline schema."""
    return pl.DataFrame(schema=TIMELINE_SCHEMA)


def load_acc_events(acc_dir: Path, date_tag: str) -> pl.DataFrame:
    """Load ACC events from JSONL files for the given date."""
    acc_files = list(acc_dir.glob(f"*-{date_tag}.jsonl"))
    if not acc_files:
        return empty_timeline_df()

    rows = []
    for path in acc_files:
        with open(path) as f:
            for line in f:
                line = line.strip()
                if not line:
                    continue
                rec = json.loads(line)
                ts_recv = rec.get("ts_recv")
                if not ts_recv:
                    continue
                fields = rec.get("fields", {})
                ts_ms = parse_iso_to_ms(ts_recv)
                rows.append({
                    "ts_recv": ts_ms,
                    "ts_event": ts_ms,
                    "latency_ms": None,
                    "source": "acc",
                    "event_type": "ACC",
                    "track_id": None,
                    "zone_id": None,
                    "zone_name": fields.get("pos_zone"),
                    "receipt_id": fields.get("receipt_id"),
                    "position": None,
                    "height": None,
                    "frame_number": None,
                })

    df = pl.DataFrame(rows)
    return df.with_columns([
        pl.col("ts_recv").cast(pl.Datetime("ms")),
        pl.col("ts_event").cast(pl.Datetime("ms")),
    ])


XOVIS_EVENT_TYPES = frozenset(("ZONE_ENTRY", "ZONE_EXIT", "TRACK_CREATE", "TRACK_DELETE"))


def load_xovis_events(mqtt_dir: Path, date_tag: str, zone_names: dict) -> pl.DataFrame:
    """Load and flatten Xovis sensor events from JSONL."""
    sensor_file = mqtt_dir / f"xovis-sensor-{date_tag}.jsonl"
    if not sensor_file.exists():
        return empty_timeline_df()

    rows = []
    with open(sensor_file) as f:
        for line in f:
            line = line.strip()
            if not line:
                continue
            rec = json.loads(line)
            ts_recv_str = rec.get("ts_recv")
            if not ts_recv_str:
                continue
            ts_recv_ms = parse_iso_to_ms(ts_recv_str)

            payload_raw = rec.get("payload_raw")
            if not payload_raw:
                continue
            try:
                payload = json.loads(payload_raw)
            except json.JSONDecodeError:
                continue

            frames = payload.get("live_data", {}).get("frames", [])

            for frame in frames:
                frame_time = frame.get("time")
                if not isinstance(frame_time, int):
                    continue
                frame_number = frame.get("framenumber")

                # Build track_id -> position/height lookup from tracked_objects
                track_info = {}
                for obj in frame.get("tracked_objects", []):
                    tid = obj.get("track_id")
                    if tid is not None:
                        pos = obj.get("position")
                        attrs = obj.get("attributes", {})
                        track_info[tid] = {
                            "position": pos,
                            "height": attrs.get("person_height"),
                        }

                for ev in frame.get("events", []):
                    if ev.get("category") != "SCENE":
                        continue
                    event_type = ev.get("type")
                    if event_type not in XOVIS_EVENT_TYPES:
                        continue

                    attrs = ev.get("attributes", {})
                    track_id = attrs.get("track_id")
                    zone_id = attrs.get("geometry_id")
                    info = track_info.get(track_id, {})
                    pos = info.get("position")
                    pos_str = f"({pos[0]:.1f},{pos[1]:.1f},{pos[2]:.1f})" if pos else None

                    rows.append({
                        "ts_recv": ts_recv_ms,
                        "ts_event": frame_time,
                        "latency_ms": ts_recv_ms - frame_time,
                        "source": "xovis",
                        "event_type": event_type,
                        "track_id": track_id,
                        "zone_id": zone_id,
                        "zone_name": zone_names.get(str(zone_id)) if zone_id else None,
                        "receipt_id": None,
                        "position": pos_str,
                        "height": info.get("height"),
                        "frame_number": frame_number,
                    })

    if not rows:
        return empty_timeline_df()

    df = pl.DataFrame(rows)
    return df.with_columns([
        pl.col("ts_recv").cast(pl.Datetime("ms")),
        pl.col("ts_event").cast(pl.Datetime("ms")),
    ])


def normalize_schema(df: pl.DataFrame) -> pl.DataFrame:
    """Ensure consistent schema for concatenation."""
    return df.select([pl.col(name).cast(dtype) for name, dtype in TIMELINE_SCHEMA.items()])


def load_timeline(log_dir: Path, date_tag: str, zone_names: dict) -> pl.DataFrame:
    """Load and merge ACC and Xovis events into unified timeline."""
    acc_df = load_acc_events(log_dir / "acc", date_tag)
    xovis_df = load_xovis_events(log_dir / "mqtt", date_tag, zone_names)

    acc_df = normalize_schema(acc_df)
    xovis_df = normalize_schema(xovis_df)

    return pl.concat([acc_df, xovis_df])


def build_person_group_links(df: pl.DataFrame, zone_name: str, window_ms: int = 500) -> dict[int, int]:
    """
    Build mapping of person_id -> group_id based on zone entries within window_ms.
    Returns dict mapping person track IDs to their linked group track IDs.
    """
    entries = df.filter(
        (pl.col("zone_name") == zone_name) &
        (pl.col("event_type") == "ZONE_ENTRY")
    ).sort("ts_event")

    rows = entries.to_dicts()
    person_to_group: dict[int, int] = {}

    for i in range(len(rows) - 1):
        curr = rows[i]
        next_row = rows[i + 1]

        curr_ts = curr["ts_event"]
        next_ts = next_row["ts_event"]
        curr_id = curr["track_id"]
        next_id = next_row["track_id"]

        if not all([curr_ts, next_ts, curr_id, next_id]):
            continue

        delta_ms = (next_ts - curr_ts).total_seconds() * 1000
        if delta_ms > window_ms:
            continue

        curr_is_group = curr_id >= GROUP_ID_BASE
        next_is_group = next_id >= GROUP_ID_BASE

        # Link person to group if one of each
        if curr_is_group and not next_is_group:
            person_to_group[next_id] = curr_id
        elif next_is_group and not curr_is_group:
            person_to_group[curr_id] = next_id

    return person_to_group


def compute_zone_occupancy(df: pl.DataFrame, zone_name: str) -> list[dict]:
    """Process events chronologically, tracking zone occupancy state at each event."""
    df_sorted = df.sort("ts_event")
    active_tracks: set[int] = set()
    results = []

    for row in df_sorted.iter_rows(named=True):
        event_type = row["event_type"]
        track_id = row["track_id"]
        is_zone_event = row["zone_name"] == zone_name
        pre_state = active_tracks.copy()

        if is_zone_event and track_id is not None:
            if event_type == "ZONE_ENTRY":
                active_tracks.add(track_id)
            elif event_type == "ZONE_EXIT":
                active_tracks.discard(track_id)

        result = dict(row)
        result["active_tracks_before"] = pre_state
        result["active_tracks_after"] = active_tracks.copy()
        result["is_zone_event"] = is_zone_event
        results.append(result)

    return results


class TimelineViewer(App):
    """TUI application for viewing POS timeline with occupancy tracking."""

    CSS = """
    #header-info {
        dock: top;
        height: 2;
        background: $primary;
        color: $text;
        padding: 0 1;
    }
    DataTable {
        height: 1fr;
    }
    .entry { color: green; }
    .exit { color: red; }
    .acc { color: yellow; background: $primary-darken-2; }
    """

    BINDINGS = [
        Binding("q", "quit", "Quit"),
        Binding("h", "prev_hour", "Prev Hour"),
        Binding("l", "next_hour", "Next Hour"),
        Binding("left", "prev_hour", "Prev Hour", show=False),
        Binding("right", "next_hour", "Next Hour", show=False),
        Binding("n", "next_acc", "Next ACC"),
        Binding("p", "prev_acc", "Prev ACC"),
        Binding("c", "copy_receipt", "Copy Receipt"),
        *[Binding(str(i), f"select_pos('POS_{i}')", f"POS_{i}") for i in range(1, 6)],
        Binding("f", "toggle_filter", "Filter Zone"),
        Binding("m", "toggle_mouse", "Mouse"),
    ]

    def __init__(
        self,
        df: pl.DataFrame,
        date_tag: str,
        pos_zone_ids: dict[str, int],
        **kwargs
    ):
        super().__init__(**kwargs)
        self.full_df = df
        self.date_tag = date_tag
        self.pos_zone_ids = pos_zone_ids
        self.current_pos = "POS_1"
        self.current_hour = self._find_first_active_hour()
        self.filter_to_zone = True  # Only show zone events + ACC
        self._occupancy_cache: dict[str, list[dict]] = {}
        self._acc_cache: dict[str, list[dict]] = {}
        self._link_cache: dict[str, dict[int, int]] = {}  # person_id -> group_id
        self.current_acc_idx: int = -1  # Track which ACC we're viewing

    def _find_first_active_hour(self) -> int:
        if self.full_df.is_empty():
            return 12
        hours = self.full_df.select(pl.col("ts_event").dt.hour()).to_series().unique().sort()
        return hours[0] if len(hours) > 0 else 12

    def _get_occupancy_data(self) -> list[dict]:
        """Get occupancy-annotated events for current POS (cached)."""
        if self.current_pos not in self._occupancy_cache:
            self._occupancy_cache[self.current_pos] = compute_zone_occupancy(
                self.full_df, self.current_pos
            )
        return self._occupancy_cache[self.current_pos]

    def _get_person_group_links(self) -> dict[int, int]:
        """Get person->group ID links for current POS (cached)."""
        if self.current_pos not in self._link_cache:
            self._link_cache[self.current_pos] = build_person_group_links(
                self.full_df, self.current_pos
            )
        return self._link_cache[self.current_pos]

    def compose(self) -> ComposeResult:
        yield Header()
        yield Static(id="header-info")
        yield DataTable()
        yield Footer()

    def on_mount(self) -> None:
        table = self.query_one(DataTable)
        table.add_columns(
            "Time", "Event", "Track", "In Zone (after)", "Details"
        )
        table.cursor_type = "row"
        self._refresh_view()

    def _get_filtered_events(self) -> list[dict]:
        """Get events filtered by hour and optionally by zone relevance."""
        all_events = self._get_occupancy_data()

        # Filter by hour
        filtered = []
        for row in all_events:
            ts = row["ts_event"]
            if ts and ts.hour == self.current_hour:
                filtered.append(row)

        if self.filter_to_zone:
            # Find tracks that appear in this POS zone (from zone events)
            tracks_in_zone = set()
            for r in filtered:
                if r["is_zone_event"] and r["track_id"] is not None:
                    tracks_in_zone.add(r["track_id"])

            # Show: zone entry/exit for this POS, ACC for this POS,
            # or TRACK_CREATE/DELETE for tracks seen in zone
            filtered = [
                r for r in filtered
                if r["is_zone_event"]
                or (r["event_type"] == "ACC" and r["zone_name"] == self.current_pos)
                or (r["event_type"] in ("TRACK_CREATE", "TRACK_DELETE")
                    and r["track_id"] in tracks_in_zone)
            ]

        return filtered

    def _format_track_id(self, track_id: int | None, show_link: bool = False) -> str:
        """Format a track ID with P:/G: prefix and optional link."""
        if track_id is None:
            return ""
        links = self._get_person_group_links()
        is_group = track_id >= GROUP_ID_BASE
        if is_group:
            return f"G:{track_id - GROUP_ID_BASE}"
        else:
            base = f"P:{track_id}"
            if show_link and track_id in links:
                linked_group = links[track_id] - GROUP_ID_BASE
                return f"{base}→G:{linked_group}"
            return base

    def _format_in_zone(self, active_tracks: set[int]) -> str:
        """Format active tracks showing person/group pairs."""
        if not active_tracks:
            return "(empty)"

        links = self._get_person_group_links()
        persons = sorted(t for t in active_tracks if t < GROUP_ID_BASE)
        groups = sorted(t - GROUP_ID_BASE for t in active_tracks if t >= GROUP_ID_BASE)

        # Show linked pairs and unlinked tracks
        parts = []
        shown_groups = set()
        for p in persons:
            if p in links:
                g = links[p] - GROUP_ID_BASE
                parts.append(f"P:{p}+G:{g}")
                shown_groups.add(g)
            else:
                parts.append(f"P:{p}")
        for g in groups:
            if g not in shown_groups:
                parts.append(f"G:{g}")
        return " ".join(parts) if parts else "(empty)"

    def _format_row(self, row: dict) -> tuple[str, str, str, str, str]:
        """Format a row dict into table columns."""
        ts = row["ts_event"]
        time_str = ts.strftime("%H:%M:%S.%f")[:-3] if ts else "---"

        event_type = row["event_type"] or ""
        event_str = EVENT_DISPLAY.get(event_type, event_type)
        track_str = self._format_track_id(row["track_id"], show_link=True)

        active_after = row.get("active_tracks_after", set())
        in_zone_str = self._format_in_zone(active_after)

        details_parts = []
        if row["receipt_id"]:
            details_parts.append(row["receipt_id"])
        if row["height"]:
            details_parts.append(f"h={row['height']:.2f}")
        if row["position"]:
            details_parts.append(f"pos={row['position']}")

        before = row.get("active_tracks_before", set())
        after = row.get("active_tracks_after", set())
        if before != after:
            if added := after - before:
                details_parts.append(f"+[{','.join(self._format_track_id(t) for t in added)}]")
            if removed := before - after:
                details_parts.append(f"-[{','.join(self._format_track_id(t) for t in removed)}]")

        return time_str, event_str, track_str, in_zone_str, " ".join(details_parts)

    def _refresh_view(self) -> None:
        filter_mode = "zone only" if self.filter_to_zone else "all events"
        acc_events = self._get_all_acc_events()

        if acc_events and self.current_acc_idx >= 0:
            acc_info = f"  ACC {self.current_acc_idx + 1}/{len(acc_events)}"
        elif acc_events:
            acc_info = f"  ({len(acc_events)} ACCs)"
        else:
            acc_info = ""

        self.query_one("#header-info", Static).update(
            f"[{self.current_pos}]  {self.date_tag} Hour {self.current_hour:02d}  [{filter_mode}]{acc_info}"
        )

        table = self.query_one(DataTable)
        table.clear()
        for row in self._get_filtered_events():
            table.add_row(*self._format_row(row))

    def action_prev_hour(self) -> None:
        self.current_hour = (self.current_hour - 1) % 24
        self._refresh_view()

    def action_next_hour(self) -> None:
        self.current_hour = (self.current_hour + 1) % 24
        self._refresh_view()

    def action_select_pos(self, pos: str) -> None:
        self.current_pos = pos
        self.current_acc_idx = -1
        self._refresh_view()

    def action_toggle_filter(self) -> None:
        self.filter_to_zone = not self.filter_to_zone
        self._refresh_view()

    def action_toggle_mouse(self) -> None:
        """Toggle mouse capture to allow text selection."""
        self.capture_mouse = not self.capture_mouse
        if self.capture_mouse:
            self.notify("Mouse captured (TUI mode)")
        else:
            self.notify("Mouse released (can select text now)")

    def action_copy_receipt(self) -> None:
        """Copy current ACC receipt ID to clipboard."""
        acc_events = self._get_all_acc_events()
        if not acc_events or self.current_acc_idx < 0:
            self.notify("No ACC selected (use n/p to navigate)", severity="warning")
            return

        acc = acc_events[self.current_acc_idx]
        receipt_id = acc.get("receipt_id")
        if not receipt_id:
            self.notify("No receipt ID", severity="warning")
            return

        try:
            subprocess.run(["pbcopy"], input=receipt_id.encode(), check=True)
            self.notify(f"Copied: {receipt_id}", severity="information")
        except Exception as e:
            self.notify(f"Copy failed: {e}", severity="error")

    def _get_all_acc_events(self) -> list[dict]:
        """Get all ACC events for current POS, sorted by time (cached)."""
        if self.current_pos not in self._acc_cache:
            all_events = self._get_occupancy_data()
            self._acc_cache[self.current_pos] = [
                e for e in all_events
                if e["event_type"] == "ACC" and e["zone_name"] == self.current_pos
            ]
        return self._acc_cache[self.current_pos]

    def _go_to_acc(self, idx: int) -> None:
        """Navigate to ACC event at given index (wraps around)."""
        acc_events = self._get_all_acc_events()
        if not acc_events:
            return

        self.current_acc_idx = idx % len(acc_events)
        acc = acc_events[self.current_acc_idx]
        if ts := acc["ts_event"]:
            self.current_hour = ts.hour
            self._refresh_view()
            self._scroll_to_acc(acc)

    def _find_first_acc_from_hour(self, reverse: bool = False) -> int | None:
        """Find first ACC index at or after/before current hour."""
        acc_events = self._get_all_acc_events()
        if not acc_events:
            return None

        indices = range(len(acc_events) - 1, -1, -1) if reverse else range(len(acc_events))
        compare = (lambda h: h <= self.current_hour) if reverse else (lambda h: h >= self.current_hour)

        for i in indices:
            if (ts := acc_events[i]["ts_event"]) and compare(ts.hour):
                return i
        return len(acc_events) - 1 if reverse else 0

    def action_next_acc(self) -> None:
        """Jump to next ACC event."""
        if not self._get_all_acc_events():
            return
        if self.current_acc_idx < 0:
            idx = self._find_first_acc_from_hour(reverse=False)
            if idx is not None:
                self._go_to_acc(idx)
        else:
            self._go_to_acc(self.current_acc_idx + 1)

    def action_prev_acc(self) -> None:
        """Jump to previous ACC event."""
        if not self._get_all_acc_events():
            return
        if self.current_acc_idx < 0:
            idx = self._find_first_acc_from_hour(reverse=True)
            if idx is not None:
                self._go_to_acc(idx)
        else:
            self._go_to_acc(self.current_acc_idx - 1)

    def _scroll_to_acc(self, target_acc: dict) -> None:
        """Scroll table to specific ACC event."""
        table = self.query_one(DataTable)
        target_ts = target_acc["ts_event"]
        for i, row in enumerate(self._get_filtered_events()):
            if row["event_type"] == "ACC" and row["ts_event"] == target_ts:
                table.move_cursor(row=i)
                return


def main():
    parser = argparse.ArgumentParser(description="POS Timeline Viewer")
    parser.add_argument("--date", default=None, help="Date in YYYYMMDD format")
    parser.add_argument(
        "--log-dir",
        default=".",
        help="Log directory containing acc/ and mqtt/ subdirs",
    )
    args = parser.parse_args()

    # Default to today if no date specified
    date_tag = args.date or datetime.now(timezone.utc).strftime("%Y%m%d")

    # Load data
    log_dir = Path(args.log_dir)
    print(f"Loading data for {date_tag} from {log_dir}...")
    df = load_timeline(log_dir, date_tag, ZONE_NAMES)
    print(f"Loaded {len(df)} events")

    # Run TUI
    app = TimelineViewer(df, date_tag, POS_ZONE_IDS)
    app.run()


if __name__ == "__main__":
    main()
