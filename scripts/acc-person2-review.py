#!/usr/bin/env python3
import argparse
import glob
import json
from datetime import datetime, timezone

try:
    import tomllib
except ImportError:  # pragma: no cover - fallback for older Python
    import tomli as tomllib


GROUP_ID_BASE = 2_147_483_648  # 2^31


def parse_ts(ts: str) -> int:
    return int(datetime.fromisoformat(ts.replace("Z", "+00:00")).timestamp() * 1000)


def parse_time_arg(value: str) -> int:
    try:
        return int(value)
    except ValueError:
        return parse_ts(value)


def load_config(path: str) -> dict:
    with open(path, "rb") as f:
        return tomllib.load(f)


def load_acc_events(acc_files, ip_to_pos):
    events = []
    for path in acc_files:
        with open(path, "r") as f:
            for line in f:
                line = line.strip()
                if not line:
                    continue
                rec = json.loads(line)
                ts = rec.get("ts_recv")
                fields = rec.get("fields") or {}
                if not ts:
                    continue
                events.append(
                    {
                        "ts_ms": parse_ts(ts),
                        "ts_recv": ts,
                        "kiosk_ip": fields.get("kiosk_ip") or "unknown",
                        "receipt_id": fields.get("receipt_id"),
                        "pos_zone": fields.get("pos_zone")
                        or ip_to_pos.get(fields.get("kiosk_ip")),
                    }
                )
    return events


def load_zone_events(sensor_file):
    zone_events = []

    with open(sensor_file, "r") as f:
        for line in f:
            line = line.strip()
            if not line:
                continue
            rec = json.loads(line)
            ts_recv = rec.get("ts_recv")
            recv_ms = parse_ts(ts_recv) if ts_recv else None
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
                    etype = ev.get("type")
                    if etype not in ("ZONE_ENTRY", "ZONE_EXIT"):
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
                    zone_events.append((recv_ms, frame_time, etype, track_id, zone_id))

    return zone_events


def filter_zone_events(zone_events, cutoff_ms):
    filtered = []
    for recv_ms, frame_ms, etype, track_id, zone_id in zone_events:
        if recv_ms is not None and recv_ms > cutoff_ms:
            continue
        filtered.append((frame_ms, etype, track_id, zone_id))
    return filtered


def find_late_entries(zone_events, zone_id, acc_time, grace_ms):
    entries = []
    window_end = acc_time + grace_ms
    for recv_ms, frame_ms, etype, track_id, zid in zone_events:
        if etype != "ZONE_ENTRY" or zid != zone_id:
            continue
        late_by_recv = recv_ms is not None and acc_time < recv_ms <= window_end
        late_by_event = acc_time <= frame_ms <= window_end
        if not (late_by_recv or late_by_event):
            continue
        entries.append(
            {
                "track_id": track_id,
                "event_ms": frame_ms,
                "recv_ms": recv_ms,
                "dt_event_ms": frame_ms - acc_time,
                "dt_recv_ms": recv_ms - acc_time if recv_ms is not None else None,
                "late_by_event": late_by_event,
                "late_by_recv": late_by_recv,
            }
        )
    return entries


def build_intervals(zone_events):
    zone_events.sort(key=lambda x: x[0])
    open_intervals = {}
    intervals_by_track = {}

    for t_ms, etype, track_id, zone_id in zone_events:
        key = (track_id, zone_id)
        if etype == "ZONE_ENTRY":
            open_intervals[key] = t_ms
        elif etype == "ZONE_EXIT":
            start = open_intervals.pop(key, None)
            if start is None:
                continue
            intervals_by_track.setdefault(track_id, []).append((start, t_ms, zone_id, True))

    if zone_events:
        last_time = zone_events[-1][0]
        for (track_id, zone_id), start in list(open_intervals.items()):
            intervals_by_track.setdefault(track_id, []).append(
                (start, last_time, zone_id, False)
            )

    return intervals_by_track


def merge_intervals(intervals, merge_window_ms):
    if not intervals:
        return []
    intervals.sort(key=lambda x: x[0])
    merged = [intervals[0]]
    for start, end, zone_id, closed in intervals[1:]:
        p_start, p_end, p_zone, p_closed = merged[-1]
        if zone_id == p_zone and start - p_end <= merge_window_ms:
            merged[-1] = (p_start, max(p_end, end), zone_id, p_closed and closed)
        else:
            merged.append((start, end, zone_id, closed))
    return merged


