#!/usr/bin/env python3
"""
Gateway Baseline Stress Test

Establishes a firm baseline by testing:
1. Burst traffic handling
2. Sustained throughput
3. Stitch reliability
4. Error rates

Usage:
    python baseline_test.py --host localhost --duration 60
"""

import argparse
import json
import time
import random
import threading
from datetime import datetime, timezone
from dataclasses import dataclass
from typing import List, Optional

try:
    import paho.mqtt.client as mqtt
except ImportError:
    print("Error: paho-mqtt not installed. Run: pip install paho-mqtt")
    exit(1)


@dataclass
class TestResults:
    messages_sent: int = 0
    tracks_created: int = 0
    journeys_started: int = 0
    journeys_completed: int = 0  # Expected based on what we sent
    authorized_journeys: int = 0  # Journeys with full dwell
    stitches_attempted: int = 0
    burst_messages: int = 0
    errors: List[str] = None

    def __post_init__(self):
        if self.errors is None:
            self.errors = []


class BaselineTest:
    def __init__(self, host: str, port: int, topic: str):
        self.host = host
        self.port = port
        self.topic = topic
        self.client = mqtt.Client(
            mqtt.CallbackAPIVersion.VERSION2,
            client_id=f"baseline-test-{random.randint(1000, 9999)}"
        )
        self.connected = False
        self.track_counter = 1000
        self.results = TestResults()

    def connect(self):
        def on_connect(client, userdata, flags, rc, props):
            if rc == 0:
                self.connected = True

        self.client.on_connect = on_connect
        self.client.connect(self.host, self.port, 60)
        self.client.loop_start()

        for _ in range(50):
            if self.connected:
                break
            time.sleep(0.1)

        if not self.connected:
            raise ConnectionError(f"Could not connect to {self.host}:{self.port}")
        print(f"Connected to MQTT broker at {self.host}:{self.port}")

    def disconnect(self):
        self.client.loop_stop()
        self.client.disconnect()

    def _now_iso(self) -> str:
        return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%S.%f")[:-3] + "+00:00"

    def _publish(self, events: list, tracked_objects: list = None):
        msg = {
            "live_data": {
                "frames": [{
                    "time": self._now_iso(),
                    "events": events,
                    "tracked_objects": tracked_objects or []
                }]
            }
        }
        self.client.publish(self.topic, json.dumps(msg))
        self.results.messages_sent += 1

    def _next_track_id(self) -> int:
        self.track_counter += 1
        return self.track_counter

    # ===================
    # Test Scenarios
    # ===================

    def test_burst_traffic(self, burst_size: int = 100):
        """Send a burst of messages as fast as possible"""
        print(f"\n[BURST TEST] Sending {burst_size} messages...")

        tracks = []  # Store (tid, position) tuples
        start = time.time()

        # Create tracks in rapid succession
        for _ in range(burst_size):
            tid = self._next_track_id()
            pos = [random.uniform(0, 5), random.uniform(0, 5), random.uniform(1.5, 1.9)]
            tracks.append((tid, pos))
            self._publish([{
                'type': 'TRACK_CREATE',
                'attributes': {'track_id': tid}
            }], tracked_objects=[{
                'track_id': tid,
                'type': 'PERSON',
                'position': pos
            }])
            self.results.tracks_created += 1

        elapsed = time.time() - start
        rate = burst_size / elapsed if elapsed > 0 else 0
        print(f"  Sent {burst_size} TRACK_CREATE in {elapsed:.3f}s ({rate:.1f} msg/sec)")
        self.results.burst_messages += burst_size

        # Small delay then delete all with positions
        time.sleep(0.5)
        for tid, pos in tracks:
            self._publish([{
                'type': 'TRACK_DELETE',
                'attributes': {'track_id': tid}
            }], tracked_objects=[{
                'track_id': tid,
                'type': 'PERSON',
                'position': pos
            }])

        print(f"  Cleaned up {len(tracks)} tracks")

    def test_sustained_journeys(self, count: int = 10, dwell_ms: int = 200):
        """Run multiple complete journeys with short dwell (for speed)"""
        print(f"\n[SUSTAINED TEST] Running {count} journeys (dwell={dwell_ms}ms)...")

        for i in range(count):
            tid = self._next_track_id()
            self.results.journeys_started += 1

            # Create track
            self._publish([{
                'type': 'TRACK_CREATE',
                'attributes': {'track_id': tid}
            }], tracked_objects=[{
                'track_id': tid,
                'type': 'PERSON',
                'position': [2.0, 3.0, 1.75]
            }])
            self.results.tracks_created += 1

            # Enter POS zone
            self._publish([{
                'type': 'ZONE_ENTRY',
                'attributes': {'track_id': tid, 'geometry_id': 1001}
            }])

            # Dwell (short for testing, won't authorize but that's OK)
            time.sleep(dwell_ms / 1000.0)

            # Exit POS zone
            self._publish([{
                'type': 'ZONE_EXIT',
                'attributes': {'track_id': tid, 'geometry_id': 1001}
            }])

            # Enter gate zone
            self._publish([{
                'type': 'ZONE_ENTRY',
                'attributes': {'track_id': tid, 'geometry_id': 1007}
            }])

            # Cross exit line
            self._publish([{
                'type': 'LINE_CROSS_FORWARD',
                'attributes': {'track_id': tid, 'geometry_id': 1006, 'direction': 'forward'}
            }])

            self.results.journeys_completed += 1

            if (i + 1) % 5 == 0:
                print(f"  Completed {i + 1}/{count} journeys")

        print(f"  All {count} journeys sent")

    def test_stitch_reliability(self, count: int = 10):
        """Test track stitching with controlled scenarios"""
        print(f"\n[STITCH TEST] Testing {count} stitch scenarios...")

        for i in range(count):
            tid1 = self._next_track_id()
            tid2 = self._next_track_id()

            pos = [random.uniform(1, 4), random.uniform(1, 4), random.uniform(1.6, 1.8)]

            # Create first track
            self._publish([{
                'type': 'TRACK_CREATE',
                'attributes': {'track_id': tid1}
            }], tracked_objects=[{
                'track_id': tid1,
                'type': 'PERSON',
                'position': pos
            }])
            self.results.tracks_created += 1

            # Some activity
            self._publish([{
                'type': 'ZONE_ENTRY',
                'attributes': {'track_id': tid1, 'geometry_id': 1001}
            }])
            time.sleep(0.05)
            self._publish([{
                'type': 'ZONE_EXIT',
                'attributes': {'track_id': tid1, 'geometry_id': 1001}
            }])

            # Delete track (should go to stitch pending)
            self._publish([{
                'type': 'TRACK_DELETE',
                'attributes': {'track_id': tid1}
            }], tracked_objects=[{
                'track_id': tid1,
                'type': 'PERSON',
                'position': pos
            }])

            # Small gap
            time.sleep(0.1)

            # Create nearby track (should stitch)
            nearby_pos = [pos[0] + 0.05, pos[1] + 0.02, pos[2] + 0.01]
            self._publish([{
                'type': 'TRACK_CREATE',
                'attributes': {'track_id': tid2}
            }], tracked_objects=[{
                'track_id': tid2,
                'type': 'PERSON',
                'position': nearby_pos
            }])
            self.results.tracks_created += 1
            self.results.stitches_attempted += 1

            # Complete journey
            self._publish([{
                'type': 'LINE_CROSS_FORWARD',
                'attributes': {'track_id': tid2, 'geometry_id': 1006, 'direction': 'forward'}
            }])

        print(f"  Sent {count} stitch scenarios")

    def test_concurrent_tracks(self, track_count: int = 20):
        """Simulate multiple people in store simultaneously"""
        print(f"\n[CONCURRENT TEST] Simulating {track_count} simultaneous tracks...")

        tracks = []

        # Create all tracks
        for _ in range(track_count):
            tid = self._next_track_id()
            tracks.append({
                'id': tid,
                'pos': [random.uniform(0, 5), random.uniform(0, 5), random.uniform(1.5, 1.9)],
                'zone': random.choice([1001, 1002, 1003, 1004, 1005])
            })
            self._publish([{
                'type': 'TRACK_CREATE',
                'attributes': {'track_id': tid}
            }], tracked_objects=[{
                'track_id': tid,
                'type': 'PERSON',
                'position': tracks[-1]['pos']
            }])
            self.results.tracks_created += 1

        print(f"  Created {track_count} tracks")

        # Simulate activity - each track enters/exits a zone
        for t in tracks:
            self._publish([{
                'type': 'ZONE_ENTRY',
                'attributes': {'track_id': t['id'], 'geometry_id': t['zone']}
            }])

        time.sleep(0.2)

        for t in tracks:
            self._publish([{
                'type': 'ZONE_EXIT',
                'attributes': {'track_id': t['id'], 'geometry_id': t['zone']}
            }])

        # All exit
        for t in tracks:
            self._publish([{
                'type': 'LINE_CROSS_FORWARD',
                'attributes': {'track_id': t['id'], 'geometry_id': 1006, 'direction': 'forward'}
            }])
            self.results.journeys_completed += 1

        print(f"  All {track_count} tracks completed journeys")

    def test_authorized_journeys(self, count: int = 3, dwell_secs: float = 8.0):
        """Run journeys with full authorization dwell (triggers gate commands)"""
        print(f"\n[AUTHORIZED TEST] Running {count} journeys with {dwell_secs}s dwell...")

        for i in range(count):
            tid = self._next_track_id()
            self.results.journeys_started += 1

            # Create track
            self._publish([{
                'type': 'TRACK_CREATE',
                'attributes': {'track_id': tid}
            }], tracked_objects=[{
                'track_id': tid,
                'type': 'PERSON',
                'position': [2.0, 3.0, 1.75]
            }])
            self.results.tracks_created += 1

            # Enter POS zone
            self._publish([{
                'type': 'ZONE_ENTRY',
                'attributes': {'track_id': tid, 'geometry_id': 1001}
            }])

            # Full dwell for authorization (8+ seconds)
            print(f"    Journey {i+1}/{count}: dwelling for {dwell_secs}s...")
            time.sleep(dwell_secs)

            # Exit POS zone (should now be authorized)
            self._publish([{
                'type': 'ZONE_EXIT',
                'attributes': {'track_id': tid, 'geometry_id': 1001}
            }])

            # Enter gate zone (should trigger gate open)
            self._publish([{
                'type': 'ZONE_ENTRY',
                'attributes': {'track_id': tid, 'geometry_id': 1007}
            }])

            # Cross exit line
            self._publish([{
                'type': 'LINE_CROSS_FORWARD',
                'attributes': {'track_id': tid, 'geometry_id': 1006, 'direction': 'forward'}
            }])

            self.results.journeys_completed += 1
            self.results.authorized_journeys += 1
            print(f"    Journey {i+1}/{count} completed (should be authorized)")

        print(f"  All {count} authorized journeys sent")

    def run_baseline(self, duration_secs: int):
        """Run full baseline test suite"""
        print("=" * 60)
        print("GATEWAY BASELINE STRESS TEST")
        print("=" * 60)
        print(f"Duration: {duration_secs}s")
        print(f"Host: {self.host}:{self.port}")

        start_time = time.time()

        # Run test scenarios
        self.test_burst_traffic(burst_size=100)
        self.test_sustained_journeys(count=20, dwell_ms=100)
        self.test_stitch_reliability(count=15)
        self.test_concurrent_tracks(track_count=30)

        # Test authorized journeys with full dwell (triggers gate commands)
        self.test_authorized_journeys(count=3, dwell_secs=8.0)

        # If we have time left, do more sustained testing
        elapsed = time.time() - start_time
        remaining = duration_secs - elapsed

        if remaining > 10:
            print(f"\n[EXTENDED TEST] Running for {remaining:.0f}s more...")
            iterations = int(remaining / 2)
            for i in range(iterations):
                self.test_sustained_journeys(count=5, dwell_ms=50)
                time.sleep(0.5)
                if (i + 1) % 10 == 0:
                    print(f"  Extended iteration {i + 1}/{iterations}")

        # Final wait for processing
        print("\nWaiting for final processing...")
        time.sleep(2)

        total_time = time.time() - start_time

        # Print summary
        print("\n" + "=" * 60)
        print("TEST SUMMARY")
        print("=" * 60)
        print(f"Total duration: {total_time:.1f}s")
        print(f"Messages sent: {self.results.messages_sent}")
        print(f"Tracks created: {self.results.tracks_created}")
        print(f"Journeys completed: {self.results.journeys_completed}")
        print(f"Authorized journeys: {self.results.authorized_journeys}")
        print(f"Stitches attempted: {self.results.stitches_attempted}")
        print(f"Burst messages: {self.results.burst_messages}")
        print(f"Message rate: {self.results.messages_sent / total_time:.1f} msg/sec")
        print("=" * 60)
        print("\nRun analyze_logs.sh to verify gateway processed all events correctly")
        print("=" * 60)


def main():
    parser = argparse.ArgumentParser(description='Gateway Baseline Stress Test')
    parser.add_argument('--host', default='localhost', help='MQTT broker host')
    parser.add_argument('--port', type=int, default=1883, help='MQTT broker port')
    parser.add_argument('--topic', default='xovis/live', help='MQTT topic')
    parser.add_argument('--duration', type=int, default=60, help='Test duration in seconds')
    args = parser.parse_args()

    test = BaselineTest(args.host, args.port, args.topic)

    try:
        test.connect()
        test.run_baseline(args.duration)
    finally:
        test.disconnect()
        print("\nDisconnected from broker")


if __name__ == '__main__':
    main()
