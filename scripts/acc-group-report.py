#!/usr/bin/env python3
"""Summarize ACC group evaluation samples and classify drop reasons."""

import argparse
import json
from collections import Counter, defaultdict

# Variants that use entry spread filtering
ENTRY_SPREAD_VARIANTS = {"test_c_entry_spread", "test_d_focus", "test_e_flicker_merge"}
# Variants that use other POS activity filtering
OTHER_POS_VARIANTS = {"test_d_focus", "test_e_flicker_merge"}


def load_jsonl(path):
    """Load items from a JSONL file."""
    items = []
    with open(path, "r", encoding="utf-8") as handle:
        for line in handle:
            line = line.strip()
            if line:
                items.append(json.loads(line))
    return items


def classify_drop(sample, min_dwell_ms, entry_spread_s):
    """Classify why a baseline group was dropped by a test variant."""
    baseline = sample.get("baseline_member_info") or []
    entry_spread = sample.get("baseline_entry_spread_s")
    variant = sample.get("variant", "")

    if any(not member.get("present_at_acc") for member in baseline):
        return "not_present"
    if any((member.get("dwell_ms") or 0) < min_dwell_ms for member in baseline):
        return "low_dwell"
    if variant in ENTRY_SPREAD_VARIANTS and entry_spread is not None and entry_spread > entry_spread_s:
        return "entry_spread"
    if variant in OTHER_POS_VARIANTS and any(member.get("other_pos_activity") for member in baseline):
        return "other_pos"
    return "unknown"


def summarize_samples(samples, min_dwell_ms, entry_spread_s, sample_size):
    """Print summary statistics and sample details for each variant."""
    by_variant = defaultdict(list)
    for sample in samples:
        by_variant[sample.get("variant", "unknown")].append(sample)

    print(f"Samples: {len(samples)}")

    for variant, items in sorted(by_variant.items()):
        counts = Counter(s.get("kind", "unknown") for s in items)
        print(f"{variant} {dict(counts)}")

        # Classify drop reasons
        reasons = Counter(
            classify_drop(s, min_dwell_ms, entry_spread_s)
            for s in items
            if s.get("kind") == "dropped"
        )
        if reasons:
            print(f"{variant} drop_reasons {dict(reasons)}")

        # Show sample details
        dropped_samples = [s for s in items if s.get("kind") == "dropped"]
        for sample in dropped_samples[:sample_size]:
            print(f"--- {sample.get('acc_ts')} {sample.get('zone')} {sample.get('kiosk')}")
            print(f"baseline_entry_spread_s {sample.get('baseline_entry_spread_s')}")
            print(f"baseline_members {sample.get('baseline_members')}")
            print(f"baseline_member_info {sample.get('baseline_member_info')}")


def summarize_group_member_ids(journeys):
    """Print summary of group_member_ids field across journeys."""
    size_hist = Counter()
    non_empty = 0

    for journey in journeys:
        members = journey.get("group_member_ids") or []
        if members:
            non_empty += 1
            size_hist[len(members)] += 1

    total = len(journeys)
    print(f"Journeys: {total}")
    print(f"group_member_ids non-empty: {non_empty}")
    print(f"group_member_ids empty: {total - non_empty}")
    print(f"group_member_ids size_hist: {dict(size_hist)}")


def main():
    parser = argparse.ArgumentParser(description="Summarize ACC group eval samples.")
    parser.add_argument("--samples", required=True, help="Samples JSONL from acc-group-eval")
    parser.add_argument("--journeys", help="Optional journeys JSONL to summarize group_member_ids")
    parser.add_argument("--min-dwell-ms", type=int, default=7000)
    parser.add_argument("--entry-spread-s", type=int, default=10)
    parser.add_argument("--sample-size", type=int, default=3)
    args = parser.parse_args()

    samples = load_jsonl(args.samples)
    summarize_samples(samples, args.min_dwell_ms, args.entry_spread_s, args.sample_size)

    if args.journeys:
        journeys = load_jsonl(args.journeys)
        summarize_group_member_ids(journeys)


if __name__ == "__main__":
    main()
