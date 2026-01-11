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
    journeys = []
    with open(path, "r", encoding="utf-8") as handle:
        for line in handle:
            line = line.strip()
            if not line:
                continue
            journeys.append(json.loads(line))
    return journeys


def build_raw_sessions(events):
    sessions = []
    open_entries = {}
    for event in events:
        event_type = event.get("type")
        event_ts = parse_ts(event.get("ts"))
        data = event.get("data") or {}
        zone = data.get("zone")
        if not zone or not zone.startswith("POS_"):
            continue
        if event_type == "zone_entry":
            open_entries[zone] = event_ts
        elif event_type == "zone_exit":
            entry = open_entries.pop(zone, None)
            if entry is not None:
                sessions.append([zone, entry, event_ts])
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


def other_pos_activity(sessions, zone, ts, window_s):
    window = timedelta(seconds=window_s)
    for session_zone, start, end in sessions:
        if session_zone == zone:
            continue
        if start is None:
            continue
        if end is None:
            if ts >= start - window:
                return True
        else:
            if start - window <= ts <= end + window:
                return True
    return False


def other_pos_duration_s(sessions, zone, ts, window_s):
    window_start = ts - timedelta(seconds=window_s)
    window_end = ts  # Backward-only window to match runtime behavior
    total = 0.0
    for session_zone, start, end in sessions:
        if session_zone == zone:
            continue
        if start is None:
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


def get_exit_ts(events):
    """Get the last exit_cross timestamp from journey events."""
    exit_ts = None
    for event in events:
        if event.get("type") == "exit_cross":
            ts = parse_ts(event.get("ts"))
            if ts is not None:
                exit_ts = ts
    return exit_ts


def acc_keys(events):
    """Extract ACC event keys from journey events."""
    keys = []
    for event in events:
        if event.get("type") != "acc":
            continue
        data = event.get("data") or {}
        ts = parse_ts(event.get("ts"))
        zone = data.get("zone")
        kiosk = data.get("kiosk")
        if ts and zone and kiosk:
            keys.append({
                "key": (ts.isoformat(), zone, kiosk),
                "ts": ts,
                "zone": zone,
                "kiosk": kiosk,
                "group_size": data.get("group") or 1,
            })
    return keys


def cluster_exits(exits, window_s):
    """Group exits that occur within window_s of each other."""
    if not exits:
        return []
    exits.sort(key=lambda e: e["exit_ts"])
    clusters = []
    current = [exits[0]]
    start_ts = exits[0]["exit_ts"]
    for item in exits[1:]:
        if (item["exit_ts"] - start_ts).total_seconds() <= window_s:
            current.append(item)
        else:
            clusters.append(current)
            current = [item]
            start_ts = item["exit_ts"]
    clusters.append(current)
    return clusters


