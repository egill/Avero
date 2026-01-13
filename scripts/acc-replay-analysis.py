#!/usr/bin/env python3
"""
ACC matching analysis using incremental occupancy replay.

Key differences from acc-person2-review.py:
- Uses frame_time (sensor time) instead of ts_recv for Xovis events
- Incremental occupancy tracking instead of pre-built intervals
- Includes both person and group tracks (no GROUP_ID_BASE filter)

This mirrors how the gateway evaluates ACC events in real-time.
"""
import argparse
import csv
import glob
import heapq
import json
from collections import Counter, defaultdict
from dataclasses import dataclass
from datetime import datetime, timezone
from typing import Iterator

try:
    import tomllib
except ImportError:  # pragma: no cover - fallback for older Python
    import tomli as tomllib


@dataclass
class UnifiedEvent:
    """A unified event from either Xovis or ACC source."""
    ts_ms: int
    source: str  # "xovis" or "acc"
    event_type: str  # ZONE_ENTRY, ZONE_EXIT, ACC
    track_id: int | None
    zone_id: int | None
    # ACC-specific fields
    receipt_id: str | None = None
    kiosk_ip: str | None = None
    pos_zone: str | None = None


@dataclass
class RecentExit:
    """A recent exit from a zone."""
    track_id: int
    exit_time: int  # ms


@dataclass
class AccResult:
    """Result of evaluating an ACC event against current occupancy."""
    receipt_id: str
    ts_recv: str
    ts_ms: int
    pos_zone: str
    zone_id: int
    kiosk_ip: str
    candidates: list[int]
    person_count: int
    matched: bool
    # Late auth fields
    grace_candidates: list[int] | None = None  # candidates from exit grace window
    match_type: str = "current"  # "current", "grace", or "none"


def parse_ts(ts: str) -> int:
    """Parse ISO8601 timestamp to epoch milliseconds."""
    return int(datetime.fromisoformat(ts.replace("Z", "+00:00")).timestamp() * 1000)


def ms_to_iso(ts_ms: int) -> str:
    """Convert epoch milliseconds to ISO8601 string."""
    return datetime.fromtimestamp(ts_ms / 1000, tz=timezone.utc).isoformat()


def load_config(path: str) -> dict:
    """Load TOML configuration file."""
    with open(path, "rb") as f:
        return tomllib.load(f)


def stream_xovis_zone_events(sensor_file: str) -> Iterator[UnifiedEvent]:
    """
    Stream ZONE_ENTRY and ZONE_EXIT events from Xovis sensor JSONL.

    Uses frame_time (live_data.frames[].time) as timestamp.
    Includes both person and group tracks.
    """
    with open(sensor_file, "r") as f:
        for line in f:
            line = line.strip()
            if not line:
                continue

            rec = json.loads(line)
            payload = rec.get("payload_raw")
            if not payload:
                continue

            try:
                parsed = json.loads(payload)
            except Exception:
                continue

            live = parsed.get("live_data") or {}
            frames = live.get("frames") or []

            for frame in frames:
                frame_time = frame.get("time")
                if not isinstance(frame_time, int):
                    continue

                for ev in frame.get("events") or []:
                    if ev.get("category") != "SCENE":
                        continue

                    event_type = ev.get("type")
                    if event_type not in ("ZONE_ENTRY", "ZONE_EXIT"):
                        continue

                    attrs = ev.get("attributes") or {}
                    track_id = attrs.get("track_id")
                    zone_id = attrs.get("geometry_id")

                    if track_id is None or zone_id is None:
                        continue

                    try:
                        track_id = int(track_id)
                        zone_id = int(zone_id)
                    except Exception:
                        continue

                    yield UnifiedEvent(
                        ts_ms=frame_time,
                        source="xovis",
                        event_type=event_type,
                        track_id=track_id,
                        zone_id=zone_id,
                    )


