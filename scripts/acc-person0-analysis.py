#!/usr/bin/env python3
import argparse
import csv
import importlib.util
import os
from collections import Counter
from datetime import datetime, timezone


def load_acc_module():
    base_dir = os.path.dirname(os.path.abspath(__file__))
    path = os.path.join(base_dir, "acc-person2-review.py")
    spec = importlib.util.spec_from_file_location("acc_review", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def to_hour(ts_ms):
    return datetime.fromtimestamp(ts_ms / 1000, tz=timezone.utc).strftime("%H")


def overlap(a_start, a_end, b_start, b_end):
    s = max(a_start, b_start)
    e = min(a_end, b_end)
    return max(0, e - s)


def stats(values):
    if not values:
        return None
    values = sorted(values)

    def pct(p):
        i = int(round((p / 100) * (len(values) - 1)))
        return values[i]

    return {
        "min": values[0],
        "p50": pct(50),
        "p90": pct(90),
        "max": values[-1],
    }


def bucket_counts(values, thresholds):
    counts = {}
    for t in thresholds:
        counts[t] = sum(1 for v in values if v <= t)
    return counts


def label_bucket(count):
    return str(count) if count < 3 else "3+"


def main():
    parser = argparse.ArgumentParser(
        description="Analyze person_0 ACCs for timing/traffic patterns."
    )
    parser.add_argument("--date", default=None, help="YYYYMMDD (default: today UTC)")
    parser.add_argument(
        "--log-dir",
        default="/var/log/gateway-analysis",
        help="Base log dir (contains acc/ and mqtt/)",
    )
    parser.add_argument(
        "--config",
        default="/opt/avero/gateway-poc/config/netto.toml",
        help="Config path (zones + acc mapping)",
    )
    parser.add_argument("--lookback-ms", type=int, default=60_000)
    parser.add_argument("--grace-ms", type=int, default=3_000)
    parser.add_argument("--exit-grace-ms", type=int, default=1_500)
    parser.add_argument("--merge-window-ms", type=int, default=None)
    parser.add_argument("--window-ms", type=int, default=60_000)
    parser.add_argument("--ignore-first-acc", action="store_true", default=True)
    parser.add_argument("--no-ignore-first-acc", action="store_false", dest="ignore_first_acc")
    parser.add_argument(
        "--csv",
        default=None,
        help="Optional CSV output path for per-ACC metrics",
    )
    args = parser.parse_args()

    acc = load_acc_module()
    date_tag = args.date or datetime.now(timezone.utc).strftime("%Y%m%d")

    cfg = acc.load_config(args.config)
    zone_names = cfg.get("zones", {}).get("names", {}) or {}
    ip_to_pos = cfg.get("acc", {}).get("ip_to_pos", {}) or {}
    pos_name_to_zone_id = {}
    for k, v in zone_names.items():
        try:
            zid = int(k)
        except Exception:
            continue
        if isinstance(v, str) and v.startswith("POS_"):
            pos_name_to_zone_id[v] = zid

    merge_window_ms = args.merge_window_ms
    if merge_window_ms is None:
        merge_window_s = cfg.get("acc", {}).get("flicker_merge_s", 10)
        merge_window_ms = int(merge_window_s * 1000)

    acc_dir = f"{args.log_dir}/acc"
    acc_files = [
        os.path.join(acc_dir, path)
        for path in os.listdir(acc_dir)
        if path.endswith(f"-{date_tag}.jsonl")
    ]
    if not acc_files:
        raise SystemExit(f"No ACC logs found for {date_tag} in {acc_dir}")

    acc_events = acc.load_acc_events(acc_files, ip_to_pos)
    acc_events.sort(key=lambda x: x["ts_ms"])
    if args.ignore_first_acc and acc_events:
        acc_events = acc_events[1:]

    sensor_file = f"{args.log_dir}/mqtt/xovis-sensor-{date_tag}.jsonl"
    if not os.path.exists(sensor_file):
        raise SystemExit(f"Missing Xovis file: {sensor_file}")

    zone_events = acc.load_zone_events(sensor_file)

    def merged_by_track_for(cutoff_ms):
        filtered = acc.filter_zone_events(zone_events, cutoff_ms)
        intervals_by_track = acc.build_intervals(filtered)
        merged = {}
        for tid, intervals in intervals_by_track.items():
            if tid >= acc.GROUP_ID_BASE:
                continue
            merged[tid] = acc.merge_intervals(intervals, merge_window_ms)
        return merged

    person_0 = []
    cache = {}
    for acc_ev in acc_events:
        pos = acc_ev.get("pos_zone")
        zone_id = pos_name_to_zone_id.get(pos)
        if zone_id is None:
            continue
        acc_time = acc_ev["ts_ms"]
        cutoff = acc_time + args.grace_ms
        if cutoff not in cache:
            cache[cutoff] = merged_by_track_for(cutoff)
        merged = cache[cutoff]
        cands = acc.candidates_at(
            zone_id, acc_time, merged, args.lookback_ms, args.exit_grace_ms
        )
        if not cands:
            person_0.append((acc_ev, zone_id, merged))

    pos_counts = Counter()
    hour_counts = Counter()
    last_exit_before = []
    last_entry_before = []
    next_entry_after = []
    tracks_in_window_counts = Counter()
    entries_in_window_counts = Counter()
    exits_in_window_counts = Counter()

    per_acc_rows = []
    window_start_offset = -args.window_ms
    window_end_offset = args.window_ms

    for acc_ev, zone_id, merged in person_0:
        acc_time = acc_ev["ts_ms"]
        pos_counts[acc_ev.get("pos_zone")] += 1
        hour_counts[to_hour(acc_time)] += 1

        last_entry = None
        next_entry = None
        last_exit = None
        tracks_in_window = set()
        entries_in_window = 0
        exits_in_window = 0

        for tid, intervals in merged.items():
            for start, end, zid, closed in intervals:
                if zid != zone_id:
                    continue
                if start <= acc_time and (last_entry is None or start > last_entry):
                    last_entry = start
                if start > acc_time and (next_entry is None or start < next_entry):
                    next_entry = start
                if closed and end <= acc_time and (last_exit is None or end > last_exit):
                    last_exit = end

                eff_end = end if closed else min(acc_time, start + args.lookback_ms)
                if overlap(
                    start,
                    eff_end,
                    acc_time + window_start_offset,
                    acc_time + window_end_offset,
                ):
                    tracks_in_window.add(tid)
                if abs(start - acc_time) <= args.window_ms:
                    entries_in_window += 1
                if closed and abs(end - acc_time) <= args.window_ms:
                    exits_in_window += 1

        if last_entry is not None:
            last_entry_before.append(acc_time - last_entry)
        if next_entry is not None:
            next_entry_after.append(next_entry - acc_time)
        if last_exit is not None:
            last_exit_before.append(acc_time - last_exit)

        tracks_in_window_counts[min(len(tracks_in_window), 3)] += 1
        entries_in_window_counts[min(entries_in_window, 3)] += 1
        exits_in_window_counts[min(exits_in_window, 3)] += 1

        per_acc_rows.append(
            {
                "receipt_id": acc_ev.get("receipt_id"),
                "ts_recv": acc_ev.get("ts_recv"),
                "pos_zone": acc_ev.get("pos_zone"),
                "kiosk_ip": acc_ev.get("kiosk_ip"),
                "last_entry_ms_before": None
                if last_entry is None
                else acc_time - last_entry,
                "last_exit_ms_before": None if last_exit is None else acc_time - last_exit,
                "next_entry_ms_after": None
                if next_entry is None
                else next_entry - acc_time,
                "tracks_in_window": len(tracks_in_window),
                "entries_in_window": entries_in_window,
                "exits_in_window": exits_in_window,
            }
        )

    thresholds = [1500, 5000, 30000, 60000]
    print(f"date: {date_tag}")
    print(f"person_0_total: {len(person_0)}")
    print("person_0_by_pos:")
    for pos, count in sorted(pos_counts.items()):
        print(f"  {pos}: {count}")
    print("hourly_distribution_utc:")
    for hour, count in sorted(hour_counts.items()):
        print(f"  {hour}: {count}")

    if last_exit_before:
        print("last_exit_ms_before stats:", stats(last_exit_before))
        print("last_exit_ms_before counts:", bucket_counts(last_exit_before, thresholds))
    else:
        print("last_exit_ms_before stats: n/a")

    if last_entry_before:
        print("last_entry_ms_before stats:", stats(last_entry_before))
        print("last_entry_ms_before counts:", bucket_counts(last_entry_before, thresholds))
    else:
        print("last_entry_ms_before stats: n/a")

    if next_entry_after:
        print("next_entry_ms_after stats:", stats(next_entry_after))
        print("next_entry_ms_after counts:", bucket_counts(next_entry_after, thresholds))
    else:
        print("next_entry_ms_after stats: n/a")

    print("tracks_in_window distribution:")
    for count, total in sorted(tracks_in_window_counts.items()):
        print(f"  {label_bucket(count)}: {total}")

    print("entries_in_window (0,1,2,3+):")
    for count, total in sorted(entries_in_window_counts.items()):
        print(f"  {label_bucket(count)}: {total}")

    print("exits_in_window (0,1,2,3+):")
    for count, total in sorted(exits_in_window_counts.items()):
        print(f"  {label_bucket(count)}: {total}")

    if args.csv:
        with open(args.csv, "w", newline="") as f:
            writer = csv.writer(f)
            writer.writerow(
                [
                    "receipt_id",
                    "ts_recv",
                    "pos_zone",
                    "kiosk_ip",
                    "last_entry_ms_before",
                    "last_exit_ms_before",
                    "next_entry_ms_after",
                    "tracks_in_window",
                    "entries_in_window",
                    "exits_in_window",
                ]
            )
            for row in per_acc_rows:
                writer.writerow(
                    [
                        row["receipt_id"],
                        row["ts_recv"],
                        row["pos_zone"],
                        row["kiosk_ip"],
                        row["last_entry_ms_before"],
                        row["last_exit_ms_before"],
                        row["next_entry_ms_after"],
                        row["tracks_in_window"],
                        row["entries_in_window"],
                        row["exits_in_window"],
                    ]
                )


if __name__ == "__main__":
    main()
