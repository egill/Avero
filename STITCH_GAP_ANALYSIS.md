# Netto Gateway POC - Stitch Gap Pattern Analysis

**Analysis Date:** 2026-01-07
**Data Period:** Last 5 hours (10:00-15:00 UTC)
**Total Stitches Analyzed:** 125 successful stitches
**Source:** Gateway-POC systemd journal logs

---

## Executive Summary

**Recommendation: YES, implement multi-stitching support**

Nearly 25% of all successful stitches require multi-stitch capability to handle gaps between track IDs. The maximum gap observed is 13 tracks, but the vast majority (90.4%) can be handled by supporting gaps up to 2. The performance overhead (~1 second per multi-stitch) is acceptable for the consumer experience.

---

## Key Metrics

### Track ID Gap Statistics
- **Maximum gap:** 13 tracks (1 occurrence, 0.8%)
- **Minimum gap:** 1 track (94 occurrences, 75.2%)
- **Average gap:** 1.71 tracks
- **Median gap:** 1 track
- **Mode gap:** 1 track (most common)

### Distribution Summary
| Gap Size | Count | Percentage | Cumulative |
|----------|-------|------------|------------|
| 1        | 94    | 75.2%      | 75.2%      |
| 2        | 19    | 15.2%      | 90.4%      |
| 3        | 1     | 0.8%       | 91.2%      |
| 4        | 3     | 2.4%       | 93.6%      |
| 5        | 2     | 1.6%       | 95.2%      |
| 6        | 1     | 0.8%       | 96.0%      |
| 9        | 2     | 1.6%       | 97.6%      |
| 10       | 2     | 1.6%       | 99.2%      |
| 13       | 1     | 0.8%       | 100.0%     |

---

## Multi-Stitching Requirement Analysis

### Current Breakdown
- **Simple stitches (gap = 1):** 94 stitches (75.2%)
  - Direct stitch between consecutive tracks
  - No intermediate tracks to handle

- **Multi-stitches (gap > 1):** 31 stitches (24.8%)
  - Require linking through intermediate tracks
  - Range: 2 to 13 intermediate tracks

### Coverage Scenarios
| Max Gap Supported | Total Coverage | Additional Stitches |
|------------------|-----------------|-------------------|
| 1 (current)      | 75.2% (94)     | 0                 |
| 2                | 90.4% (113)    | +19               |
| 3                | 91.2% (114)    | +1                |
| 4                | 93.6% (117)    | +3                |
| 5                | 95.2% (119)    | +2                |
| 6                | 96.0% (120)    | +1                |
| 13 (full)        | 100.0% (125)   | +5                |

---

## Timing Analysis

### Time by Gap Size
| Gap | Stitches | Avg Time | Min Time | Max Time | Std Dev | Range |
|-----|----------|----------|----------|----------|---------|-------|
| 1   | 94       | 746 ms   | 30 ms    | 4263 ms  | 890 ms  | 4233 ms |
| 2   | 19       | 1571 ms  | 110 ms   | 3791 ms  | 1194 ms | 3681 ms |
| 3   | 1        | 4188 ms  | 4188 ms  | 4188 ms  | 0 ms    | 0 ms    |
| 4   | 3        | 2083 ms  | 682 ms   | 4169 ms  | 1504 ms | 3487 ms |
| 5   | 2        | 2697 ms  | 2537 ms  | 2857 ms  | 160 ms  | 320 ms  |
| 6   | 1        | 3671 ms  | 3671 ms  | 3671 ms  | 0 ms    | 0 ms    |
| 9   | 2        | 1686 ms  | 74 ms    | 3298 ms  | 1612 ms | 3224 ms |
| 10  | 2        | 1676 ms  | 378 ms   | 2975 ms  | 1298 ms | 2597 ms |
| 13  | 1        | 487 ms   | 487 ms   | 487 ms   | 0 ms    | 0 ms    |

### Performance Comparison

**Simple Stitch (gap = 1):**
- Average: 746 ms
- Range: 30 ms to 4263 ms
- Count: 94 stitches

**Multi-Stitch (gap > 1):**
- Average: 1825 ms
- Range: 74 ms to 4188 ms
- Count: 31 stitches
- **Overhead: +1079 ms (144% increase)**

**Observations:**
- Wide variance in all categories suggests external dependencies (sensor proximity, movement patterns, dwell time)
- Timing is not linear with gap size (gap 13 is fastest at 487ms, gap 3 slowest at 4188ms)
- Suggests gap size is not the primary performance factor; other factors dominate

---

## Multi-Stitch Deep Dive (Gap > 1)