def main():
    parser = argparse.ArgumentParser(
        description="Analyze exit-time windows and shared ACC grouping."
    )
    parser.add_argument("--input", required=True, help="Journeys JSONL export")
    parser.add_argument("--window-s", type=int, default=10)
    parser.add_argument("--min-dwell-ms", type=int, default=7000)
    parser.add_argument("--entry-spread-s", type=int, default=10)
    parser.add_argument("--other-pos-window-s", type=int, default=30)
    parser.add_argument("--other-pos-min-s", type=int, default=0)
    parser.add_argument("--merge-gap-s", type=int, default=10)
    parser.add_argument("--sample-size", type=int, default=10)
    parser.add_argument("--samples", help="Write sample clusters to JSONL")
    args = parser.parse_args()

    journeys = load_journeys(args.input)
    raw_sessions = {
        j.get("person_id"): build_raw_sessions(j.get("events") or [])
        for j in journeys
    }
    merged_sessions = {
        pid: merge_sessions(sessions, args.merge_gap_s)
        for pid, sessions in raw_sessions.items()
    }
    exits = []
    for journey in journeys:
        exit_ts = get_exit_ts(journey.get("events") or [])
        if exit_ts is None:
            continue
        exits.append(
            {
                "person_id": journey.get("person_id"),
                "exit_ts": exit_ts,
                "acc_keys": acc_keys(journey.get("events") or []),
            }
        )

    clusters = cluster_exits(exits, args.window_s)
    clusters_ge2 = [c for c in clusters if len(c) >= 2]

    shared_group_clusters = 0
    no_shared_group_clusters = 0
    sample_clusters = []
    shared_key_eval = defaultdict(int)
    shared_key_total = 0

    def evaluate_shared_key(acc_ts, zone, members):
        present_raw = []
        for pid in members:
            session = session_at(raw_sessions.get(pid, []), zone, acc_ts)
            if session is None:
                continue
            start, _end = session
            dwell_ms = int((acc_ts - start).total_seconds() * 1000)
            other_pos_total_s = other_pos_duration_s(
                raw_sessions.get(pid, []),
                zone,
                acc_ts,
                args.other_pos_window_s,
            )
            present_raw.append(
                {
                    "person_id": pid,
                    "entry_ts": start.isoformat(),
                    "dwell_ms": dwell_ms,
                    "other_pos_total_s": other_pos_total_s,
                    "other_pos_activity": other_pos_active(
                        other_pos_total_s, args.other_pos_min_s
                    ),
                }
            )

        present_merged = []
        for pid in members:
            session = session_at(merged_sessions.get(pid, []), zone, acc_ts)
            if session is None:
                continue
            start, _end = session
            dwell_ms = int((acc_ts - start).total_seconds() * 1000)
            other_pos_total_s = other_pos_duration_s(
                merged_sessions.get(pid, []),
                zone,
                acc_ts,
                args.other_pos_window_s,
            )
            present_merged.append(
                {
                    "person_id": pid,
                    "entry_ts": start.isoformat(),
                    "dwell_ms": dwell_ms,
                    "other_pos_total_s": other_pos_total_s,
                    "other_pos_activity": other_pos_active(
                        other_pos_total_s, args.other_pos_min_s
                    ),
                }
            )

        def entry_spread(entries):
            if len(entries) < 2:
                return None
            times = [datetime.fromisoformat(e["entry_ts"]) for e in entries]
            return (max(times) - min(times)).total_seconds()

        present_raw_qualified = [
            m for m in present_raw if m["dwell_ms"] >= args.min_dwell_ms
        ]
        present_merged_qualified = [
            m for m in present_merged if m["dwell_ms"] >= args.min_dwell_ms
        ]

        spread_raw = entry_spread(present_raw_qualified)
        spread_merged = entry_spread(present_merged_qualified)

        test_b = len(present_raw_qualified) >= 2
        test_c = test_b and spread_raw is not None and spread_raw <= args.entry_spread_s
        test_d = test_c and not any(
            m["other_pos_activity"] for m in present_raw_qualified
        )
        test_e = (
            len(present_merged_qualified) >= 2
            and spread_merged is not None
            and spread_merged <= args.entry_spread_s
            and not any(m["other_pos_activity"] for m in present_merged_qualified)
        )

        return {
            "members": members,
            "present_raw": present_raw_qualified,
            "present_merged": present_merged_qualified,
            "entry_spread_raw_s": spread_raw,
            "entry_spread_merged_s": spread_merged,
            "test_b_present_dwell": test_b,
            "test_c_entry_spread": test_c,
            "test_d_focus": test_d,
            "test_e_flicker_merge": test_e,
        }

    for cluster in clusters_ge2:
        key_to_members = defaultdict(list)
        for member in cluster:
            for acc in member["acc_keys"]:
                key_to_members[acc["key"]].append(member["person_id"])

        shared_keys = {
            key: members for key, members in key_to_members.items() if len(members) >= 2
        }

        if shared_keys:
            shared_group_clusters += 1
        else:
            no_shared_group_clusters += 1

        cluster_shared_evals = []
        for key, members in shared_keys.items():
            shared_key_total += 1
            acc_ts = parse_ts(key[0])
            zone = key[1]

            evaluation = evaluate_shared_key(acc_ts, zone, members)
            if evaluation["test_b_present_dwell"]:
                shared_key_eval["test_b_present_dwell"] += 1
            if evaluation["test_c_entry_spread"]:
                shared_key_eval["test_c_entry_spread"] += 1
            if evaluation["test_d_focus"]:
                shared_key_eval["test_d_focus"] += 1
            if evaluation["test_e_flicker_merge"]:
                shared_key_eval["test_e_flicker_merge"] += 1
            cluster_shared_evals.append(
                {
                    "acc_ts": acc_ts.isoformat(),
                    "zone": zone,
                    "kiosk": key[2],
                    "evaluation": evaluation,
                }
            )

        if len(sample_clusters) < args.sample_size:
            sample_clusters.append(
                {
                    "window_s": args.window_s,
                    "cluster_start": min(m["exit_ts"] for m in cluster).isoformat(),
                    "cluster_end": max(m["exit_ts"] for m in cluster).isoformat(),
                    "members": [
                        {
                            "person_id": m["person_id"],
                            "exit_ts": m["exit_ts"].isoformat(),
                            "acc_events": [
                                {
                                    "ts": a["ts"].isoformat(),
                                    "zone": a["zone"],
                                    "kiosk": a["kiosk"],
                                    "group_size": a["group_size"],
                                }
                                for a in m["acc_keys"]
                            ],
                        }
                        for m in cluster
                    ],
                    "shared_group_keys": [
                        {"acc_ts": key[0], "zone": key[1], "kiosk": key[2], "members": members}
                        for key, members in shared_keys.items()
                    ],
                    "shared_group_key_evals": cluster_shared_evals,
                }
            )

    print("Journeys:", len(journeys))
    print("Exit events:", len(exits))
    print("Exit clusters:", len(clusters))
    print("Exit clusters (>=2):", len(clusters_ge2))
    print("Clusters with shared ACC group key:", shared_group_clusters)
    print("Clusters with no shared ACC group key:", no_shared_group_clusters)
    if shared_key_total:
        print("Shared ACC keys:", shared_key_total)
        for key in [
            "test_b_present_dwell",
            "test_c_entry_spread",
            "test_d_focus",
            "test_e_flicker_merge",
        ]:
            print(f"{key}:", shared_key_eval.get(key, 0))

    if args.samples:
        with open(args.samples, "w", encoding="utf-8") as handle:
            for sample in sample_clusters:
                handle.write(json.dumps(sample) + "\n")


if __name__ == "__main__":
    main()