def load_acc_events(acc_files: list[str], ip_to_pos: dict) -> list[UnifiedEvent]:
    """Load ACC events with ts_recv converted to milliseconds."""
    events = []

    for path in acc_files:
        with open(path, "r") as f:
            for line in f:
                line = line.strip()
                if not line:
                    continue

                rec = json.loads(line)
                ts_recv = rec.get("ts_recv")
                if not ts_recv:
                    continue

                fields = rec.get("fields") or {}
                ts_ms = parse_ts(ts_recv)
                kiosk_ip = fields.get("kiosk_ip") or "unknown"
                pos_zone = fields.get("pos_zone") or ip_to_pos.get(kiosk_ip)

                events.append(UnifiedEvent(
                    ts_ms=ts_ms,
                    source="acc",
                    event_type="ACC",
                    track_id=None,
                    zone_id=None,
                    receipt_id=fields.get("receipt_id"),
                    kiosk_ip=kiosk_ip,
                    pos_zone=pos_zone,
                ))

    return sorted(events, key=lambda e: e.ts_ms)


def merge_event_streams(
    xovis_iter: Iterator[UnifiedEvent],
    acc_events: list[UnifiedEvent],
) -> Iterator[UnifiedEvent]:
    """
    Merge Xovis stream and ACC list into unified event stream.

    On timestamp ties, Xovis events come before ACC (to ensure
    occupancy is updated before ACC evaluation).
    """
    # Key: (ts_ms, 0 for xovis, 1 for acc)
    def make_key(event: UnifiedEvent) -> tuple[int, int]:
        return (event.ts_ms, 0 if event.source == "xovis" else 1)

    xovis_keyed = ((make_key(e), e) for e in xovis_iter)
    acc_keyed = ((make_key(e), e) for e in acc_events)

    for _, event in heapq.merge(xovis_keyed, acc_keyed, key=lambda x: x[0]):
        yield event


def replay_and_evaluate(
    event_stream: Iterator[UnifiedEvent],
    pos_name_to_zone_id: dict[str, int],
    pos_zone_ids: set[int],
    exit_grace_ms: int = 2500,
    verbose: bool = False,
) -> tuple[list[AccResult], dict]:
    """
    Replay events incrementally, tracking occupancy and evaluating ACCs.

    Args:
        event_stream: Unified event stream (sorted by ts_ms, xovis before acc on ties)
        pos_name_to_zone_id: Mapping of POS zone names to geometry IDs
        pos_zone_ids: Set of zone IDs that are POS zones (for occupancy filtering)
        exit_grace_ms: Grace window for recent exits (default 2500ms)
        verbose: Print per-event progress

    Returns:
        - List of AccResult for each ACC event
        - Statistics dictionary
    """
    # Occupancy state: zone_id -> set of track_ids
    occupancy: dict[int, set[int]] = defaultdict(set)

    # Recent exits: zone_id -> list of RecentExit (track_id, exit_time)
    recent_exits: dict[int, list[RecentExit]] = defaultdict(list)

    results: list[AccResult] = []
    stats = {
        "total_zone_events": 0,
        "total_acc_events": 0,
        "person_counts": Counter(),
        "match_types": Counter(),
        "by_pos": defaultdict(lambda: {"total": 0, "person_0": 0, "grace_matches": 0}),
    }

    for event in event_stream:
        if event.source == "xovis":
            # Only track occupancy for POS zones
            if event.zone_id not in pos_zone_ids:
                continue

            stats["total_zone_events"] += 1

            if event.event_type == "ZONE_ENTRY":
                occupancy[event.zone_id].add(event.track_id)
                if verbose:
                    print(f"[{event.ts_ms}] ENTRY zone={event.zone_id} track={event.track_id} -> {len(occupancy[event.zone_id])} in zone")
            elif event.event_type == "ZONE_EXIT":
                occupancy[event.zone_id].discard(event.track_id)
                # Track recent exit
                recent_exits[event.zone_id].append(RecentExit(
                    track_id=event.track_id,
                    exit_time=event.ts_ms,
                ))
                if verbose:
                    print(f"[{event.ts_ms}] EXIT  zone={event.zone_id} track={event.track_id} -> {len(occupancy[event.zone_id])} in zone")

        elif event.source == "acc":
            stats["total_acc_events"] += 1

            # Resolve zone_id from pos_zone name
            zone_id = pos_name_to_zone_id.get(event.pos_zone)
            if zone_id is None:
                if verbose:
                    print(f"[{event.ts_ms}] ACC {event.receipt_id} pos={event.pos_zone} - unknown zone")
                continue

            # Snapshot current occupancy for this zone
            candidates = sorted(occupancy.get(zone_id, set()))
            person_count = len(candidates)

            # Check recent exits within grace window
            grace_candidates = []
            cutoff = event.ts_ms - exit_grace_ms
            zone_exits = recent_exits.get(zone_id, [])
            for ex in zone_exits:
                if ex.exit_time >= cutoff:
                    grace_candidates.append(ex.track_id)
            grace_candidates = sorted(set(grace_candidates) - set(candidates))  # Only add new ones

            # Determine match type
            if person_count > 0:
                match_type = "current"
                matched = True
            elif grace_candidates:
                match_type = "grace"
                matched = True
            else:
                match_type = "none"
                matched = False

            # Bucket the count (based on current occupancy)
            if person_count >= 3:
                bucket = "person_3+"
            else:
                bucket = f"person_{person_count}"
            stats["person_counts"][bucket] += 1
            stats["match_types"][match_type] += 1

            # Per-POS stats
            stats["by_pos"][event.pos_zone]["total"] += 1
            if person_count == 0:
                stats["by_pos"][event.pos_zone]["person_0"] += 1
            if match_type == "grace":
                stats["by_pos"][event.pos_zone]["grace_matches"] += 1

            if verbose:
                grace_info = f" grace={grace_candidates}" if grace_candidates else ""
                print(f"[{event.ts_ms}] ACC {event.receipt_id} pos={event.pos_zone} zone={zone_id} candidates={candidates}{grace_info} [{match_type}]")

            results.append(AccResult(
                receipt_id=event.receipt_id or "",
                ts_recv=ms_to_iso(event.ts_ms),
                ts_ms=event.ts_ms,
                pos_zone=event.pos_zone or "",
                zone_id=zone_id,
                kiosk_ip=event.kiosk_ip or "",
                candidates=candidates,
                person_count=person_count,
                matched=matched,
                grace_candidates=grace_candidates if grace_candidates else None,
                match_type=match_type,
            ))

            # Prune old exits to prevent memory growth
            if len(zone_exits) > 100:
                recent_exits[zone_id] = [ex for ex in zone_exits if ex.exit_time >= cutoff]

    return results, stats


