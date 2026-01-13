# ACC Notes

## Observation: unmatched ACC 2026-01-11T23:02:07 (POS_4)

Receipt: `T000450SC04000189204` (kiosk `10.120.48.214`, mapped to POS_4 / zone 1002).

Timeline around the ACC (person track only, group ids excluded):
- 23:01:31.280 ZONE_ENTRY POS_4, tid=29507
- 23:02:03.440 ZONE_EXIT  POS_4, tid=29507 (about 2.7s before ACC)
- 23:02:06.118 ACC received (occupancy=0)
- 23:02:03.680 ZONE_ENTRY POS_3, tid=29507
- 23:02:11.600 ZONE_EXIT  POS_3, tid=29507
- 23:02:13.120 LINE_CROSS_BACKWARD ENTRY_1
- 23:02:13.280 ZONE_ENTRY STORE
- 23:02:19.280 ZONE_EXIT STORE
- 23:02:19.280 TRACK_DELETE

Notes:
- This ACC is “unmatched” because the last POS_4 occupant exited ~2.7s before the ACC.
- There were no other POS_4 entries immediately after; the next POS_4 entry was ~+137s later (tid=29522).
- This looks like a timing edge rather than a bad POS mapping.

Implications / next steps:
- Consider a fallback match rule: if the last POS exit is within N seconds (e.g., 3–5s) before ACC, treat it as a possible match.
- When we have more data, measure how often the “last exit within N seconds” rule helps without increasing false positives.

## Review: first person=2 match (2026-01-12)

ACC: `T000450SC01000538763` (POS_1).

Tracks:
- 29740: in POS_1 ~27s before ACC, exits ~1s after ACC.
- 29744: short POS_1 dwell (~3.5s), then quickly leaves to POS_2 and deletes.

Decision notes (from review):
- Prefer 29740 as the likely payer.
- Short POS_1 dwell like 29744 is not a good match signal.
- Longer presence before ACC (tens of seconds) is a strong positive.

## Review: second person=2 match (2026-01-12)

ACC: `T000450SC02000359749` (POS_2).

Tracks:
- 29760: long POS_2 dwell (~109s), covers ACC and exits later.
- 29766: short POS_2 dwell (~6.6s), track appears/disappears quickly around ACC.

Decision notes (from review):
- Prefer 29760 as the likely payer.
- 29766 looks like a track created within the checkout area (missing start/finish); likely a pass-through.
- Save track 29766 for stitching investigation; if stitching shows longer POS_2 presence around ACC, reassess.

## Review: third person=2 match (2026-01-12)

ACC: `T000450SC01000538770` (POS_1).

Tracks (with 60s lookback + 5s proximity):
- 29787: pos_ratio=1.0, pos_dwell=60s, current_stay=114s, stable POS_1 presence.
- 29803: pos_ratio=0.312, pos_dwell~8s, current_stay~0.07s, transient across POS zones.

Decision notes (from review):
- Prefer 29787 as the payer.
- Treat 29803 as noise/pass-through.
- Rule confirmed: if `pos_ratio < 0.5` and `current_stay < 5s`, demote.

## Review: fourth person=2 match (2026-01-12)

ACC: `T000450SC02000359752` (POS_2).

Tracks (with 60s lookback + 5s proximity):
- 29822: pos_ratio=0.835, pos_dwell=22.7s, current_stay=22.7s, stable POS_2 presence.
- 29824: pos_ratio=1.0, pos_dwell=1.4s, current_stay=1.4s, transient right at ACC.

Decision notes (from review):
- Prefer 29822 as the payer.
- Treat 29824 as pass-through.
- Rule confirmed: short `current_stay` (~1–2s) gets demoted even if `pos_ratio=1.0`.

## Review: fifth person=2 match (2026-01-12)

ACC: `T000450SC01000538772` (POS_1).

Tracks (with 60s lookback + 5s proximity):
- 29834: pos_ratio=0.968, pos_dwell=43.5s, current_stay=43.5s, stable POS_1 presence.
- 29838: pos_ratio=0.888, pos_dwell=7.6s, current_stay=7.6s, highly flickery POS_1, short track.

