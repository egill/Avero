#!/usr/bin/env python3
"""Evaluate ACC grouping variants offline against journey data."""
# TODO: Functions merge_sessions(), build_raw_sessions(), session_at(), other_pos_activity()
# are duplicated in acc-group-diff.py and acc-exit-window-report.py. Consider extracting
# to a shared acc_utils.py module.

import argparse
import json
from collections import Counter, defaultdict
from datetime import datetime, timedelta


def parse_ts(ts):
    """Parse ISO timestamp, handling Z suffix."""
    if ts is None:
        return None
    if ts.endswith("Z"):
        ts = ts[:-1] + "+00:00"
    return datetime.fromisoformat(ts)


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
        """Check if there's activity in another POS zone during the given time range."""
        for other_zone, zone_sessions in by_zone.items():
            if other_zone == zone:
                continue
            for _, other_start, other_end in zone_sessions:
                if sessions_overlap(start, end, other_start, other_end):
                    return True
        return False

    def should_split(cur_end, nxt_start, zone):
        """Determine if sessions should be split (not merged)."""
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
    """Find the session for a zone at a given timestamp."""
    for session_zone, start, end in sessions:
        if session_zone == zone and start and ts >= start and (end is None or ts <= end):
            return start, end
    return None


def other_pos_activity(sessions, zone, ts, window_s):
    """Check if there's activity in other POS zones within a time window."""
    window = timedelta(seconds=window_s)
    for session_zone, start, end in sessions:
        if session_zone == zone or start is None:
            continue
        if end is None:
            if ts >= start - window:
                return True
        elif start - window <= ts <= end + window:
            return True
    return False


def other_pos_duration_s(sessions, zone, ts, window_s):
    """Return total other-POS time within +/- window_s of ts."""
    window_start = ts - timedelta(seconds=window_s)
    window_end = ts + timedelta(seconds=window_s)
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


def build_zone_sessions(sessions_by_person):
    """Invert sessions_by_person to zone_sessions for efficient lookup."""
    zone_sessions = defaultdict(list)
    for pid, sessions in sessions_by_person.items():
        for zone, start, end in sessions:
            zone_sessions[zone].append((start, end, pid))
    return zone_sessions


def active_in_zone(zone_sessions, zone, ts):
    """Find all people active in a zone at a given timestamp."""
    return [
        (pid, start, end)
        for start, end, pid in zone_sessions.get(zone, [])
        if start is not None and ts >= start and (end is None or ts <= end)
    ]


def evaluate_variant(
    name,
    acc_events,
    zone_sessions,
    sessions_by_person,
    min_dwell_ms,
    entry_spread_s=None,
    other_pos_window_s=None,
    other_pos_min_s=0,
):
    """Evaluate a grouping variant and return detected groups."""
    group_keys = set()
    group_members = {}
    size_hist = Counter()

    for key, acc in acc_events.items():
        acc_ts, zone = acc["ts"], acc["zone"]
        if acc_ts is None or zone is None:
            continue

        # Find candidates: present, sufficient dwell, no other POS activity
        candidates = []
        for pid, start, _end in active_in_zone(zone_sessions, zone, acc_ts):
            dwell_ms = int((acc_ts - start).total_seconds() * 1000)
            if dwell_ms < min_dwell_ms:
                continue
            if other_pos_window_s is not None:
                other_pos_total_s = other_pos_duration_s(
                    sessions_by_person.get(pid, []), zone, acc_ts, other_pos_window_s
                )
                if other_pos_active(other_pos_total_s, other_pos_min_s):
                    continue
            candidates.append((pid, start))

        if len(candidates) < 2:
            continue

        # Check entry spread constraint
        if entry_spread_s is not None:
            entry_times = [entry for _pid, entry in candidates]
            spread = (max(entry_times) - min(entry_times)).total_seconds()
            if spread > entry_spread_s:
                continue

        members = tuple(sorted(pid for pid, _ in candidates))
        group_keys.add(key)
        group_members[key] = members
        size_hist[len(members)] += 1

    return {
        "name": name,
        "group_keys": group_keys,
        "group_members": group_members,
        "size_hist": size_hist,
    }