def print_statistics(stats: dict, results: list[AccResult], date_tag: str, exit_grace_ms: int):
    """Print summary statistics to console."""
    total = stats["total_acc_events"]
    counts = stats["person_counts"]
    match_types = stats["match_types"]

    print(f"\ndate: {date_tag}")
    print(f"exit_grace_ms: {exit_grace_ms}")
    print(f"total_zone_events: {stats['total_zone_events']:,}")
    print(f"total_acc_events: {total}")

    print("\nACC person counts (current occupancy at ACC time):")
    for bucket in ["person_0", "person_1", "person_2", "person_3+"]:
        count = counts.get(bucket, 0)
        pct = (count / total * 100) if total > 0 else 0
        print(f"  {bucket}: {count} ({pct:.1f}%)")

    print("\nMatch types (with exit grace window):")
    for mtype in ["current", "grace", "none"]:
        count = match_types.get(mtype, 0)
        pct = (count / total * 100) if total > 0 else 0
        label = {
            "current": "current occupancy",
            "grace": "exit grace window",
            "none": "unmatched",
        }[mtype]
        marker = " <- late auth rescued!" if mtype == "grace" else ""
        marker = " <- truly unmatched" if mtype == "none" else marker
        print(f"  {mtype}: {count} ({pct:.1f}%) [{label}]{marker}")

    print("\nBy POS zone:")
    for pos in sorted(stats["by_pos"].keys()):
        pos_stats = stats["by_pos"][pos]
        grace_info = f", {pos_stats['grace_matches']} grace" if pos_stats['grace_matches'] else ""
        print(f"  {pos}: {pos_stats['total']} total, {pos_stats['person_0']} person_0{grace_info}")


