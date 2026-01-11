#!/usr/bin/env python3
import argparse
import json
from collections import Counter, defaultdict


def load_samples(path):
    """Load samples from JSONL file."""
    with open(path, "r", encoding="utf-8") as handle:
        return [json.loads(line) for line in handle if line.strip()]


def classify_eval(eval_data):
    """Classify test results by which stage failed (B->C->D->E progression)."""
    if not eval_data:
        return "no_eval"
    if eval_data.get("test_e_flicker_merge"):
        return "e_yes"
    if eval_data.get("test_d_focus"):
        return "d_yes_e_no"
    if eval_data.get("test_c_entry_spread"):
        return "c_yes_d_no"
    if eval_data.get("test_b_present_dwell"):
        return "b_yes_c_no"
    return "none"


def summarize(samples, per_category):
    counts = Counter()
    other_pos_durations = []
    other_pos_short = 0
    other_pos_total = 0

    for sample in samples:
        for item in sample.get("shared_group_key_evals", []):
            counts[classify_eval(item.get("evaluation"))] += 1
            evaluation = item.get("evaluation") or {}
            for member in evaluation.get("present_raw", []):
                total_s = member.get("other_pos_total_s")
                if total_s is None:
                    continue
                if total_s <= 0:
                    continue
                other_pos_total += 1
                other_pos_durations.append(total_s)
                if total_s < 2:
                    other_pos_short += 1
    print("Shared ACC key evals:", dict(counts))
    if other_pos_total:
        short_pct = other_pos_short / other_pos_total * 100.0
        print(
            f"other_pos_total_s <2s: {other_pos_short}/{other_pos_total} ({short_pct:.1f}%)"
        )

    for category in per_category:
        printed = 0
        print(f"\nCategory: {category}")
        for sample in samples:
            for item in sample.get("shared_group_key_evals", []):
                evaluation = item.get("evaluation") or {}
                label = classify_eval(evaluation)
                if label != category:
                    continue
                print("---")
                print("cluster", sample.get("cluster_start"), sample.get("cluster_end"))
                print("acc", item.get("acc_ts"), item.get("zone"), item.get("kiosk"))
                print("members", evaluation.get("members"))
                print("entry_spread_raw_s", evaluation.get("entry_spread_raw_s"))
                print("entry_spread_merged_s", evaluation.get("entry_spread_merged_s"))
                print("present_raw", evaluation.get("present_raw"))
                print("present_merged", evaluation.get("present_merged"))
                print("test_b", evaluation.get("test_b_present_dwell"))
                print("test_c", evaluation.get("test_c_entry_spread"))
                print("test_d", evaluation.get("test_d_focus"))
                print("test_e", evaluation.get("test_e_flicker_merge"))
                printed += 1
                if printed >= 3:
                    break
            if printed >= 3:
                break


def main():
    parser = argparse.ArgumentParser(
        description="Analyze exit-window samples for test coverage."
    )
    parser.add_argument("--samples", required=True, help="Samples JSONL from exit report")
    args = parser.parse_args()

    samples = load_samples(args.samples)
    print("Samples:", len(samples))
    summarize(samples, ["b_yes_c_no", "c_yes_d_no", "e_yes"])

    no_shared = sum(1 for sample in samples if not sample.get("shared_group_keys"))
    print("\nClusters with no shared ACC keys in sample:", no_shared)


if __name__ == "__main__":
    main()
