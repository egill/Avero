# Position Data Analysis: Gate Zone Tracking

## Summary

Analysis of 332MB of Xovis sensor data (150,844 frames, ~5 hours) demonstrates that position data provides significant advantages over zone/line events alone for understanding customer behavior in the gate zone.

## Data Collection

**Method**: `gateway-analysis` binary subscribed to all MQTT topics, persisting raw Xovis frames to JSONL files.

**Data source**: `/opt/avero/logs/xovis-sensor-*.jsonl` on Netto (100.80.187.3)

**Frame rate**: ~80ms intervals (12.5 fps)

## Geometry Reference

```
GATE_1 zone:  x: 0.0 → 3.5,  y: -0.2 → 2.1  (area in front of gate)
EXIT_1 line:  x: 1.71 → 2.76, y: 2.1         (exit threshold)
```

## Timing Analysis

**Question**: Does position data have the same built-in lag as zone/line events?

**Finding**: Yes. Both come from the same Xovis processing pipeline.

| Metric | Value |
|--------|-------|
| Average lag (recv - event) | 1341ms |
| Min lag | 1222ms |
| Max lag | 1805ms |

**Conclusion**: Position data has ~1.3-1.4s built-in lag, same as zone/line events. No timing advantage from using position data.

## Position vs Zone Event Synchronization

**Question**: Does position data arrive at the same time as zone events, or before/after?

**Finding**: **SAME FRAME** - position and events arrive simultaneously.

| Metric | Value |
|--------|-------|
| GATE_1 ZONE_ENTRY events | 1,014 |
| With position in same frame | 1,014 (100%) |
| Position y at GATE_1 entry | avg 0.35m |

**Key Insight**: Position data doesn't arrive *before* zone events. However, position shows approach trajectory *before* the zone boundary is crossed. We could detect "crossing y=0" before GATE_1 ZONE_ENTRY fires.

**Practical Implication**:
- Using position to detect "crossing y = -0.2" fires at SAME time as GATE_1 ZONE_ENTRY
- Using position to detect "crossing y = -0.5" fires ~300ms EARLIER

This confirms: position data gives us *where* information (which enables earlier virtual triggers), but doesn't arrive *before* zone events.

## Zone/Line Events vs Position Data

| Capability | Zone/Line Events | Position Data |
|------------|------------------|---------------|
| Know when someone enters gate area | ✓ ZONE_ENTRY | ✓ y enters range |
| Know when someone exits gate area | ✓ ZONE_EXIT | ✓ y leaves range |
| Know when someone crosses exit line | ✓ LINE_CROSS_FORWARD | ✓ y > 2.1 |
| Know **where** exactly they are | ✗ | ✓ Continuous (x, y, z) |
| Know **direction** of movement | ✗ | ✓ Calculate dy/dt |
| Know **distance to exit** | ✗ | ✓ 2.1 - y |
| Detect **hesitation** | ✗ | ✓ Direction changes |
| Predict **who exits next** | ✗ | ✓ Sort by y-coordinate |
| Detect **turned back** | ✗ | ✓ max_y > 1.5, end_y < 1.0 |
| **Detect missed exits** | ✗ | ✓ Position y > 2.1 |
| **Identify stitch candidates** | ✗ | ✓ Proximity matching |
| **Pre-emptive prediction** | ✗ | ✓ Trajectory analysis |

## Behavior Classification

Analyzed 962 tracks that passed through gate area:

| Behavior | Count | % | Description |
|----------|-------|---|-------------|
| straight_through | 355 | 37% | Entered, walked to exit, crossed |
| hesitated | 192 | 20% | Spent time moving around, then exited |
| turned_back | 11 | 1% | Approached exit (y > 1.5), then backed off (y < 1.0) |
| wandered | 266 | 28% | Moved around without clear direction |
| (short tracks) | 138 | 14% | < 3 position samples, excluded |

### Classification Logic

```python
# Direction changes = how many times y-movement reversed
if max_y > 2.0 and end_y > 2.0:  # Made it to exit
    if direction_changes <= 1:
        "straight_through"
    else:
        "hesitated"
elif max_y > 1.5 and end_y < 1.0:  # Approached but turned back
    "turned_back"
else:
    "wandered"
```

## Confirmed "Stuck" Scenarios

Found **9 confirmed cases** where:
1. Person A approached exit (y > 1.5m)
2. Person B crossed EXIT_1 within ±5 seconds
3. Person A backed off (end_y < 1.0m)

Example:
```
Track 58098: approached to y=1.98m, backed off to y=0.45m
  Track 58099 crossed EXIT_1 +0.8s relative to approach
```

This confirms the "stuck" scenario exists in real data and position tracking can detect it.

## Stitching Analysis

**Question**: Can position data help with track stitching (reconnecting identity when sensor loses a person)?

**Finding**: Yes. Position proximity strongly indicates stitch candidates.

| Metric | Count |
|--------|-------|
| Potential stitch candidates (gap<5s, dist<2m) | 2,790 |
| In gate area | 786 |

**Examples of stitch candidates in gate area**:
```
58116 → 58117: gap=80ms, dist=0.56m
  del_pos=(-1.18, 1.45), cre_pos=(-1.53, 1.01)

58126 → 2147529095: gap=80ms, dist=0.02m
  del_pos=(-1.68, 1.25), cre_pos=(-1.68, 1.27)
```

**Conclusion**: Position data can verify stitching decisions by checking if delete/create positions are within expected distance.

