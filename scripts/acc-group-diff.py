#!/usr/bin/env python3
import argparse
import json
from collections import defaultdict
from datetime import datetime, timedelta


def parse_ts(ts):
    if ts is None:
        return None
    if ts.endswith("Z"):
        ts = ts[:-1] + "+00:00"
    return datetime.fromisoformat(ts)


def load_journeys(path):
    """Load journeys from JSONL file, keyed by person_id."""
    journeys = {}
    with open(path, "r", encoding="utf-8") as handle:
        for line in handle:
            if line.strip():
                obj = json.loads(line)
                journeys[obj["person_id"]] = obj
    return journeys


def build_raw_sessions(events):
    """Extract POS zone sessions (entry/exit pairs) from journey events."""
    sessions = []
    open_entries = {}
    for event in events:
        data = event.get("data") or {}
        zone = data.get("zone")
        if not zone or not zone.startswith("POS_"):
            continue
        event_type = event.get("type")
        event_ts = parse_ts(event.get("ts"))
        if event_type == "zone_entry":
            open_entries[zone] = event_ts
        elif event_type == "zone_exit":
            entry = open_entries.pop(zone, None)
            if entry is not None:
                sessions.append([zone, entry, event_ts])
    # Add unclosed sessions
    for zone, entry in open_entries.items():
        sessions.append([zone, entry, None])
    sessions.sort(key=lambda s: s[1])
    return sessions


def sessions_overlap(start1, end1, start2, end2):
    """Check if two time ranges overlap."""
    if start2 is None:
        return False
    if end2 is None:
        return end1 is None or start2 <= end1
    if end1 is None:
        return end2 >= start1
    return start2 <= end1 and end2 >= start1


def merge_sessions(raw_sessions, gap_s):
    """Merge consecutive sessions in same zone if gap is small and no other POS activity between."""
    if not raw_sessions:
        return []

    by_zone = defaultdict(list)
    for session in raw_sessions:
        by_zone[session[0]].append(session)

    def other_pos_between(zone, start, end):
        for other_zone, zone_sessions in by_zone.items():
            if other_zone == zone:
                continue
            for _, other_start, other_end in zone_sessions:
                if sessions_overlap(start, end, other_start, other_end):
                    return True
        return False

    def should_split(cur_end, nxt_start, zone):
        if cur_end is None:
            return True
        gap = (nxt_start - cur_end).total_seconds()
        if gap_s is not None and gap > gap_s:
            return True
        return other_pos_between(zone, cur_end, nxt_start)

    merged = []
    for zone, zone_sessions in by_zone.items():
        zone_sessions.sort(key=lambda s: s[1])
        current = zone_sessions[0][:]
        for nxt in zone_sessions[1:]:
            if should_split(current[2], nxt[1], zone):
                merged.append(current)
                current = nxt[:]
            else:
                current[2] = nxt[2]
        merged.append(current)

    merged.sort(key=lambda s: s[1])
    return merged


def session_at(sessions, zone, ts):
    for session_zone, start, end in sessions:
        if session_zone != zone:
            continue
        if start and ts >= start and (end is None or ts <= end):
            return start, end
    return None


def build_zone_sessions(sessions_by_person):
    zone_sessions = defaultdict(list)
    for pid, sessions in sessions_by_person.items():
        for zone, start, end in sessions:
            zone_sessions[zone].append((start, end, pid))
    return zone_sessions


def active_in_zone(zone_sessions, zone, ts):
    return [
        (pid, start, end)
        for start, end, pid in zone_sessions.get(zone, [])
        if start is not None and ts >= start and (end is None or ts <= end)
    ]


def other_pos_duration_s(sessions, zone, ts, window_s):
    window_start = ts - timedelta(seconds=window_s)
    window_end = ts  # Backward-only window to match runtime behavior
    total = 0.0
    for session_zone, start, end in sessions:
        if session_zone == zone or start is None:
            continue
        session_end = end or window_end
        overlap_start = max(start, window_start)
        overlap_end = min(session_end, window_end)
        if overlap_end > overlap_start:
            total += (overlap_end - overlap_start).total_seconds()
    return total


def other_pos_active(total_s, min_s):
    """Check if other-POS activity exceeds threshold."""
    return total_s > 0.0 if min_s <= 0 else total_s >= min_s


def extract_acc_events(journeys):
    """Extract unique ACC events keyed by (ts, zone, kiosk)."""
    acc_events = {}
    for journey in journeys.values():
        for event in journey.get("events", []):
            if event.get("type") != "acc":
                continue
            data = event.get("data") or {}
            acc_ts = parse_ts(event.get("ts"))
            zone = data.get("zone")
            kiosk = data.get("kiosk")
            if acc_ts is None or zone is None:
                continue
            key = (acc_ts.isoformat(), zone, kiosk)
            acc_events[key] = {"ts": acc_ts, "zone": zone, "kiosk": kiosk}
    return acc_events


