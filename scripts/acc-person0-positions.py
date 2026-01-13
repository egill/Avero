#!/usr/bin/env python3
import argparse
import csv
import importlib.util
import json
import os
from collections import Counter, deque
from datetime import datetime, timezone


def load_acc_module():
    base_dir = os.path.dirname(os.path.abspath(__file__))
    path = os.path.join(base_dir, "acc-person2-review.py")
    spec = importlib.util.spec_from_file_location("acc_review", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def parse_time(ts_ms):
    return datetime.fromtimestamp(ts_ms / 1000, tz=timezone.utc).isoformat()


def update_bounds(bounds, zid, pos):
    x, y = pos[0], pos[1]
    if zid not in bounds:
        bounds[zid] = {"min_x": x, "max_x": x, "min_y": y, "max_y": y}
        return
    bounds[zid]["min_x"] = min(bounds[zid]["min_x"], x)
    bounds[zid]["max_x"] = max(bounds[zid]["max_x"], x)
    bounds[zid]["min_y"] = min(bounds[zid]["min_y"], y)
    bounds[zid]["max_y"] = max(bounds[zid]["max_y"], y)


def in_bounds(pos, bounds):
    x, y = pos[0], pos[1]
    return bounds["min_x"] <= x <= bounds["max_x"] and bounds["min_y"] <= y <= bounds["max_y"]


def build_closed_intervals(acc, zone_events):
    intervals_by_track = {}
    intervals = acc.build_intervals(zone_events)
    for tid, items in intervals.items():
        for start, end, zid, closed in items:
            if not closed:
                continue
            intervals_by_track.setdefault(tid, []).append((start, end, zid))
    for tid in intervals_by_track:
        intervals_by_track[tid].sort(key=lambda x: x[0])
    return intervals_by_track


def main():
    parser = argparse.ArgumentParser(
        description="Use tracked object positions to analyze person_0 ACCs."
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
    parser.add_argument(
        "--window-ms",
        type=int,
        default=5_000,
        help="Window around ACC to check positions (default 5000ms)",
    )
    parser.add_argument(
        "--pad-m",
        type=float,
        default=0.2,
        help="Padding to apply around POS bounds (default 0.2m)",
    )
    parser.add_argument(
        "--csv",
        default=None,
        help="Optional CSV output path for per-ACC position summary",
    )
    args = parser.parse_args()

    acc = load_acc_module()
    date_tag = args.date or datetime.now(timezone.utc).strftime("%Y%m%d")

    cfg = acc.load_config(args.config)
    zone_names = cfg.get("zones", {}).get("names", {}) or {}
    ip_to_pos = cfg.get("acc", {}).get("ip_to_pos", {}) or {}
    pos_name_to_zone_id = {}
    zone_id_to_pos = {}
    for k, v in zone_names.items():
        try:
            zid = int(k)
        except Exception:
            continue
        if isinstance(v, str) and v.startswith("POS_"):
            pos_name_to_zone_id[v] = zid
            zone_id_to_pos[zid] = v

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
    closed_intervals = build_closed_intervals(acc, zone_events)

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
            person_0.append(
                {
                    "receipt_id": acc_ev.get("receipt_id"),
                    "ts_recv": acc_ev.get("ts_recv"),
                    "pos_zone": pos,
                    "zone_id": zone_id,
                    "kiosk_ip": acc_ev.get("kiosk_ip"),
                    "acc_time": acc_time,
                }
            )

    if not person_0:
        print(f"date: {date_tag}")
        print("person_0_total: 0")
        return

    # First pass: build POS bounds from tracked objects that are inside zones.
    bounds = {}
    interval_idx = {tid: 0 for tid in closed_intervals}

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
                for obj in frame.get("tracked_objects") or []:
                    tid = obj.get("track_id")
                    pos = obj.get("position")
                    if tid is None or not pos:
                        continue
                    intervals = closed_intervals.get(tid)
                    if not intervals:
                        continue
                    idx = interval_idx.get(tid, 0)
                    while idx < len(intervals) and t_ms > intervals[idx][1]:
                        idx += 1
                    interval_idx[tid] = idx
                    if idx < len(intervals):
                        start, end, zid = intervals[idx]
                        if start <= t_ms <= end:
                            update_bounds(bounds, zid, pos)

    pos_bounds = {}
    for zid, b in bounds.items():
        if zid not in zone_id_to_pos:
            continue
        pos_bounds[zid] = {
            "min_x": b["min_x"] - args.pad_m,
            "max_x": b["max_x"] + args.pad_m,
            "min_y": b["min_y"] - args.pad_m,
            "max_y": b["max_y"] + args.pad_m,
        }

    # Second pass: for each person_0, check if any tracked_object is inside POS bounds.
    person_0.sort(key=lambda x: x["acc_time"])
    active = deque()
    idx = 0
    window_ms = args.window_ms

    for acc_ev in person_0:
        acc_ev["frames_with_presence"] = 0
        acc_ev["total_frames"] = 0
        acc_ev["max_tracks_in_pos"] = 0

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
                while idx < len(person_0) and person_0[idx]["acc_time"] - window_ms <= t_ms:
                    active.append(idx)
                    idx += 1
                while active and person_0[active[0]]["acc_time"] + window_ms < t_ms:
                    active.popleft()
                if not active:
                    continue

                counts_by_zone = Counter()
                for obj in frame.get("tracked_objects") or []:
                    pos = obj.get("position")
                    if not pos:
                        continue
                    for zid, b in pos_bounds.items():
                        if in_bounds(pos, b):
                            counts_by_zone[zid] += 1

                for acc_idx in active:
                    acc_rec = person_0[acc_idx]
                    zid = acc_rec["zone_id"]
                    acc_rec["total_frames"] += 1
                    count = counts_by_zone.get(zid, 0)
                    if count:
                        acc_rec["frames_with_presence"] += 1
                        acc_rec["max_tracks_in_pos"] = max(
                            acc_rec["max_tracks_in_pos"], count
                        )

    with_presence = 0
    max_tracks_dist = Counter()
    by_pos = Counter()
    by_pos_with = Counter()

    for acc_ev in person_0:
        by_pos[acc_ev["pos_zone"]] += 1
        max_tracks_dist[min(acc_ev["max_tracks_in_pos"], 3)] += 1
        if acc_ev["frames_with_presence"] > 0:
            with_presence += 1
            by_pos_with[acc_ev["pos_zone"]] += 1

    print(f"date: {date_tag}")
    print(f"person_0_total: {len(person_0)}")
    print(f"person_0_with_pos_presence: {with_presence}")
    print("person_0_with_pos_presence_by_pos:")
    for pos, count in sorted(by_pos_with.items()):
        print(f"  {pos}: {count} / {by_pos[pos]}")
    print("max_tracks_in_pos distribution (0,1,2,3+):")
    for count, total in sorted(max_tracks_dist.items()):
        label = str(count) if count < 3 else "3+"
        print(f"  {label}: {total}")

    if args.csv:
        with open(args.csv, "w", newline="") as f:
            writer = csv.writer(f)
            writer.writerow(
                [
                    "receipt_id",
                    "ts_recv",
                    "pos_zone",
                    "kiosk_ip",
                    "acc_time",
                    "frames_with_presence",
                    "total_frames",
                    "max_tracks_in_pos",
                ]
            )
            for acc_ev in person_0:
                writer.writerow(
                    [
                        acc_ev["receipt_id"],
                        acc_ev["ts_recv"],
                        acc_ev["pos_zone"],
                        acc_ev["kiosk_ip"],
                        acc_ev["acc_time"],
                        acc_ev["frames_with_presence"],
                        acc_ev["total_frames"],
                        acc_ev["max_tracks_in_pos"],
                    ]
                )


if __name__ == "__main__":
    main()