## Exit Detection (Missed Events)

**Question**: Can position data detect exits when LINE_CROSS events are missed?

**Finding**: Yes. 35% of "missed" exits are detectable via position.

| Metric | Count |
|--------|-------|
| Tracks entered GATE_1, exited zone, but no LINE_CROSS | 159 |
| Of those, position shows exit (y > 2.0) | 56 (35%) |
| Tracks with no ZONE_EXIT but position shows exit | 3 |

**Examples**:
```
Track 58156: max_y=2.83m (EXIT_1 is at y=2.1) - no LINE_CROSS event
Track 58169: max_y=2.90m (EXIT_1 is at y=2.1) - no LINE_CROSS event
```

**Conclusion**: Position data provides reliable exit detection fallback when LINE_CROSS events are missed by the sensor.

## Prediction: First to Gate

**Question**: Can we predict who will exit first based on position?

**Finding**: Yes. Distance-to-exit predicts first exit with 87% accuracy.

| Metric | Value |
|--------|-------|
| Multi-person gate frames analyzed | 1,485 |
| Prediction accuracy (closest exits first) | 87% (47/54) |

**Examples**:
```
Predicted: 58098, Actual: 58098 ✓
  Occupants: [(58098, 1.07m, away), (58101, 1.44m, away)]

Predicted: 58106, Actual: 58106 ✓
  Occupants: [(58106, 0.38m, toward), (2147529069, 0.77m, toward)]
```

**Conclusion**: Sorting occupants by distance-to-exit reliably predicts exit order. Adding direction (toward/away) could improve accuracy further.

## Pre-emptive Gate Opening

**Question**: Can we predict gate entry before ZONE_ENTRY event fires?

**Finding**: Yes. Trajectory analysis can predict entry ~7 seconds early.

| Metric | Value |
|--------|-------|
| Gate entries analyzed | 35 |
| Average early detection | 7,275ms (7.3s) |
| Max early detection | 10,000ms (10s) |
| Min early detection | 560ms |

**Conclusion**: By tracking approach trajectory (y increasing toward gate zone), we can predict entry well before ZONE_ENTRY fires. This could be used for pre-emptive gate opening if authorization is already established.

## Position Data Efficiency

**Sampling strategy**: Every 10th frame (~800ms) is sufficient for behavior classification while reducing data volume 10x.

**Key calculations**:
```python
# Distance to exit
distance = 2.1 - position.y

# Direction (over 400ms window)
dy = position[t+1].y - position[t].y
if dy > 0.05: "toward exit"
elif dy < -0.05: "away from exit"
else: "stationary"

# Velocity
velocity = dy / dt_ms * 1000  # m/s

# ETA to exit
eta = distance / velocity  # seconds
```

## Recommendations for GateZoneTracker

### Core Data Structure

```rust
struct GateOccupant {
    track_id: i64,
    position: (f32, f32, f32),  // x, y, z
    distance_to_exit: f32,      // 2.1 - y
    direction: Direction,        // TowardExit, AwayFromExit, Stationary
    velocity: f32,               // m/s toward exit
    authorized: bool,
    samples: VecDeque<(u64, f32)>, // (time, y) for direction calc
}
```

### Priority Queue

Sort occupants by `distance_to_exit` ascending. When gate opens, track who actually exits (position crosses y=2.1 or LINE_CROSS_FORWARD event).

### Re-open Logic

If authorized occupant was next in queue but different track crossed EXIT_1:
1. Check if authorized occupant still in zone
2. If yes and still closest to exit, re-open gate
3. Decrement retry counter
4. Log tailgating event

### Thresholds (initial values, tune with data)

| Parameter | Value | Rationale |
|-----------|-------|-----------|
| Exit line y | 2.1 | Matches EXIT_1 geometry |
| "Close to exit" | y > 1.5 | Approaching exit area |
| Direction window | 400ms | 5 frames, smooth noise |
| Stationary threshold | ±0.05m | Less than this = not moving |
| Max retries | 2 | Prevent infinite re-opens |

## Files

- `/tmp/gate-analysis/movement_analysis.py` - Behavior classification
- `/tmp/gate-analysis/find_stuck.py` - Stuck scenario detection
- `/tmp/gate-analysis/analyze.py` - Position vs zone/line comparison
- `config/geometries/netto.json` - Xovis geometry data

## Visualization: HTML/SVG Floorplan

The Xovis geometry data (`config/geometries/netto.json`) contains polygon coordinates for all zones and lines. This can be rendered as an interactive floorplan.

**Approach**:
1. Parse geometry polygons from `netto.json`
2. Render as SVG paths/polygons
3. Overlay position dots with track_id labels
4. Animate movement using requestAnimationFrame or CSS transitions
5. Color-code by direction (green=toward exit, red=away, yellow=stationary)

**SVG Structure**:
```html
<svg viewBox="-2 -1 6 5">
  <!-- Gate zone polygon -->
  <polygon points="0,-0.2 3.5,-0.2 3.5,2.1 0,2.1" fill="#eef" stroke="#aaa"/>

  <!-- Exit line -->
  <line x1="1.71" y1="2.1" x2="2.76" y2="2.1" stroke="green" stroke-width="0.05"/>

  <!-- Person dot (animated) -->
  <circle cx="1.5" cy="0.8" r="0.1" fill="green">
    <animate attributeName="cy" from="0.8" to="2.1" dur="3s"/>
  </circle>
</svg>
```