def overlap(a_start, a_end, b_start, b_end):
    s = max(a_start, b_start)
    e = min(a_end, b_end)
    return max(0, e - s)


def candidates_at(zone_id, acc_time, merged_by_track, lookback_ms, exit_grace_ms):
    candidates = []
    for tid, intervals in merged_by_track.items():
        for start, end, zid, closed in intervals:
            if zid != zone_id:
                continue
            if closed:
                if start <= acc_time <= end + exit_grace_ms:
                    candidates.append(tid)
                    break
            else:
                if start <= acc_time <= start + lookback_ms:
                    candidates.append(tid)
                    break
    return candidates


def compute_metrics(
    tid, intervals, zone_id, acc_time, lookback_ms, prox_ms, exit_grace_ms
):
    pos_intervals = [(s, e, closed) for s, e, zid, closed in intervals if zid == zone_id]
    all_intervals = [(s, e, closed) for s, e, _, closed in intervals]

    window_start = acc_time - lookback_ms
    window_end = acc_time

    def dwell(intervals_list):
        total = 0
        for s, e, closed in intervals_list:
            eff_end = e if closed else min(acc_time, s + lookback_ms)
            total += overlap(s, eff_end, window_start, window_end)
        return total

    pos_dwell = dwell(pos_intervals)
    total_dwell = dwell(all_intervals)
    pos_ratio = (pos_dwell / total_dwell) if total_dwell > 0 else 0.0

    status = "none"
    current_stay = None
    last_exit_dt = None
    next_entry_dt = None
    for s, e, closed in pos_intervals:
        if closed:
            in_interval = s <= acc_time <= e
        else:
            in_interval = s <= acc_time <= s + lookback_ms
        if in_interval:
            status = "during"
            current_stay = acc_time - s
            break
    if status != "during":
        for s, e, closed in pos_intervals:
            if closed and e <= acc_time and (
                last_exit_dt is None or e > acc_time + last_exit_dt
            ):
                last_exit_dt = e - acc_time
            if s >= acc_time and (next_entry_dt is None or s < acc_time + next_entry_dt):
                next_entry_dt = s - acc_time
        if last_exit_dt is not None and abs(last_exit_dt) <= exit_grace_ms:
            status = "exit_grace"
        elif next_entry_dt is not None and abs(next_entry_dt) <= prox_ms:
            status = "just_after"

    return {
        "pos_dwell": pos_dwell,
        "total_dwell": total_dwell,
        "pos_ratio": pos_ratio,
        "status": status,
        "current_stay": current_stay,
        "last_exit_dt": last_exit_dt,
        "next_entry_dt": next_entry_dt,
    }


def aligned_timeline(zone_events, tids, acc_time, zone_id_to_name, window_minutes):
    rows = {}

    def add_event(t_ms, tid, label):
        dt_sec = int((t_ms - acc_time) / 1000)
        rows.setdefault(
            dt_sec,
            {"left": [], "right": [], "acc": False, "time_ms": acc_time + dt_sec * 1000},
        )
        if tid == tids[0]:
            rows[dt_sec]["left"].append(label)
        else:
            rows[dt_sec]["right"].append(label)

    rows.setdefault(0, {"left": [], "right": [], "acc": True, "time_ms": acc_time})
    rows[0]["acc"] = True

    start = acc_time - window_minutes * 60 * 1000
    end = acc_time + window_minutes * 60 * 1000

    for t_ms, etype, tid, zid in zone_events:
        if tid not in tids:
            continue
        if t_ms < start or t_ms > end:
            continue
        label = f"{etype} {zone_id_to_name.get(zid, zid)}"
        add_event(t_ms, tid, label)

    return rows