def summarize_baseline(
    acc_events, sessions_by_person, entry_spread_s, other_pos_window_s, other_pos_min_s
):
    """Summarize baseline groups from ACC events and compute diagnostic stats."""
    group_keys = set()
    group_members = {}
    size_hist = Counter()
    present_lt2 = 0
    entry_spread_gt = 0
    other_pos_hits = 0

    for key, acc in acc_events.items():
        if acc["group_size"] <= 1:
            continue

        group_keys.add(key)
        members = tuple(sorted(acc["members"]))
        group_members[key] = members
        size_hist[len(members)] += 1

        acc_ts, zone = acc["ts"], acc["zone"]
        present = [
            (pid, session[0])
            for pid in members
            if (session := session_at(sessions_by_person.get(pid, []), zone, acc_ts)) is not None
        ]

        if len(present) < 2:
            present_lt2 += 1
            continue

        entry_times = [entry for _pid, entry in present]
        spread = (max(entry_times) - min(entry_times)).total_seconds()
        if spread > entry_spread_s:
            entry_spread_gt += 1

        other_pos_flag = False
        for pid, _ in present:
            other_pos_total_s = other_pos_duration_s(
                sessions_by_person.get(pid, []), zone, acc_ts, other_pos_window_s
            )
            if other_pos_active(other_pos_total_s, other_pos_min_s):
                other_pos_flag = True
                break
        if other_pos_flag:
            other_pos_hits += 1

    return {
        "name": "baseline",
        "group_keys": group_keys,
        "group_members": group_members,
        "size_hist": size_hist,
        "present_lt2": present_lt2,
        "entry_spread_gt": entry_spread_gt,
        "other_pos_hits": other_pos_hits,
    }


def format_hist(hist):
    """Format a histogram as a comma-separated string."""
    return ", ".join(f"{k}:{v}" for k, v in sorted(hist.items()))


def member_info(members, sessions_by_person, zone, acc_ts, other_pos_window_s):
    """Build detailed info for each member in a group."""
    infos = []
    for pid in members:
        session = session_at(sessions_by_person.get(pid, []), zone, acc_ts)
        if session is None:
            info = {"person_id": pid, "present_at_acc": False}
        else:
            start, _end = session
            info = {
                "person_id": pid,
                "present_at_acc": True,
                "entry_ts": start.isoformat() if start else None,
                "dwell_ms": int((acc_ts - start).total_seconds() * 1000),
            }
        if acc_ts and zone:
            other_pos_total_s = other_pos_duration_s(
                sessions_by_person.get(pid, []), zone, acc_ts, other_pos_window_s
            )
            info["other_pos_total_s"] = other_pos_total_s
            info["other_pos_activity"] = other_pos_active(other_pos_total_s, 0)
        infos.append(info)
    return infos


def entry_spread_seconds(infos):
    """Calculate entry time spread for members who were present."""
    entries = [
        datetime.fromisoformat(info["entry_ts"])
        for info in infos
        if info.get("present_at_acc") and info.get("entry_ts")
    ]
    if len(entries) < 2:
        return None
    return (max(entries) - min(entries)).total_seconds()


def load_journeys(path):
    """Load journeys from JSONL file."""
    journeys = {}
    with open(path, "r", encoding="utf-8") as handle:
        for line in handle:
            line = line.strip()
            if not line:
                continue
            obj = json.loads(line)
            journeys[obj["person_id"]] = obj
    return journeys


def extract_acc_events(journeys):
    """Extract and group ACC events by (ts, zone, kiosk)."""
    acc_events = {}
    for pid, journey in journeys.items():
        for event in journey.get("events", []):
            if event.get("type") != "acc":
                continue
            data = event.get("data") or {}
            acc_ts = parse_ts(event.get("ts"))
            zone = data.get("zone")
            kiosk = data.get("kiosk")
            group_size = data.get("group") or 1
            key = (acc_ts.isoformat() if acc_ts else None, zone, kiosk)

            if key not in acc_events:
                acc_events[key] = {
                    "ts": acc_ts,
                    "zone": zone,
                    "kiosk": kiosk,
                    "members": set(),
                    "group_size": group_size,
                }
            acc_events[key]["members"].add(pid)
            acc_events[key]["group_size"] = max(acc_events[key]["group_size"], group_size)
    return acc_events


def build_sample(kind, test_name, acc, baseline, test, sessions_by_variant, other_pos_window_s):
    """Build a sample record for a dropped or added group."""
    baseline_info = member_info(
        baseline["group_members"].get((acc["ts"].isoformat() if acc["ts"] else None, acc["zone"], acc["kiosk"]), []),
        sessions_by_variant["baseline"],
        acc["zone"],
        acc["ts"],
        other_pos_window_s,
    )
    candidate_info = member_info(
        test["group_members"].get((acc["ts"].isoformat() if acc["ts"] else None, acc["zone"], acc["kiosk"]), []),
        sessions_by_variant[test_name],
        acc["zone"],
        acc["ts"],
        other_pos_window_s,
    )
    key = (acc["ts"].isoformat() if acc["ts"] else None, acc["zone"], acc["kiosk"])
    return {
        "kind": kind,
        "variant": test_name,
        "acc_ts": acc["ts"].isoformat() if acc["ts"] else None,
        "zone": acc["zone"],
        "kiosk": acc["kiosk"],
        "baseline_members": sorted(baseline["group_members"].get(key, [])),
        "candidate_members": sorted(test["group_members"].get(key, [])),
        "baseline_member_info": baseline_info,
        "candidate_member_info": candidate_info,
        "baseline_entry_spread_s": entry_spread_seconds(baseline_info),
        "candidate_entry_spread_s": entry_spread_seconds(candidate_info),
    }