**Data source**: Stream positions via MQTT → WebSocket → browser, or replay from JSONL.

## Prediction Algorithms (Gaming Techniques)

Online video games solve similar problems with network latency. Relevant algorithms:

### 1. Dead Reckoning

Predict future position based on current velocity:
```
predicted_position = current_position + velocity * time_delta
```

**Application**: Predict where person will be when gate command arrives (compensate for 1.4s lag).

### 2. Linear Interpolation (Lerp)

Smooth between known positions:
```
position = lerp(pos_a, pos_b, t)  // t in [0,1]
```

**Application**: Smooth rendering between 80ms frame updates.

### 3. Exponential Moving Average (EMA)

Smooth noisy velocity calculations:
```
velocity_smoothed = alpha * velocity_current + (1-alpha) * velocity_previous
```

**Application**: Reduce jitter in direction detection. Alpha = 0.3-0.5 works well.

### 4. Kalman Filter

Optimal state estimation combining prediction + measurement:
```
predict: x_pred = F * x_prev  (physics model)
update:  x_new = x_pred + K * (measurement - x_pred)
```

**Application**: More accurate velocity/position estimation, especially when positions are noisy.

### 5. Intent Prediction

Combine position + direction + velocity to predict goal:
```
if direction == TowardExit && velocity > 0.5 m/s:
    intent = "likely to exit"
    eta = distance / velocity
```

**Application**: Trigger gate open when authorized person has high exit intent, not just zone entry.

### Validated Results

Tested against 150,844 frames of actual position data:

| Algorithm | Metric | Result |
|-----------|--------|--------|
| **Dead Reckoning** (1.4s ahead) | Avg error | 0.33m |
| | Median error | 0.24m |
| | 95th percentile | 0.94m |
| **ETA Prediction** | Within 2s accuracy | 46% |
| | Within 5s accuracy | 67% |
| | Median error | 0.6s |
| **Direction Prediction** | "Toward exit" → actually exits | 88% |
| | "Away from exit" → actually exits | 68% |
| **EMA Smoothing** | Best alpha | 0.2-0.4 |
| | Avg error (alpha=0.2) | 0.18m |

**Key Insights**:
- Dead reckoning works well: 0.33m error for 1.4s prediction is acceptable
- Direction is highly predictive: 88% of "toward exit" actually exit
- ETA at gate entry is less reliable (people pause, change speed)
- EMA smoothing helps but alpha doesn't matter much (0.2-0.4 are similar)

### Recommended Implementation

Start simple, add complexity only if needed:

1. **Phase 1**: Dead reckoning + EMA for smooth velocity
   - Predict position 1.4s ahead (avg error: 0.33m)
   - Smooth velocity with EMA (alpha=0.3)

2. **Phase 2**: Intent scoring
   - Use direction as primary signal (88% correlation with exit)
   - Score = (1 / distance_to_exit) * velocity_toward_exit
   - Higher score = higher gate priority

3. **Phase 3** (if needed): Kalman filter
   - Only if position noise causes problems (current data looks clean)

## Files

- `/tmp/gate-analysis/movement_analysis.py` - Behavior classification
- `/tmp/gate-analysis/find_stuck.py` - Stuck scenario detection
- `/tmp/gate-analysis/analyze.py` - Position vs zone/line comparison
- `/tmp/gate-analysis/deep_analysis.py` - Timing, prediction, pre-emptive analysis
- `/tmp/gate-analysis/stitch_exit_analysis.py` - Stitching and exit detection
- `/tmp/gate-analysis/validate_predictions.py` - Prediction algorithm validation
- `/tmp/gate-analysis/group_fallback_analysis.py` - GROUP as PERSON fallback
- `/tmp/gate-analysis/prediction_outliers.py` - Failure analysis and strategy comparison
- `/tmp/gate-analysis/speed_and_correction.py` - Speed tracking and correction timeframe
- `/tmp/gate-analysis/pos_to_gate_analysis.py` - POS zone to gate early detection
- `/tmp/gate-analysis/gate_to_exit_timing.py` - Actual transit times and optimal trigger
- `config/geometries/netto.json` - Xovis geometry data

## Speed Tracking

**Question**: Can we track walking speed, not just direction?

**Finding**: Yes. Velocity calculated from position delta.

| Metric | Value |
|--------|-------|
| Average speed (gate area) | 0.35 m/s (1.2 km/h) |
| Median speed | 0.19 m/s (0.7 km/h) |
| 95th percentile | 1.13 m/s (4.1 km/h) |

**Speed distribution**:
- Slow (<0.5 m/s): 73% - standing, browsing
- Normal (0.5-1.2 m/s): 24% - walking
- Fast (>1.2 m/s): 4% - rushing

## Course Correction Timeframe

**Question**: If prediction is wrong, how quickly can we detect and correct?

**Finding**: Median 0.8s to detect overtake.

| Metric | Value |
|--------|-------|
| Average detection time | 1.0s |
| Median detection time | 0.8s |
| 95th percentile | 3.0s |
| Within 1s | 59% |
| Within 2s | 88% |
| Within 3s | 96% |

**Conclusion**: 85% initial accuracy + 0.8s correction = robust system. Monitor priority every 500ms and re-prioritize if overtake detected.

## GROUP as PERSON Fallback

**Question**: When PERSON tracking is lost, does GROUP tracking continue?