Decision notes (from review):
- Prefer 29834 as the payer.
- 29838 shows jittery pass-through behavior (multiple POS_1 flickers + entry line crosses + quick delete).

## Review: sixth person=2 match (2026-01-12)

ACC: `T000450SC04000189210` (POS_4).

Tracks (with 60s lookback + 5s proximity):
- 29837: pos_ratio=0.913, pos_dwell=21.0s, current_stay=21.0s, stable POS_4 presence.
- 29841: pos_ratio=1.0, pos_dwell=1.5s, current_stay=1.5s, short transient.

Decision notes (from review):
- Prefer 29837 as the payer.
- Treat 29841 as pass-through.

## Review: seventh person=2 match (2026-01-12)

ACC: `T000450SC02000359753` (POS_2).

Tracks (with 60s lookback + 5s proximity):
- 29846: pos_ratio=1.0, pos_dwell=60s, current_stay=69.7s, stable POS_2 presence.
- 29847: pos_ratio=0.246, pos_dwell=13.2s, current_stay=1.0s, very flickery across POS_1/POS_2.

Decision notes (from review):
- Prefer 29846 as the payer.
- Treat 29847 as pass-through.

## Review: eighth person=2 match (2026-01-12)

ACC: `T000450SC02000359754` (POS_2).

Tracks (with 60s lookback + 5s proximity):
- 29859: pos_ratio=0.923, pos_dwell=21.1s, current_stay=21.1s, stable POS_2 presence.
- 29864: pos_ratio=1.0, pos_dwell=0.104s, current_stay=0.104s, ultra-short presence at ACC.

Decision notes (from review):
- Prefer 29859 as the payer.
- Treat 29864 as pass-through.

## Review: ninth person=2 match (2026-01-12)

ACC: `T000450SC04000189213` (POS_4).

Tracks (with 60s lookback + 5s proximity):
- 29866: pos_ratio=1.0, pos_dwell=60s, current_stay=104.6s, stable POS_4 presence.
- 29867: pos_ratio=1.0, pos_dwell=60s, current_stay=77.9s, stable POS_4 presence.

Decision notes (from review):
- Authorize both (two valid payers present).
- No tie-breaker needed when both have long POS dwell.

## Review: tenth person=2 match (2026-01-12)

ACC: `T000450SC02000359755` (POS_2).

Tracks (with 60s lookback + 5s proximity):
- 29881: pos_ratio=0.905, pos_dwell=30.5s, current_stay=30.5s, stable POS_2 presence.
- 29882: pos_ratio=1.0, pos_dwell=1.7s, current_stay=1.7s, short transient.

Decision notes (from review):
- Prefer 29881 as the payer.
- Treat 29882 as pass-through.

## Review: eleventh person=2 match (2026-01-12)

ACC: `T000450SC04000189217` (POS_4).

Tracks (with 60s lookback + 5s proximity):
- 29941: pos_ratio=0.911, pos_dwell=27.1s, current_stay=27.1s, stable POS_4 presence.
- 29943: pos_ratio=1.0, pos_dwell=1.4s, current_stay=1.4s, short transient.

Decision notes (from review):
- Prefer 29941 as the payer.
- Treat 29943 as pass-through.

## Note: group outlives person track (2026-01-12)

ACC: `T000450SC01000538764` (POS_1).

Tracks:
- Person track 29753 (height ~1.77-1.79) created at 07:52:28.286 and deleted at 07:52:47.170.
- Group track 2147504770 (members=1) created at 07:52:30.378 and persisted to exit.

Post-ACC activity (group track 2147504770):
- LINE_CROSS_FORWARD geometry 1010 at 07:52:46.528.
- LINE_CROSS_FORWARD geometry 1006 at 07:52:48.030.
- TRACK_DELETE at 07:52:49.085.

Decision notes (from review):
- Person track ended, but group track continued through exit path.
- This may help recover continuity when person tracks flicker near exit.