def write_csv(path: str, results: list[AccResult]):
    """Write detailed results to CSV file."""
    with open(path, "w", newline="") as f:
        writer = csv.writer(f)
        writer.writerow([
            "receipt_id", "ts_recv", "ts_ms", "pos_zone", "zone_id",
            "kiosk_ip", "person_count", "matched", "match_type",
            "candidates", "grace_candidates"
        ])
        for r in results:
            writer.writerow([
                r.receipt_id,
                r.ts_recv,
                r.ts_ms,
                r.pos_zone,
                r.zone_id,
                r.kiosk_ip,
                r.person_count,
                r.matched,
                r.match_type,
                ",".join(str(c) for c in r.candidates),
                ",".join(str(c) for c in (r.grace_candidates or [])),
            ])
    print(f"\nWrote {len(results)} results to {path}")


def main():
    parser = argparse.ArgumentParser(
        description="ACC matching analysis with incremental occupancy replay"
    )
    parser.add_argument("--date", default=None, help="YYYYMMDD (default: today UTC)")
    parser.add_argument(
        "--log-dir",
        default="./gateway-analysis",
        help="Base log dir (contains acc/ and mqtt/)",
    )
    parser.add_argument(
        "--config",
        default="./config/netto.toml",
        help="Config path (zones + acc mapping)",
    )
    parser.add_argument("--csv", default=None, help="Output CSV path")
    parser.add_argument("--exit-grace-ms", type=int, default=2500,
                        help="Grace window for recent exits (default: 2500ms)")
    parser.add_argument("--ignore-first-acc", action="store_true", default=True)
    parser.add_argument("--no-ignore-first-acc", action="store_false", dest="ignore_first_acc")
    parser.add_argument("--verbose", action="store_true", help="Show per-event progress")
    args = parser.parse_args()

    # Determine date
    date_tag = args.date
    if not date_tag:
        date_tag = datetime.now(timezone.utc).strftime("%Y%m%d")

    # Load config
    cfg = load_config(args.config)
    ip_to_pos = cfg.get("acc", {}).get("ip_to_pos", {}) or {}
    zone_names = cfg.get("zones", {}).get("names", {}) or {}
    pos_zones_list = cfg.get("zones", {}).get("pos_zones", []) or []

    # Build zone mappings
    pos_name_to_zone_id = {}
    zone_id_to_name = {}
    for k, v in zone_names.items():
        try:
            zid = int(k)
        except Exception:
            continue
        zone_id_to_name[zid] = v
        if isinstance(v, str) and v.startswith("POS_"):
            pos_name_to_zone_id[v] = zid

    # Set of POS zone IDs (for filtering occupancy tracking)
    pos_zone_ids = set(pos_zones_list)

    # Locate files
    acc_dir = f"{args.log_dir}/acc"
    mqtt_dir = f"{args.log_dir}/mqtt"
    acc_files = sorted(glob.glob(f"{acc_dir}/*-{date_tag}.jsonl"))
    sensor_file = f"{mqtt_dir}/xovis-sensor-{date_tag}.jsonl"

    if not acc_files:
        raise SystemExit(f"No ACC logs found for {date_tag} in {acc_dir}")
    if not glob.glob(sensor_file):
        raise SystemExit(f"Missing Xovis file: {sensor_file}")

    print(f"Loading ACC events from {len(acc_files)} files...")
    acc_events = load_acc_events(acc_files, ip_to_pos)
    acc_events.sort(key=lambda e: e.ts_ms)

    if args.ignore_first_acc and acc_events:
        print(f"Ignoring first ACC event (startup artifact)")
        acc_events = acc_events[1:]

    print(f"Loaded {len(acc_events)} ACC events")
    print(f"Streaming Xovis zone events from {sensor_file}...")

    # Create event streams
    xovis_stream = stream_xovis_zone_events(sensor_file)

    # Merge and replay
    event_stream = merge_event_streams(xovis_stream, acc_events)
    results, stats = replay_and_evaluate(
        event_stream, pos_name_to_zone_id, pos_zone_ids,
        exit_grace_ms=args.exit_grace_ms, verbose=args.verbose
    )

    # Output statistics
    print_statistics(stats, results, date_tag, args.exit_grace_ms)

    # Optional CSV output
    if args.csv:
        write_csv(args.csv, results)


if __name__ == "__main__":
    main()
