#!/usr/bin/env python3
import argparse
import csv
import importlib.util
import json
import os
from bisect import bisect_left, bisect_right
from collections import Counter, defaultdict
from datetime import datetime, timezone


def load_acc_module():
    base_dir = os.path.dirname(os.path.abspath(__file__))
    path = os.path.join(base_dir, "acc-person2-review.py")
    spec = importlib.util.spec_from_file_location("acc_review", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def parse_time(ts_ms):
    return datetime.fromtimestamp(ts_ms / 1000, tz=timezone.utc).strftime("%H")


def load_frame_times(sensor_file):
    frame_times = []
    zone_event_times = defaultdict(list)
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
            frames = (parsed.get("live_data") or {}).get("frames") or []
            for frame in frames:
                t_ms = frame.get("time")
                if not isinstance(t_ms, int):
                    continue
                frame_times.append(t_ms)
                for ev in frame.get("events") or []:
                    if ev.get("category") != "SCENE":
                        continue
                    if ev.get("type") not in ("ZONE_ENTRY", "ZONE_EXIT"):
                        continue
                    attrs = ev.get("attributes") or {}
                    zid = attrs.get("geometry_id")
                    if zid is None:
                        continue
                    try:
                        zid = int(zid)
                    except Exception:
                        continue
                    zone_event_times[zid].append(t_ms)

    frame_times.sort()
    for zid in zone_event_times:
        zone_event_times[zid].sort()
    return frame_times, zone_event_times


def main():
    parser = argparse.ArgumentParser(description="Summarize person_0 ACCs.")
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
    parser.add_argument(
        "--csv",
        default=None,
        help="Optional CSV output path for person_0 list",
    )
    parser.add_argument(
        "--list",
        action="store_true",
        help="Print person_0 receipt list to stdout",
    )
    parser.add_argument(
        "--frame-window-ms",
        type=int,
        default=2000,
        help="Window to check missing frames around ACC (default 2000ms)",
    )
    parser.add_argument(
        "--frame-window-ms2",
        type=int,
        default=5000,
        help="Second window to check missing frames (default 5000ms)",
    )
    parser.add_argument(
        "--zone-window-ms",
        type=int,
        default=60000,
        help="Window to check missing zone events for POS (default 60000ms)",
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

    sensor_file = f"{args.log_dir}/mqtt/xovis-sensor-{date_tag}.jsonl"
    if not os.path.exists(sensor_file):
        raise SystemExit(f"Missing Xovis file: {sensor_file}")

    zone_events = acc.load_zone_events(sensor_file)
    frame_times, zone_event_times = load_frame_times(sensor_file)

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
            person_0.append((acc_ev, zone_id))

    pos_counts = Counter()
    hour_counts = Counter()
    no_frame_1 = 0
    no_frame_2 = 0
    no_zone_window = 0

    for acc_ev, zone_id in person_0:
        pos_counts[acc_ev.get("pos_zone")] += 1
        hour_counts[parse_time(acc_ev["ts_ms"])] += 1

        acc_time = acc_ev["ts_ms"]
        left = bisect_left(frame_times, acc_time - args.frame_window_ms)
        right = bisect_right(frame_times, acc_time + args.frame_window_ms)
        if right - left == 0:
            no_frame_1 += 1

        left = bisect_left(frame_times, acc_time - args.frame_window_ms2)
        right = bisect_right(frame_times, acc_time + args.frame_window_ms2)
        if right - left == 0:
            no_frame_2 += 1

        events = zone_event_times.get(zone_id, [])
        if events:
            l = bisect_left(events, acc_time - args.zone_window_ms)
            r = bisect_right(events, acc_time + args.zone_window_ms)
            if r - l == 0:
                no_zone_window += 1
        else:
            no_zone_window += 1

    print(f"date: {date_tag}")
    print(f"person_0_total: {len(person_0)}")
    print("person_0_by_pos:")
    for pos, count in sorted(pos_counts.items()):
        print(f"  {pos}: {count}")
    print(f"no_frames_within_{args.frame_window_ms}ms: {no_frame_1}")
    print(f"no_frames_within_{args.frame_window_ms2}ms: {no_frame_2}")
    print(f"no_zone_events_within_{args.zone_window_ms}ms: {no_zone_window}")
    print("hourly_distribution_utc:")
    for hour, count in sorted(hour_counts.items()):
        print(f"  {hour}: {count}")

    if args.list:
        print("\nperson_0_receipts:")
        for acc_ev, _zone_id in person_0:
            print(
                f"{acc_ev.get('receipt_id')} {acc_ev.get('ts_recv')} "
                f"{acc_ev.get('pos_zone')} {acc_ev.get('kiosk_ip')}"
            )

    if args.csv:
        with open(args.csv, "w", newline="") as f:
            writer = csv.writer(f)
            writer.writerow(["receipt_id", "ts_recv", "pos_zone", "kiosk_ip"])
            for acc_ev, _zone_id in person_0:
                writer.writerow(
                    [
                        acc_ev.get("receipt_id"),
                        acc_ev.get("ts_recv"),
                        acc_ev.get("pos_zone"),
                        acc_ev.get("kiosk_ip"),
                    ]
                )


if __name__ == "__main__":
    main()