def main():
    parser = argparse.ArgumentParser(description="Evaluate ACC grouping variants offline.")
    parser.add_argument("--input", required=True, help="JSONL export from person_journeys")
    parser.add_argument("--min-dwell-ms", type=int, default=7000)
    parser.add_argument("--entry-spread-s", type=int, default=10)
    parser.add_argument("--other-pos-window-s", type=int, default=30)
    parser.add_argument("--other-pos-min-s", type=int, default=0)
    parser.add_argument("--merge-gap-s", type=int, default=10)
    parser.add_argument("--sample-size", type=int, default=15)
    parser.add_argument("--samples", help="Write disagreement samples to JSONL")
    args = parser.parse_args()

    journeys = load_journeys(args.input)

    raw_sessions_by_person = {pid: build_raw_sessions(j["events"]) for pid, j in journeys.items()}
    merged_sessions_by_person = {
        pid: merge_sessions(sessions, args.merge_gap_s)
        for pid, sessions in raw_sessions_by_person.items()
    }

    acc_events = extract_acc_events(journeys)
    zone_sessions_raw = build_zone_sessions(raw_sessions_by_person)
    zone_sessions_merged = build_zone_sessions(merged_sessions_by_person)

    baseline = summarize_baseline(
        acc_events,
        raw_sessions_by_person,
        args.entry_spread_s,
        args.other_pos_window_s,
        args.other_pos_min_s,
    )

    tests = [
        evaluate_variant(
            "test_b_present_dwell",
            acc_events,
            zone_sessions_raw,
            raw_sessions_by_person,
            args.min_dwell_ms,
        ),
        evaluate_variant(
            "test_c_entry_spread",
            acc_events,
            zone_sessions_raw,
            raw_sessions_by_person,
            args.min_dwell_ms,
            entry_spread_s=args.entry_spread_s,
        ),
        evaluate_variant(
            "test_d_focus",
            acc_events,
            zone_sessions_raw,
            raw_sessions_by_person,
            args.min_dwell_ms,
            entry_spread_s=args.entry_spread_s,
            other_pos_window_s=args.other_pos_window_s,
            other_pos_min_s=args.other_pos_min_s,
        ),
        evaluate_variant(
            "test_e_flicker_merge",
            acc_events,
            zone_sessions_merged,
            merged_sessions_by_person,
            args.min_dwell_ms,
            entry_spread_s=args.entry_spread_s,
            other_pos_window_s=args.other_pos_window_s,
            other_pos_min_s=args.other_pos_min_s,
        ),
    ]

    sessions_by_variant = {
        "baseline": raw_sessions_by_person,
        "test_b_present_dwell": raw_sessions_by_person,
        "test_c_entry_spread": raw_sessions_by_person,
        "test_d_focus": raw_sessions_by_person,
        "test_e_flicker_merge": merged_sessions_by_person,
    }

    print(f"Journeys: {len(journeys)}")
    print(f"ACC events: {len(acc_events)}")
    print(f"Baseline groups: {len(baseline['group_keys'])} size_hist {format_hist(baseline['size_hist'])}")
    print(
        f"Baseline present<2: {baseline['present_lt2']} "
        f"entry_spread_gt: {baseline['entry_spread_gt']} "
        f"other_pos_hits: {baseline['other_pos_hits']}"
    )

    all_samples = []
    for test in tests:
        dropped = baseline["group_keys"] - test["group_keys"]
        added = test["group_keys"] - baseline["group_keys"]
        print(
            f"{test['name']} groups {len(test['group_keys'])} "
            f"size_hist {format_hist(test['size_hist'])} "
            f"dropped {len(dropped)} added {len(added)}"
        )

        for key in list(dropped)[: args.sample_size]:
            all_samples.append(
                build_sample("dropped", test["name"], acc_events[key], baseline, test, sessions_by_variant, args.other_pos_window_s)
            )
        for key in list(added)[: args.sample_size]:
            all_samples.append(
                build_sample("added", test["name"], acc_events[key], baseline, test, sessions_by_variant, args.other_pos_window_s)
            )

    if args.samples:
        with open(args.samples, "w", encoding="utf-8") as handle:
            for sample in all_samples:
                handle.write(json.dumps(sample) + "\n")


if __name__ == "__main__":
    main()