### When Multi-Stitch is Required
- **Frequency:** 31 out of 125 stitches (24.8%)
- **Average gap:** 3.9 intermediate tracks
- **Gap range:** 2 to 13 tracks
- **Time range:** 74 ms to 4188 ms

### Most Common Multi-Stitch Size
- **Gap = 2:** 19 occurrences (61.3% of all multi-stitches)
  - This is the "sweet spot" for implementation
  - Supporting only gap=2 would handle most multi-stitch cases

### Less Common Gaps
- **Gaps 3-6:** 8 occurrences (25.8%)
- **Gaps 9-13:** 4 occurrences (12.9%)

---

## Recommendations

### PRIORITY 1: Implement Multi-Stitch for Gaps 1-2
**Impact:** HIGH | **Effort:** MEDIUM | **Urgency:** HIGH

- Covers 90.4% of all stitches (113 out of 125)
- Supports the most common scenario (gap=2 in 19 cases)
- Reasonable implementation scope:
  - Add state machine for track chain linking
  - Cache intermediate track references
  - Implement sequential linking algorithm

**Estimated Timeline:** 2-3 weeks development + testing

### PRIORITY 2: Extend to Gaps 1-5
**Impact:** MEDIUM | **Effort:** LOW | **Urgency:** MEDIUM

- Increases coverage to 95.2% (119 out of 125)
- Additional 5% coverage with minimal effort
- Leverages existing gap=1,2 infrastructure
- Recommended to avoid technical debt

**Estimated Timeline:** 1 week additional work

### PRIORITY 3: Full Support (Gaps 1-13)
**Impact:** LOW | **Effort:** MEDIUM | **Urgency:** LOW

- Achieves 100% coverage
- Handles edge cases (very rare: 6 occurrences over 5 hours)
- Can be deferred unless edge cases become problematic
- Consider if system scales to higher track creation rates

**Estimated Timeline:** 2-3 weeks, implement later if needed

---

## Implementation Considerations

### Algorithm Strategy
1. **Fast Path:** Gap = 1 (existing sequential stitch)
2. **Common Path:** Gap = 2 (optimized for dual-track linking)
3. **General Path:** Gap > 2 (iterative linking algorithm)

### Data Structure Changes
- Add `track_chain` table or linked list structure
- Index intermediate tracks by (old_track_id, new_track_id) pairs
- Cache stitch candidates for 5-minute window

### Telemetry Recommendations
- Track gap size distribution in production
- Monitor why gaps occur (sensor disconnection, movement speed, etc.)
- Alert if gap sizes exceed expected thresholds (e.g., > 20)
- Measure actual performance impact when implemented

### Performance Expectations
- Multi-stitch will add ~1 second per operation (observed)
- This is acceptable for:
  - Retail environments (customer dwell times are seconds+)
  - Warehouse operations (tracking multiple items)
  - Real-time analytics (1 second latency is negligible)

### Failure Handling
- If chain linking fails, fall back to simple stitch (gap=1)
- Log multi-stitch failures for debugging
- Monitor failure rate to detect systemic issues

---

## Visual Summary

### Gap Distribution (Bar Chart)
```
Gap  1: ████████████████████████████████████████████████████████████  94 ( 75.2%)
Gap  2: ████████████                                                  19 ( 15.2%)
Gap  3:                                                                1 (  0.8%)
Gap  4: █                                                              3 (  2.4%)
Gap  5: █                                                              2 (  1.6%)
Gap  6:                                                                1 (  0.8%)
Gap  9: █                                                              2 (  1.6%)
Gap 10: █                                                              2 (  1.6%)
Gap 13:                                                                1 (  0.8%)
```

### Coverage Improvement
```
Current (gap=1):     ███████████████                    75.2%
With gap≤2:          ██████████████████                 90.4%  [+15.2%]
With gap≤5:          ██████████████████████              95.2%  [+4.8%]
With gap≤13:         ███████████████████████             100.0% [+4.8%]
```

---

## Conclusion

Multi-stitching is **necessary and recommended** for production deployment. The Netto gateway is experiencing track discontinuities in approximately 25% of cases, requiring intermediate track linking. Starting with support for gaps up to 2 provides excellent coverage (90.4%) while keeping implementation scope manageable.

The timing overhead (~1 second) is acceptable given typical retail and warehouse use cases. Extending support to gaps up to 5 is recommended to future-proof the system with minimal additional effort. Full support for gaps up to 13 can be deferred unless operational data shows higher gap frequencies at different locations.

**Next Steps:**
1. Review this analysis with team
2. Design multi-stitch state machine
3. Implement gap=1,2 support
4. Deploy and monitor
5. Extend to gap≤5 based on results