**Finding**: Yes. GROUP can bridge gaps.

| Metric | Count |
|--------|-------|
| PERSON tracks | 1,453 |
| GROUP tracks | 1,379 |
| GROUP continued after PERSON ended | 2,579 |
| In gate area | 772 |
| Simultaneous PERSON+GROUP pairs | 2,278 |

**Examples**:
```
PERSON 58115 ended at pos=(-1.30, 1.21)
  GROUP 2147529081 was 0.75m away
  GROUP continued for 127.7s (910 frames)
  GROUP reached EXIT (y > 2.0)
```

**Conclusion**: When PERSON track disappears, check for nearby GROUP track and use its position until PERSON resumes.

## Prediction Strategy Comparison

**Question**: Is 85% accuracy better than zone entry order?

| Strategy | Accuracy |
|----------|----------|
| Distance to exit | 85.4% |
| Distance + direction | 84.7% |
| Distance + velocity (ETA) | 81.6% |
| **Gate zone entry order** | **73.6%** |

**Finding**: Distance-based prediction is **12% better** than zone entry order.

## Prediction Failure Analysis

**Question**: What happens in the 15% of wrong predictions?

| Failure Type | % |
|--------------|---|
| Overtaken (someone walked faster) | 85% |
| Predicted hesitated/stopped | 13% |
| Turned back | 0% |
| Unclear | 2% |

**Examples**:
```
Overtaken:
  Predicted 58112 (dist=0.66m, vel=0.4m/s)
  Actual winner 58118 (dist=1.67m, vel=0.8m/s)
  → Winner walked 2x faster despite being further away
```

**Conclusion**: Most failures are from someone walking faster. Adding velocity consideration doesn't help because slow person might speed up. Best approach: predict by distance, correct when overtake detected.

## Game-Style Prediction Algorithms

| Algorithm | Description | Applicability |
|-----------|-------------|---------------|
| **Dead Reckoning** | position += velocity * dt | Good for lag compensation (0.33m error at 1.4s) |
| **Server Reconciliation** | Predict locally, correct with authoritative data | Perfect fit - predict, then correct |
| **Entity Interpolation** | Smooth between known positions | Less useful - we want prediction |
| **Adaptive Prediction** | Adjust based on behavior | Good for variable walkers |

**Recommended approach**:
1. Predict priority by distance (85% accurate)
2. Monitor every 500ms for position changes
3. If overtake detected (median 0.8s), re-prioritize
4. Result: ~98% effective accuracy with correction

## Early Detection from POS Zones

**Question**: Can position data trigger gate earlier than GATE_1 zone entry?

**Finding**: Depends on which POS zone. **POS_1 is far** (5.2s walk), **POS_3 is medium** (1.2s), others are adjacent.

| POS Zone | Transitions | Avg delay to GATE_1 | Position early detection |
|----------|-------------|---------------------|--------------------------|
| **POS_1** | 49 | **5,205ms (5.2s)** | **4,048ms (4.0s)** |
| POS_2 | 363 | 269ms (0.3s) | ~0ms |
| **POS_3** | 86 | **1,215ms (1.2s)** | 62ms |
| POS_4 | 178 | 354ms (0.4s) | 167ms (0.2s) |
| POS_5 | 156 | 218ms (0.2s) | ~0ms |

**Key insight**: POS_2/4/5 are adjacent to GATE_1 - minimal early detection opportunity. But **POS_1 is far** (5.2s walk), and position data can detect approach **4 seconds earlier**. POS_3 has 1.2s delay.

**Comparison**:
| Trigger Method | Avg delay from POS exit |
|----------------|-------------------------|
| Current (GATE_1 ZONE_ENTRY) | 475ms |
| Position-based trajectory | 300ms |
| **Improvement** | **175ms (37% faster)** |

**Implementation for POS_1** (the far zone):
1. When authorized person exits POS_1
2. Monitor position trajectory
3. If moving toward gate (y increasing) AND in gate corridor (x: 0-3.5m)
4. Trigger gate open immediately
5. Don't wait for GATE_1 ZONE_ENTRY event

**Benefit**: For POS_1 customers, gate starts opening while they're still walking toward it.

## Optimal Trigger Timing

**Question**: Can we trigger at the perfect time so gate is open exactly when person arrives?

### Actual Transit Times (GATE_1 entry → EXIT_1)

| Percentile | Time |
|------------|------|
| Median | 3.4s |
| Fast (10th %) | 2.3s |
| Slow (90th %) | 17.8s |
| Hesitaters (>10s) | 11% |

### Gate Readiness (assuming 2.5s gate open time)

| Trigger Timing | % Gate Ready |
|----------------|--------------|
| On GATE_1 entry | 85% |
| 500ms early | 96% |
| 1000ms early | 97% |

### Key Finding: Velocity-Based ETA Doesn't Work Well

**Problem**: People don't walk at constant velocity. They pause, hesitate, look around. Median timing error with velocity-based trigger: **-7.7s** (gate not ready).

**Solution**: Use velocity to **categorize** walkers, not predict exact ETA:

| Walker Type | Velocity | Strategy |
|-------------|----------|----------|
| Fast (>1.0 m/s) | Moving quickly | Trigger 500ms before GATE_1 entry |
| Normal (0.5-1.0 m/s) | Walking pace | Trigger on GATE_1 entry |
| Slow (<0.5 m/s) | Hesitating | GATE_1 entry is fine (they're slow anyway) |

### Implementation

```
if authorized_person approaching gate:
    velocity = calculate_velocity(recent_positions)

    if velocity > 1.0:  # Fast walker
        trigger_now()  # Don't wait for GATE_1 entry
    else:
        wait_for_gate_zone_entry()  # Normal behavior
```

**Result**: 96% of people find gate ready (up from 85% with zone entry alone).

## GATE_1 Zone Boundary Analysis

**Key Finding**: The current GATE_1 zone boundary may not be optimal for gate timing.

### Current vs Optimal Trigger Position

| Trigger y | Distance from EXIT_1 | % Gate Ready (2.5s gate) |
|-----------|---------------------|--------------------------|
| y = +1.0m | 1.1m | 71% |
| y = +0.5m | 1.6m | 83% |
| **y = +0.4m (current)** | **1.7m** | **~85%** |
| **y = 0.0m (recommended)** | **2.1m** | **93%** |
| y = -0.5m | 2.6m | 96% |
| y = -1.0m | 3.1m | 96% |

### The Problem

Current GATE_1 zone entry is at y ≈ 0.4m. For a 2.5s gate open time, only 85% of people find gate ready on arrival.

### The Fix

Move trigger point **0.4m earlier** (from y=0.4 to y=0.0):
- Gate ready: 85% → **93%**
- Only 0.4m change in trigger position

### Implementation Options

**Option 1: Adjust Xovis GATE_1 zone geometry**
- Extend GATE_1 polygon to start at y ≈ 0.0m instead of y ≈ 0.4m
- Pro: Uses existing zone event infrastructure
- Con: Requires Xovis configuration change

**Option 2: Virtual trigger line using position data**
- Keep GATE_1 zone as-is
- Use position tracking to detect when y crosses 0.0m
- Trigger gate open at virtual line, not zone entry
- Pro: No Xovis changes needed
- Con: Requires position-based logic in gateway

### Recommendation

For maximum impact with minimal change:
1. **Measure actual gate open time** (we assumed 2.5s)
2. **Calculate optimal trigger y** based on actual timing
3. Either adjust GATE_1 zone OR implement virtual trigger line
4. Expected improvement: 85% → 93% gate ready on arrival

## Multi-Person Gate Zone Scenarios

**Question**: How often are multiple people in the gate zone at once?

**Finding**: Very common - 847 overlapping scenarios in our dataset.

| Metric | Value |
|--------|-------|
| Total GATE_1 entries | 811 |
| Times with 2+ people | 847 |
| Average overlap duration | 1.8s |

### Exit Order Prediction

When multiple people are in gate zone, who exits first?

| Strategy | Accuracy |
|----------|----------|
| Closer to EXIT_1 exits first | 84% |
| Zone entry order (first in) | ~74% |

**Conclusion**: Sorting by distance-to-exit is 10% more accurate than zone entry order.

### Blocked Scenarios

Someone exits while another person waits:

| Metric | Value |
|--------|-------|
| Blocked scenarios | 563 |
| Median wait time | 1.8s |
| Average wait time | 1.9s |

This confirms the "blocked" scenario is common and needs re-trigger handling.

### Gate State Between Exits

**Question**: Does the gate close between consecutive exits, or stay open?

**Finding**: Gate typically stays open - **63% of exits are within 3s** of each other.

| Gap Between Exits | Count | % | Gate State |
|-------------------|-------|---|------------|
| ≤3s | 412 | 63% | Likely still open |
| 3-10s | 35 | 5% | Closing/closed |
| >10s | 211 | 32% | Definitely separate cycle |

**Median gap**: 1.9s (well within gate open window)

### Who Triggered the Gate?

In consecutive exits within 3s:

| Scenario | Count | % |
|----------|-------|---|
| Both had GATE_1 entry | 410 | **100%** |
| Only first had entry (tailgating) | 1 | 0% |

**No significant tailgating detected** - nearly all exits involved people who had entered GATE_1 zone (triggered gate).

**Who entered GATE_1 first:**

| Who | Count | % |
|-----|-------|---|
| First exiter entered first | 347 | 84% |
| Second exiter entered first | 64 | **16%** |

The **16%** case = "blocked" scenario: Person A triggered gate, but Person B exited first. Person A is left behind.

**Conclusion**: Gate typically stays open for both people. The question is attribution - if Person B exits through Person A's gate, Person A may need re-trigger.

## POS Zone Exit as Early Trigger

**Question**: Can we trigger gate earlier based on POS zone exit + direction?

**Finding**: Limited benefit. Only 0.3s median lead time.

### POS Exit Analysis

| Metric | Value |
|--------|-------|
| POS zone exits (POS_2/4/5) | 1,861 |
| Actually went to GATE_1 | 46% |
| Actually exited | 51% |
| **Didn't go to gate** | **54%** |

**Problem**: Most POS exits don't lead to gate. People move around, go to other areas, return to shelves.

### Velocity Filter Accuracy

Can we filter by direction to reduce false positives?

| Velocity Threshold | Precision | Recall |
|--------------------|-----------|--------|
| vy > 0.0 m/s | 60% | 57% |
| vy > 0.3 m/s | 71% | 31% |
| vy > 0.5 m/s | 76% | 17% |

Higher thresholds = fewer false positives but miss many real exiters.

### Lead Time

| Metric | Value |
|--------|-------|
| Median POS exit → GATE_1 entry | **0.3s** |
| Average | 22.1s (skewed by wanderers) |

**Conclusion**: POS_2/4/5 are adjacent to GATE_1. Little early detection opportunity.

### Recommendation

Don't use POS exit alone as trigger - 54% false positive rate. Better approach:
1. Use GATE_1 entry as primary trigger
2. Use POS exit + velocity as *secondary signal* for pre-warming gate
3. Confirm with GATE_1 entry before actual command

## GATE_1 Zone Geometry Gap

**Question**: GATE_1 zone ends before EXIT_1 - is this a problem?

**Finding**: There's a **0.7m gap** between GATE_1 zone exit (y ≈ 1.4m) and EXIT_1 line (y ≈ 2.1m).

| Location | Y Position |
|----------|------------|
| GATE_1 zone exit | ~1.4m avg |
| EXIT_1 line | ~2.1m |
| **Gap** | **~0.7m** |

### Impact

| Use Case | Zone Events | Position Data |
|----------|-------------|---------------|
| Track person approaching exit | ✗ Lose after ZONE_EXIT | ✓ Continuous |
| Know who's closest to exit | ✗ | ✓ |
| Re-trigger for blocked person | ✗ | ✓ |

**Conclusion**: The gap makes zone events insufficient for sophisticated gate management. Position tracking is necessary to monitor the 0.7m gap between zone exit and actual exit.

## Re-Trigger Strategy

**Question**: How do we re-open gate for blocked authorized person?

### Position-Based Detection

Position can detect re-approach **1.2s before exit**:

| Metric | Value |
|--------|-------|
| Blocked scenarios analyzed | 50 |
| Re-approach detected | 49 (98%) |
| Average lead time | 1.2s |

### Algorithm

```
1. On GATE_1 ZONE_ENTRY (authorized):
   - Open gate
   - Start position monitoring for all zone occupants

2. On EXIT_1 crossing:
   - If it's the authorized person → done
   - If someone else:
     a) Check if authorized person still in zone/gap (position tracking)
     b) Monitor their distance to EXIT_1
     c) If distance < 1m AND decreasing → Re-trigger gate

3. On position change (500ms interval):
   - Update distance-to-exit for all occupants
   - If authorized person approaching exit → ensure gate open
   - If authorized person retreating → let gate close

4. On GATE_1 ZONE_EXIT (authorized):
   - Continue tracking in gap (position data)
   - Only cancel when they leave completely or someone else exits
```

### Expected Improvement

| Scenario | Current | With Re-trigger |
|----------|---------|-----------------|
| Single person | Works | Works |
| Blocked by someone | Must re-enter zone | Auto re-triggers |
| Lead time | n/a | 1.2s before exit |

## Journey Method Comparison

Built journeys from Xovis sensor data using different methods, output to folders per method:

### Methods Tested

| Method | Description |
|--------|-------------|
| **baseline** | Zone events only (current gateway method) |
| **test_1_position_exit** | Detect exit via position y > 2.3 when LINE_CROSS missed |
| **test_2_early_gate** | Trigger gate at y=0 instead of GATE_1 zone entry |
| **test_3_group_fallback** | Continue tracking via GROUP when PERSON track lost |

### Results (Corrected)

| Method | Total | Exits | Lost | Exit Δ |
|--------|-------|-------|------|--------|
| **baseline** | 1,447 | 331 | 1,116 | - |
| **test_1_position_exit** | 1,447 | **565** | 882 | **+234 (+71%)** |
| test_2_early_gate | 1,447 | 331 | 1,116 | +0 |
| test_3_group_fallback | - | - | - | INVALID |
| test_3b_group_validated | 1,447 | 331 | 1,116 | +0 |

### Position Exit Detection Details

**Threshold**: y > 2.3 (EXIT_1 is at y=2.1, using 0.2m buffer)

**Filter**: Only count tracks with zone events (filters out 86 noise tracks that never entered store)

**Breakdown**:
- Event exits (LINE_CROSS): 331
- Position exits (missed by sensor): 234
- Total: 565

### Key Findings

**Test 1 (Position Exit Detection):**
- **+234 exits** (71% improvement over baseline)
- Position y > 2.3 catches exits that LINE_CROSS_FORWARD missed
- Requires zone events filter to avoid counting tracking noise outside store
- **Recommendation: Implement as fallback for missed LINE_CROSS events**

**Test 2 (Early Gate Trigger):**
- No change in outcomes (just timing improvement)
- Validates that position can detect approach before zone entry

**Test 3 (GROUP Fallback) - INVALID:**
- Original test showed +966 exits - **this was wrong**
- GROUPs and PERSONs coexist (98% overlap), not as fallback
- Only 8% of nearby GROUPs were going same direction as PERSON
- With proper validation (path correlation + direction): **0 valid rescues**
- **Conclusion: GROUP tracking is NOT useful for PERSON identity continuation**

### Implementation Recommendations

1. **Position-based exit detection** (high value):
   - When TRACK_DELETE with no EXIT_1 crossing
   - Check if max_y > 2.3 (with buffer)
   - Only if track has zone events (entered store)
   - Expected: +234 exits (+71% more exits detected)

2. ~~**GROUP fallback**~~ (INVALID):
   - GROUPs are parallel tracking, not fallback for lost PERSONs
   - Do NOT use GROUP exits to attribute to lost PERSONs

3. **Early gate trigger** (optional):
   - Detect approach at y=0 instead of zone entry
   - Only useful if gate control is selective

## Analysis Files

- `/tmp/gate-analysis/journey_methods/baseline.py` - Baseline zone-only method
- `/tmp/gate-analysis/journey_methods/test_1_position_exit.py` - Position exit detection
- `/tmp/gate-analysis/journey_methods/test_2_early_gate.py` - Early gate trigger
- `/tmp/gate-analysis/journey_methods/test_3_group_fallback.py` - GROUP fallback
- `/tmp/gate-analysis/journey_methods/compare.py` - Method comparison
- `/tmp/gate-analysis/journeys/` - Output folders per method with JSON per track_id
- `/tmp/gate-analysis/movement_analysis.py` - Behavior classification
- `/tmp/gate-analysis/find_stuck.py` - Stuck scenario detection
- `/tmp/gate-analysis/analyze.py` - Position vs zone/line comparison
- `/tmp/gate-analysis/deep_analysis.py` - Timing, prediction, pre-emptive analysis
- `/tmp/gate-analysis/stitch_exit_analysis.py` - Stitching and exit detection
- `/tmp/gate-analysis/validate_predictions.py` - Prediction algorithm validation
- `/tmp/gate-analysis/group_fallback_analysis.py` - GROUP as PERSON fallback
- `/tmp/gate-analysis/prediction_outliers.py` - Failure analysis and strategy comparison
- `/tmp/gate-analysis/speed_and_correction.py` - Speed tracking and correction timeframe
- `/tmp/gate-analysis/pos_to_gate_analysis.py` - POS zone to gate early detection
- `/tmp/gate-analysis/gate_to_exit_timing.py` - Actual transit times and optimal trigger
- `/tmp/gate-analysis/pos_exit_trigger.py` - POS exit as trigger analysis
- `/tmp/gate-analysis/retrigger_analysis_v2.py` - Re-trigger scenario analysis
- `/tmp/gate-analysis/precise_timing.py` - Position vs event synchronization
- `config/geometries/netto.json` - Xovis geometry data

## Implementation List

### Validated Improvements (Ready to Implement)

| # | Feature | Impact | Effort | Status |
|---|---------|--------|--------|--------|
| 1 | **Position-based exit detection** | +234 exits (+71%) | Low | ✅ HIGH VALUE |
| | When TRACK_DELETE with no LINE_CROSS, check max_y > 2.3 | | | |
| | Filter: only if track has zone events | | | |
| 7 | **Smart gate re-trigger** | 17 frustrated customers recovered | Medium | ✅ HIGH VALUE |
| | Re-open gate for authorized person blocked by someone exiting | | | |
| | Distance threshold: 1.15m from exit | | | |
| 3 | **Missed zone events recovery** | +8 authorizations | Low | ✅ MEDIUM VALUE |
| | Detect ZONE_ENTRY/EXIT via position when Xovis misses (2.3% miss rate) | | | |

### Low Value (Defer)

| # | Feature | Finding | Recommendation |
|---|---------|---------|----------------|
| 2 | Position-based stitching | Already 98.2% catch rate | No change needed |
| 4 | POS dwell normalization | Only +2 authorizations | Low priority |

### Invalidated (Do NOT Implement)

| Feature | Why Invalid |
|---------|-------------|
| ~~GROUP as PERSON fallback~~ | GROUPs coexist with PERSONs (98%), not fallback. 0 valid rescues. |
| ~~People moving together~~ | Only 1 of 304 POS co-presence pairs needed group auth (0.3%) |
| ~~Multi-person exit priority~~ | Only 4 problem scenarios, all outsiders or physical access issues, not gate logic |

---

## User Story #1: Position-Based Exit Detection

### Summary
Detect customer exits using position data when Xovis misses the LINE_CROSS_FORWARD event.

### User Story
**As a** store operator,
**I want** the system to detect exits using position data as a fallback,
**So that** customer journeys are accurately tracked even when the sensor misses the exit line crossing event.

### Problem
Xovis misses ~41% of exit events (only 331 of 565 actual exits detected via LINE_CROSS_FORWARD). This causes:
- Journeys incorrectly marked as "lost" instead of "exit"
- Inaccurate journey analytics
- Potential false "stuck in store" alerts

### Solution
When a track is deleted (TRACK_DELETE) without a LINE_CROSS_FORWARD event on EXIT_1:
1. Check if the person's maximum y-position exceeded 2.3m (EXIT_1 is at y=2.1, using 0.2m buffer)
2. Check if the track had zone events (to filter out tracking noise outside store)
3. If both conditions met → mark journey as "exit" instead of "lost"

### Acceptance Criteria
- [ ] Track `max_y` position throughout person's journey
- [ ] On TRACK_DELETE, if no exit_cross event AND max_y > 2.3 AND has_zone_events → outcome = exit
- [ ] Log position-detected exits separately for monitoring
- [ ] Add metric: `journeys_position_exit_total`

### Technical Notes
```rust
// In Person struct, add:
max_y: f32,
has_zone_events: bool,

// In position processing:
if position.y > person.max_y {
    person.max_y = position.y;
}

// On TRACK_DELETE:
let outcome = if person.exit_crossed {
    Outcome::Exit
} else if person.max_y > 2.3 && person.has_zone_events {
    Outcome::PositionExit  // or just Outcome::Exit with a flag
} else {
    Outcome::Lost
};
```

### Impact
- **+234 exits detected** (71% improvement)
- More accurate journey tracking
- Better analytics on exit rates

### Effort
Low - straightforward addition to existing tracker logic

---

## User Story #2: Smart Gate Re-Trigger

### Summary
Automatically re-open the gate for an authorized customer who was blocked by someone else exiting.

### User Story
**As an** authorized customer,
**I want** the gate to re-open if someone else exits before me,
**So that** I don't have to re-enter the zone or wait for manual assistance.

### Problem
When gate opens for an authorized person and someone else (closer to exit) walks through first:
- Gate closes after the exit
- Authorized person is left at a closed gate
- Must re-enter zone to trigger another open (frustrating UX)

### Current Data Caveat
⚠️ **Note**: Current data is from IR-sensor-controlled gate (opens for everyone). The "blocked" scenarios identified (78 cases) show what WOULD happen with authorization-controlled gates, but actual customer frustration cannot be measured until selective gate control is deployed.

### Solution
After someone exits through the gate:
1. Check if any authorized person is still in GATE_1 zone
2. If yes, and they are within 1.15m of EXIT_1 line
3. Re-trigger gate open for that specific track_id
4. Apply cooldown (500ms) to prevent rapid re-triggers

### Acceptance Criteria
- [ ] After EXIT_1 crossing detected, check for remaining authorized persons in GATE_1
- [ ] If authorized person within 1.15m of exit → send gate open command
- [ ] Track re-trigger to specific track_id (prevent unauthorized piggybacking)
- [ ] Add 500ms cooldown between re-triggers
- [ ] Log re-trigger events for monitoring
- [ ] Add metric: `gate_retrigger_total`
- [ ] Configurable: enable/disable re-trigger, distance threshold

### Technical Notes
```rust
// After processing EXIT_1 crossing:
fn check_retrigger(&mut self, exited_track_id: i64) {
    for (tid, person) in &self.persons {
        if *tid == exited_track_id { continue; }
        if !person.authorized { continue; }
        if !person.in_gate_zone { continue; }

        let distance_to_exit = 2.1 - person.position.y;
        if distance_to_exit < self.config.retrigger_distance_threshold {
            self.trigger_gate(*tid, GateTriggerReason::Retrigger);
            break; // Only retrigger for one person
        }
    }
}
```

### Config
```toml
[gate]
retrigger_enabled = true
retrigger_distance_threshold = 1.15  # meters from EXIT_1
retrigger_cooldown_ms = 500
```

### Impact (Estimated)
From analysis of position data:
- 78 authorized people blocked in ~5 hours of data
- 9 appeared to turn back (potential lost sales)
- 8 waited 3-18 seconds before exiting

**True impact unknown** until selective gate control is deployed. This feature should be:
1. Implemented with logging
2. Deployed with IR sensor still active (as safety net)
3. Impact measured once IR sensor is disabled

### Effort
Medium - requires tracking gate state and post-exit checking

### Dependencies
- Position tracking in gate zone (from User Story #1)
- Gate control integration

---

## Data Limitation Notice

⚠️ **All analysis in this document is based on simulated authorization.**

During data collection:
- IR sensor was controlling the gate (opens for everyone)
- No gate commands were sent based on authorization
- Customer behavior was influenced by knowing gate always opens

This means:
- "Blocked" scenarios are theoretical (customers weren't actually blocked)
- "Turn back" behavior might be normal exits, not frustration
- True impact of re-trigger cannot be measured from this data

**To get real data**: Deploy gateway with authorization-based gate control and collect new position logs.

---

## User Story #3: Position Data Logging for Analysis

### Summary
Log detailed position and event data to files for offline analysis of gate behavior.

### User Story
**As a** developer,
**I want** the gateway to log detailed position and event data,
**So that** I can analyze real-world gate behavior and validate/tune algorithms.

### Problem
Current analysis used Xovis sensor data directly. Once gateway controls the gate:
- Need to capture position data + gate commands + outcomes together
- Need to analyze real customer behavior with authorization-controlled gates
- Need to validate that position-based algorithms work as expected

### Solution
Add file-based logging of:
1. Raw position data (track_id, x, y, z, timestamp)
2. Zone events (entry/exit with geometry_id)
3. Gate commands sent (track_id, reason, timestamp)
4. Journey outcomes (track_id, outcome, auth status, dwell time)

### Acceptance Criteria
- [ ] Log positions to JSONL file (configurable path)
- [ ] Log gate commands with reason (zone_entry, retrigger, etc.)
- [ ] Log journey completions with full context
- [ ] Configurable: enable/disable logging, file rotation
- [ ] Same format as analysis scripts expect (easy to re-run analysis)

### Config
```toml
[logging]
position_log_enabled = true
position_log_path = "/opt/avero/logs/positions.jsonl"
position_log_rotate_mb = 100
```

### Log Format
```jsonl
{"type":"position","ts":1234567890,"tid":58100,"x":1.5,"y":0.8,"z":1.7}
{"type":"zone_event","ts":1234567891,"tid":58100,"event":"entry","zone":"GATE_1"}
{"type":"gate_cmd","ts":1234567892,"tid":58100,"reason":"zone_entry"}
{"type":"journey","ts":1234567900,"tid":58100,"outcome":"exit","auth":true,"dwell_ms":8500}
```

### Impact
- Enables validation of position-based algorithms with real data
- Allows tuning of thresholds (exit y, retrigger distance)
- Provides data for future improvements

### Effort
Low - extend existing MQTT egress pattern to file logging