def evaluate_focus_groups(
    acc_events,
    zone_sessions,
    sessions_by_person,
    min_dwell_ms,
    entry_spread_s,
    other_pos_window_s,
    other_pos_min_s,
):
    group_keys = set()
    group_members = {}
    for key, acc in acc_events.items():
        acc_ts = acc["ts"]
        zone = acc["zone"]
        candidates = []
        for pid, start, _end in active_in_zone(zone_sessions, zone, acc_ts):
            dwell_ms = int((acc_ts - start).total_seconds() * 1000)
            if dwell_ms < min_dwell_ms:
                continue
            other_pos_total_s = other_pos_duration_s(
                sessions_by_person.get(pid, []), zone, acc_ts, other_pos_window_s
            )
            if other_pos_active(other_pos_total_s, other_pos_min_s):
                continue
            candidates.append((pid, start, other_pos_total_s, dwell_ms))

        if len(candidates) < 2:
            continue

        entry_times = [entry for _pid, entry, _total, _dwell in candidates]
        spread = (max(entry_times) - min(entry_times)).total_seconds()
        if spread > entry_spread_s:
            continue

        members = tuple(sorted(pid for pid, _entry, _total, _dwell in candidates))
        group_keys.add(key)
        group_members[key] = candidates
    return group_keys, group_members


def build_sample(acc, candidates, entry_spread_s):
    members = [
        {
            "person_id": pid,
            "entry_ts": entry.isoformat(),
            "dwell_ms": dwell_ms,
            "other_pos_total_s": other_pos_total_s,
        }
        for pid, entry, other_pos_total_s, dwell_ms in candidates
    ]
    entries = [datetime.fromisoformat(m["entry_ts"]) for m in members]
    spread = None
    if len(entries) >= 2:
        spread = (max(entries) - min(entries)).total_seconds()
    return {
        "acc_ts": acc["ts"].isoformat(),
        "zone": acc["zone"],
        "kiosk": acc["kiosk"],
        "entry_spread_s": spread,
        "entry_spread_limit_s": entry_spread_s,
        "members": members,
    }


def main():
    parser = argparse.ArgumentParser(description="Diff focus strategies by other_pos_min_s.")
    parser.add_argument("--input", required=True, help="Journeys JSONL export")
    parser.add_argument("--min-dwell-ms", type=int, default=7000)
    parser.add_argument("--entry-spread-s", type=int, default=10)
    parser.add_argument("--other-pos-window-s", type=int, default=30)
    parser.add_argument("--other-pos-min-a", type=int, default=0)
    parser.add_argument("--other-pos-min-b", type=int, default=2)
    parser.add_argument("--merge-gap-s", type=int, default=10)
    parser.add_argument("--samples", help="Write added group samples to JSONL")
    parser.add_argument("--sample-size", type=int, default=25)
    args = parser.parse_args()

    journeys = load_journeys(args.input)
    raw_sessions_by_person = {
        pid: build_raw_sessions(j["events"]) for pid, j in journeys.items()
    }
    merged_sessions_by_person = {
        pid: merge_sessions(sessions, args.merge_gap_s)
        for pid, sessions in raw_sessions_by_person.items()
    }

    acc_events = extract_acc_events(journeys)

    zone_sessions_raw = build_zone_sessions(raw_sessions_by_person)
    zone_sessions_merged = build_zone_sessions(merged_sessions_by_person)

    focus_a, members_a = evaluate_focus_groups(
        acc_events,
        zone_sessions_raw,
        raw_sessions_by_person,
        args.min_dwell_ms,
        args.entry_spread_s,
        args.other_pos_window_s,
        args.other_pos_min_a,
    )
    focus_b, members_b = evaluate_focus_groups(
        acc_events,
        zone_sessions_raw,
        raw_sessions_by_person,
        args.min_dwell_ms,
        args.entry_spread_s,
        args.other_pos_window_s,
        args.other_pos_min_b,
    )

    flicker_a, flicker_members_a = evaluate_focus_groups(
        acc_events,
        zone_sessions_merged,
        merged_sessions_by_person,
        args.min_dwell_ms,
        args.entry_spread_s,
        args.other_pos_window_s,
        args.other_pos_min_a,
    )
    flicker_b, flicker_members_b = evaluate_focus_groups(
        acc_events,
        zone_sessions_merged,
        merged_sessions_by_person,
        args.min_dwell_ms,
        args.entry_spread_s,
        args.other_pos_window_s,
        args.other_pos_min_b,
    )

    added_focus = sorted(focus_b - focus_a)
    added_flicker = sorted(flicker_b - flicker_a)

    print("ACC events:", len(acc_events))
    print("focus groups min_a:", len(focus_a), "min_b:", len(focus_b), "added:", len(added_focus))
    print(
        "flicker groups min_a:",
        len(flicker_a),
        "min_b:",
        len(flicker_b),
        "added:",
        len(added_flicker),
    )

    samples = []
    for key in added_focus[: args.sample_size]:
        acc = acc_events[key]
        samples.append(
            {
                "strategy": "focus",
                "other_pos_min_a": args.other_pos_min_a,
                "other_pos_min_b": args.other_pos_min_b,
                "sample": build_sample(acc, members_b.get(key, []), args.entry_spread_s),
            }
        )
    for key in added_flicker[: args.sample_size]:
        acc = acc_events[key]
        samples.append(
            {
                "strategy": "flicker_focus",
                "other_pos_min_a": args.other_pos_min_a,
                "other_pos_min_b": args.other_pos_min_b,
                "sample": build_sample(acc, flicker_members_b.get(key, []), args.entry_spread_s),
            }
        )

    if args.samples:
        with open(args.samples, "w", encoding="utf-8") as handle:
            for sample in samples:
                handle.write(json.dumps(sample) + "\n")


if __name__ == "__main__":
    main()