def main():
    parser = argparse.ArgumentParser(description="Review person=2 ACC matches.")
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
    parser.add_argument("--proximity-ms", type=int, default=5_000)
    parser.add_argument("--merge-window-ms", type=int, default=None)
    parser.add_argument("--grace-ms", type=int, default=3_000)
    parser.add_argument("--exit-grace-ms", type=int, default=1_500)
    parser.add_argument("--window-minutes", type=int, default=3)
    parser.add_argument(
        "--late-report",
        default=None,
        help="Write JSONL with ACCs that gain candidates within grace window.",
    )
    parser.add_argument("--who-pos", default=None, help="POS name (e.g. POS_5)")
    parser.add_argument(
        "--who-time",
        default=None,
        help="ISO8601 timestamp or epoch ms (UTC) for who-was-in query",
    )
    parser.add_argument("--ignore-first-acc", action="store_true", default=True)
    parser.add_argument("--no-ignore-first-acc", action="store_false", dest="ignore_first_acc")
    parser.add_argument("--list", action="store_true", help="List all person=2 ACCs")
    parser.add_argument("--list-person1", action="store_true", help="List all person=1 ACCs")
    parser.add_argument("--list-person3p", action="store_true", help="List all person>=3 ACCs")
    parser.add_argument("--index", type=int, default=None, help="Index into person=2 list")
    parser.add_argument("--receipt", default=None, help="Receipt id to inspect")
    args = parser.parse_args()

    who_mode = bool(args.who_pos or args.who_time)
    if who_mode and not (args.who_pos and args.who_time):
        raise SystemExit("who mode requires both --who-pos and --who-time")

    date_tag = args.date
    if not date_tag:
        date_tag = datetime.now(timezone.utc).strftime("%Y%m%d")

    cfg = load_config(args.config)
    ip_to_pos = cfg.get("acc", {}).get("ip_to_pos", {}) or {}
    zone_names = cfg.get("zones", {}).get("names", {}) or {}
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

    merge_window_ms = args.merge_window_ms
    if merge_window_ms is None:
        merge_window_s = cfg.get("acc", {}).get("flicker_merge_s", 10)
        merge_window_ms = int(merge_window_s * 1000)

    acc_dir = f"{args.log_dir}/acc"
    mqtt_dir = f"{args.log_dir}/mqtt"
    acc_files = sorted(glob.glob(f"{acc_dir}/*-{date_tag}.jsonl"))
    if not acc_files and not who_mode:
        raise SystemExit(f"No ACC logs found for {date_tag} in {acc_dir}")

    sensor_file = f"{mqtt_dir}/xovis-sensor-{date_tag}.jsonl"
    if not glob.glob(sensor_file):
        raise SystemExit(f"Missing Xovis file: {sensor_file}")

    acc_events = []
    if acc_files:
        acc_events = load_acc_events(acc_files, ip_to_pos)
        acc_events.sort(key=lambda x: x["ts_ms"])
        if args.ignore_first_acc and acc_events:
            acc_events = acc_events[1:]

    zone_events = load_zone_events(sensor_file)

    def merged_by_track_for(cutoff_ms):
        filtered = filter_zone_events(zone_events, cutoff_ms)
        intervals_by_track = build_intervals(filtered)
        merged_by_track = {}
        for tid, intervals in intervals_by_track.items():
            if tid >= GROUP_ID_BASE:
                continue
            merged_by_track[tid] = merge_intervals(intervals, merge_window_ms)
        return merged_by_track, filtered

    if who_mode:
        zone_id = pos_name_to_zone_id.get(args.who_pos)
        if zone_id is None:
            raise SystemExit(f"Unknown POS: {args.who_pos}")
        acc_time = parse_time_arg(args.who_time)
        initial_cutoff = acc_time
        corrected_cutoff = acc_time + args.grace_ms
        merged_initial, _filtered_initial = merged_by_track_for(initial_cutoff)
        merged_corrected, _filtered_corrected = merged_by_track_for(corrected_cutoff)
        cands_initial = candidates_at(
            zone_id, acc_time, merged_initial, args.lookback_ms, args.exit_grace_ms
        )
        cands_corrected = candidates_at(
            zone_id, acc_time, merged_corrected, args.lookback_ms, args.exit_grace_ms
        )
        print(f"who_pos: {args.who_pos} zone_id={zone_id}")
        print(f"time: {args.who_time} ({acc_time})")
        print(
            f"lookback_ms: {args.lookback_ms} grace_ms: {args.grace_ms} "
            f"exit_grace_ms: {args.exit_grace_ms}"
        )
        print(f"initial candidates: {sorted(cands_initial)}")
        print(f"corrected candidates: {sorted(cands_corrected)}")
        added = sorted(set(cands_corrected) - set(cands_initial))
        removed = sorted(set(cands_initial) - set(cands_corrected))
        if added or removed:
            print(f"added: {added}")
            print(f"removed: {removed}")
            late_entries = find_late_entries(
                zone_events, zone_id, acc_time, args.grace_ms
            )
            late_added = [e for e in late_entries if e["track_id"] in added]
            if late_added:
                print("late_entries_for_added:")
                for entry in late_added:
                    print(json.dumps(entry))
        return

    # Build lists by candidate count
    pairs = []
    singles = []
    triples = []
    summary_initial = {"acc_total": 0, "person_0": 0, "person_1": 0, "person_2": 0, "person_3p": 0}
    summary_corrected = {
        "acc_total": 0,
        "person_0": 0,
        "person_1": 0,
        "person_2": 0,
        "person_3p": 0,
    }
    changed = 0
    cache = {}
    late_report = None
    if args.late_report:
        late_report = open(args.late_report, "w")
    for acc in acc_events:
        pos = acc["pos_zone"]
        zone_id = pos_name_to_zone_id.get(pos)
        if zone_id is None:
            continue
        acc_time = acc["ts_ms"]
        initial_cutoff = acc_time
        corrected_cutoff = acc_time + args.grace_ms

        if initial_cutoff not in cache:
            cache[initial_cutoff] = merged_by_track_for(initial_cutoff)
        merged_initial, _filtered_initial = cache[initial_cutoff]
        cands_initial = candidates_at(
            zone_id, acc_time, merged_initial, args.lookback_ms, args.exit_grace_ms
        )

        if corrected_cutoff not in cache:
            cache[corrected_cutoff] = merged_by_track_for(corrected_cutoff)
        merged_corrected, filtered_corrected = cache[corrected_cutoff]
        cands_corrected = candidates_at(
            zone_id, acc_time, merged_corrected, args.lookback_ms, args.exit_grace_ms
        )

        summary_initial["acc_total"] += 1
        summary_corrected["acc_total"] += 1
        if len(cands_initial) == 0:
            summary_initial["person_0"] += 1
        elif len(cands_initial) == 1:
            summary_initial["person_1"] += 1
        elif len(cands_initial) == 2:
            summary_initial["person_2"] += 1
        else:
            summary_initial["person_3p"] += 1

        if len(cands_corrected) == 0:
            summary_corrected["person_0"] += 1
        elif len(cands_corrected) == 1:
            summary_corrected["person_1"] += 1
            singles.append((acc, zone_id, cands_corrected[0], acc_time, cands_initial))
        elif len(cands_corrected) == 2:
            summary_corrected["person_2"] += 1
            pairs.append(
                (acc, zone_id, sorted(cands_corrected), acc_time, cands_initial)
            )
        else:
            summary_corrected["person_3p"] += 1
            triples.append(
                (acc, zone_id, sorted(cands_corrected), acc_time, cands_initial)
            )

        if set(cands_initial) != set(cands_corrected):
            changed += 1
            if late_report:
                late_entries = find_late_entries(
                    zone_events, zone_id, acc_time, args.grace_ms
                )
                added = sorted(set(cands_corrected) - set(cands_initial))
                removed = sorted(set(cands_initial) - set(cands_corrected))
                late_for_added = [e for e in late_entries if e["track_id"] in added]
                rec = {
                    "receipt_id": acc.get("receipt_id"),
                    "ts_recv": acc.get("ts_recv"),
                    "pos_zone": acc.get("pos_zone"),
                    "kiosk_ip": acc.get("kiosk_ip"),
                    "acc_recv_ms": acc_time,
                    "grace_ms": args.grace_ms,
                    "initial_candidates": sorted(cands_initial),
                    "corrected_candidates": sorted(cands_corrected),
                    "added_candidates": added,
                    "removed_candidates": removed,
                    "late_entries": late_for_added,
                }
                late_report.write(json.dumps(rec) + "\n")
    if late_report:
        late_report.close()

    if args.list or args.list_person1 or args.list_person3p or (
        args.index is None and args.receipt is None
    ):
        print(f"Date: {date_tag}")
        print(f"merge_window_ms: {merge_window_ms}")
        print(f"grace_ms: {args.grace_ms}")
        print("initial (cutoff=acc_recv):")
        print(f"  acc_total: {summary_initial['acc_total']}")
        print(f"  person_0: {summary_initial['person_0']}")
        print(f"  person_1: {summary_initial['person_1']}")
        print(f"  person_2: {summary_initial['person_2']}")
        print(f"  person_3p: {summary_initial['person_3p']}")
        print("corrected (cutoff=acc_recv+grace):")
        print(f"  acc_total: {summary_corrected['acc_total']}")
        print(f"  person_0: {summary_corrected['person_0']}")
        print(f"  person_1: {summary_corrected['person_1']}")
        print(f"  person_2: {summary_corrected['person_2']}")
        print(f"  person_3p: {summary_corrected['person_3p']}")
        print(f"candidates_changed: {changed}")
        if args.list:
            print("\nPerson=2 ACCs:")
            for idx, (acc, _zone_id, tids, _acc_time, initial) in enumerate(pairs):
                extra = ""
                if set(initial) != set(tids):
                    extra = f" initial={sorted(initial)}"
                print(
                    f"[{idx}] {acc['receipt_id']} {acc['ts_recv']} "
                    f"{acc['pos_zone']} {acc['kiosk_ip']} candidates={tids}{extra}"
                )
        if args.list_person1:
            print("\nPerson=1 ACCs:")
            for acc, _zone_id, tid, _acc_time, initial in singles:
                extra = ""
                if set(initial) != {tid}:
                    extra = f" initial={sorted(initial)}"
                print(
                    f"{acc['receipt_id']} {acc['ts_recv']} {acc['pos_zone']} "
                    f"{acc['kiosk_ip']} candidate={tid}{extra}"
                )
        if args.list_person3p:
            print("\nPerson>=3 ACCs:")
            for acc, _zone_id, tids, _acc_time, initial in triples:
                extra = ""
                if set(initial) != set(tids):
                    extra = f" initial={sorted(initial)}"
                print(
                    f"{acc['receipt_id']} {acc['ts_recv']} {acc['pos_zone']} "
                    f"{acc['kiosk_ip']} candidates={tids}{extra}"
                )
        if args.index is None and args.receipt is None:
            return

    if args.receipt:
        target_idx = None
        for idx, (acc, _zone_id, _tids, _acc_time, _initial) in enumerate(pairs):
            if acc.get("receipt_id") == args.receipt:
                target_idx = idx
                break
        if target_idx is None:
            raise SystemExit(f"Receipt not found in person=2 list: {args.receipt}")
        acc, zone_id, tids, acc_time, initial = pairs[target_idx]
    else:
        if args.index is None or args.index >= len(pairs):
            raise SystemExit(f"Invalid index {args.index}; available 0..{len(pairs)-1}")
        acc, zone_id, tids, acc_time, initial = pairs[args.index]

    corrected_cutoff = acc_time + args.grace_ms
    merged_corrected, filtered_corrected = merged_by_track_for(corrected_cutoff)

    metrics = {}
    for tid in tids:
        intervals = merged_corrected.get(tid, [])
        metrics[tid] = compute_metrics(
            tid,
            intervals,
            zone_id,
            acc_time,
            args.lookback_ms,
            args.proximity_ms,
            args.exit_grace_ms,
        )

    print(
        f"\nACC: {acc['receipt_id']} {acc['ts_recv']} {acc['pos_zone']} {acc['kiosk_ip']}"
    )
    print(f"candidates (corrected): {tids}")
    if set(initial) != set(tids):
        print(f"candidates (initial): {sorted(initial)}")
    print(
        f"lookback_ms: {args.lookback_ms} proximity_ms: {args.proximity_ms} "
        f"exit_grace_ms: {args.exit_grace_ms}\n"
    )

    for tid in tids:
        m = metrics[tid]
        print(f"Track {tid}")
        print(f"  pos_dwell_last{args.lookback_ms}ms: {m['pos_dwell']}")
        print(f"  total_dwell_last{args.lookback_ms}ms: {m['total_dwell']}")
        print(f"  pos_ratio: {m['pos_ratio']:.3f}")
        print(f"  proximity: {m['status']}")
        if m["current_stay"] is not None:
            print(f"  current_stay_ms: {m['current_stay']}")
        if m["last_exit_dt"] is not None:
            print(f"  last_exit_dt_ms: {m['last_exit_dt']}")
        if m["next_entry_dt"] is not None:
            print(f"  next_entry_dt_ms: {m['next_entry_dt']}")
        print("")

    rows = aligned_timeline(
        filtered_corrected, tids, acc_time, zone_id_to_name, args.window_minutes
    )
    print("Aligned timeline (by second, events only):")
    print(f"time | dt_s | ACC | Track {tids[0]} | Track {tids[1]}")
    print("-----+------+-----+----------+----------")
    for dt_sec in sorted(rows.keys()):
        row = rows[dt_sec]
        ts = datetime.fromtimestamp(row["time_ms"] / 1000, tz=timezone.utc).isoformat()
        acc_flag = "ACC" if row.get("acc") else ""
        left = "; ".join(row["left"])
        right = "; ".join(row["right"])
        print(f"{ts} | {dt_sec:4d} | {acc_flag:3s} | {left} | {right}")


if __name__ == "__main__":
    main()
