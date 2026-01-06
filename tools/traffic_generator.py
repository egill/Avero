#!/usr/bin/env python3
"""
Xovis Traffic Generator for Gateway PoC Testing

Generates fake MQTT traffic to test the gateway-poc application.
Supports various test scenarios including normal journeys, stitching,
edge cases, and stress testing.

Usage:
    python traffic_generator.py --host localhost --scenario all
    python traffic_generator.py --host 100.65.110.63 --scenario stitch
"""

import argparse
import json
import time
import random
import paho.mqtt.client as mqtt
from datetime import datetime, timezone
from dataclasses import dataclass
from typing import Optional, List

# Zone IDs from config
POS_ZONES = [1001, 1002, 1003, 1004, 1005]
GATE_ZONE = 1007
EXIT_LINE = 1006
ENTRY_LINE = 1008  # Optional

@dataclass
class Track:
    """Represents an active track"""
    track_id: int
    x: float
    y: float
    height: float  # meters

    def position(self) -> List[float]:
        return [self.x, self.y, self.height]


class XovisTrafficGenerator:
    """Generates fake Xovis MQTT traffic"""

    def __init__(self, host: str = "localhost", port: int = 1883, topic: str = "xovis/live"):
        self.host = host
        self.port = port
        self.topic = topic
        self.client = mqtt.Client(mqtt.CallbackAPIVersion.VERSION2, client_id=f"traffic-gen-{random.randint(1000, 9999)}")
        self.track_counter = 100
        self.connected = False

    def connect(self):
        """Connect to MQTT broker"""
        def on_connect(client, userdata, flags, reason_code, properties):
            if reason_code == 0:
                self.connected = True
                print(f"Connected to MQTT broker at {self.host}:{self.port}")
            else:
                print(f"Failed to connect: {reason_code}")

        self.client.on_connect = on_connect
        self.client.connect(self.host, self.port, 60)
        self.client.loop_start()

        # Wait for connection
        for _ in range(50):
            if self.connected:
                break
            time.sleep(0.1)

        if not self.connected:
            raise ConnectionError(f"Could not connect to MQTT broker at {self.host}:{self.port}")

    def disconnect(self):
        """Disconnect from MQTT broker"""
        self.client.loop_stop()
        self.client.disconnect()
        print("Disconnected from MQTT broker")

    def _now_iso(self) -> str:
        """Get current time as ISO 8601 string"""
        return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%S.%f")[:-3] + "+00:00"

    def _build_message(self, events: List[dict], tracked_objects: Optional[List[dict]] = None) -> str:
        """Build Xovis JSON message"""
        msg = {
            "live_data": {
                "frames": [{
                    "time": self._now_iso(),
                    "events": events,
                    "tracked_objects": tracked_objects or []
                }]
            }
        }
        return json.dumps(msg)

    def _publish(self, msg: str):
        """Publish message to MQTT"""
        self.client.publish(self.topic, msg)

    def next_track_id(self) -> int:
        """Get next track ID"""
        self.track_counter += 1
        return self.track_counter

    # Basic event generators
    def track_create(self, track: Track):
        """Send TRACK_CREATE event"""
        events = [{
            "type": "TRACK_CREATE",
            "attributes": {"track_id": track.track_id}
        }]
        tracked_objects = [{
            "track_id": track.track_id,
            "type": "PERSON",
            "position": track.position()
        }]
        self._publish(self._build_message(events, tracked_objects))
        print(f"  TRACK_CREATE id={track.track_id} pos={track.position()}")

    def track_delete(self, track: Track):
        """Send TRACK_DELETE event"""
        events = [{
            "type": "TRACK_DELETE",
            "attributes": {"track_id": track.track_id}
        }]
        tracked_objects = [{
            "track_id": track.track_id,
            "type": "PERSON",
            "position": track.position()
        }]
        self._publish(self._build_message(events, tracked_objects))
        print(f"  TRACK_DELETE id={track.track_id}")

    def zone_entry(self, track: Track, zone_id: int):
        """Send ZONE_ENTRY event"""
        events = [{
            "type": "ZONE_ENTRY",
            "attributes": {"track_id": track.track_id, "geometry_id": zone_id}
        }]
        tracked_objects = [{
            "track_id": track.track_id,
            "type": "PERSON",
            "position": track.position()
        }]
        self._publish(self._build_message(events, tracked_objects))
        print(f"  ZONE_ENTRY id={track.track_id} zone={zone_id}")

    def zone_exit(self, track: Track, zone_id: int):
        """Send ZONE_EXIT event"""
        events = [{
            "type": "ZONE_EXIT",
            "attributes": {"track_id": track.track_id, "geometry_id": zone_id}
        }]
        tracked_objects = [{
            "track_id": track.track_id,
            "type": "PERSON",
            "position": track.position()
        }]
        self._publish(self._build_message(events, tracked_objects))
        print(f"  ZONE_EXIT id={track.track_id} zone={zone_id}")

    def line_cross_forward(self, track: Track, line_id: int):
        """Send LINE_CROSS_FORWARD event"""
        events = [{
            "type": "LINE_CROSS_FORWARD",
            "attributes": {
                "track_id": track.track_id,
                "geometry_id": line_id,
                "direction": "forward"
            }
        }]
        tracked_objects = [{
            "track_id": track.track_id,
            "type": "PERSON",
            "position": track.position()
        }]
        self._publish(self._build_message(events, tracked_objects))
        print(f"  LINE_CROSS_FORWARD id={track.track_id} line={line_id}")

    def line_cross_backward(self, track: Track, line_id: int):
        """Send LINE_CROSS_BACKWARD event"""
        events = [{
            "type": "LINE_CROSS_BACKWARD",
            "attributes": {
                "track_id": track.track_id,
                "geometry_id": line_id,
                "direction": "backward"
            }
        }]
        tracked_objects = [{
            "track_id": track.track_id,
            "type": "PERSON",
            "position": track.position()
        }]
        self._publish(self._build_message(events, tracked_objects))
        print(f"  LINE_CROSS_BACKWARD id={track.track_id} line={line_id}")


class TestScenarios:
    """Collection of test scenarios"""

    def __init__(self, gen: XovisTrafficGenerator):
        self.gen = gen

    def scenario_normal_authorized_journey(self):
        """Normal journey: create -> dwell in POS -> gate zone -> exit"""
        print("\n=== SCENARIO: Normal Authorized Journey ===")

        track = Track(self.gen.next_track_id(), 2.0, 3.0, 1.75)

        # Track created
        self.gen.track_create(track)
        time.sleep(0.2)

        # Enter POS zone
        self.gen.zone_entry(track, POS_ZONES[0])
        print("  (dwelling for 8 seconds to authorize...)")
        time.sleep(8.0)  # Dwell to authorize (> 7s)

        # Exit POS zone (should be authorized now)
        self.gen.zone_exit(track, POS_ZONES[0])
        time.sleep(0.3)

        # Enter gate zone (should trigger gate open)
        self.gen.zone_entry(track, GATE_ZONE)
        time.sleep(0.5)

        # Cross exit line (journey complete)
        self.gen.line_cross_forward(track, EXIT_LINE)
        time.sleep(0.2)

        print("=== Journey should be COMPLETED and authorized ===\n")

    def scenario_unauthorized_journey(self):
        """Journey without enough dwell time (unauthorized)"""
        print("\n=== SCENARIO: Unauthorized Journey (insufficient dwell) ===")

        track = Track(self.gen.next_track_id(), 3.0, 4.0, 1.80)

        self.gen.track_create(track)
        time.sleep(0.2)

        # Brief POS zone visit (not enough to authorize)
        self.gen.zone_entry(track, POS_ZONES[1])
        time.sleep(2.0)  # Only 2s dwell (< 7s threshold)
        self.gen.zone_exit(track, POS_ZONES[1])
        time.sleep(0.3)

        # Enter gate zone (should NOT trigger gate - unauthorized)
        self.gen.zone_entry(track, GATE_ZONE)
        time.sleep(0.3)

        # Cross exit line
        self.gen.line_cross_forward(track, EXIT_LINE)
        time.sleep(0.2)

        print("=== Journey should be COMPLETED but UNAUTHORIZED ===\n")

    def scenario_track_stitch(self):
        """Track stitching: delete + create nearby within time window"""
        print("\n=== SCENARIO: Track Stitching ===")

        # Create and authorize a track
        track1 = Track(self.gen.next_track_id(), 2.5, 3.5, 1.72)
        self.gen.track_create(track1)
        time.sleep(0.2)

        # Authorize via dwell
        self.gen.zone_entry(track1, POS_ZONES[2])
        print("  (dwelling for 8 seconds to authorize...)")
        time.sleep(8.0)
        self.gen.zone_exit(track1, POS_ZONES[2])
        time.sleep(0.3)

        # Delete track (goes to stitch pending)
        self.gen.track_delete(track1)
        time.sleep(0.5)  # Brief gap

        # New track appears nearby (should stitch and inherit authorization)
        track2 = Track(self.gen.next_track_id(), 2.55, 3.52, 1.73)  # Similar position
        self.gen.track_create(track2)
        time.sleep(0.3)

        # Continue to gate (should still be authorized from stitch)
        self.gen.zone_entry(track2, GATE_ZONE)
        time.sleep(0.3)

        # Exit
        self.gen.line_cross_forward(track2, EXIT_LINE)
        time.sleep(0.2)

        print("=== Stitched journey should preserve authorization ===\n")

    def scenario_stitch_timeout(self):
        """Stitch failure: too much time between delete and create"""
        print("\n=== SCENARIO: Stitch Timeout (should NOT stitch) ===")

        track1 = Track(self.gen.next_track_id(), 1.0, 2.0, 1.65)
        self.gen.track_create(track1)
        time.sleep(0.2)

        # Authorize
        self.gen.zone_entry(track1, POS_ZONES[3])
        print("  (dwelling for 8 seconds...)")
        time.sleep(8.0)
        self.gen.zone_exit(track1, POS_ZONES[3])
        time.sleep(0.2)

        # Delete track
        self.gen.track_delete(track1)

        # Wait beyond stitch window (4.5s)
        print("  (waiting 5 seconds - beyond stitch window...)")
        time.sleep(5.0)

        # New track - should NOT inherit state
        track2 = Track(self.gen.next_track_id(), 1.05, 2.02, 1.66)
        self.gen.track_create(track2)
        time.sleep(0.2)

        # Try gate zone (should NOT trigger - not authorized)
        self.gen.zone_entry(track2, GATE_ZONE)
        time.sleep(0.3)

        self.gen.line_cross_forward(track2, EXIT_LINE)
        time.sleep(0.2)

        print("=== New track should be UNAUTHORIZED (no stitch) ===\n")

    def scenario_stitch_too_far(self):
        """Stitch failure: new track too far away"""
        print("\n=== SCENARIO: Stitch Too Far (should NOT stitch) ===")

        track1 = Track(self.gen.next_track_id(), 1.0, 1.0, 1.70)
        self.gen.track_create(track1)
        time.sleep(0.2)

        # Authorize
        self.gen.zone_entry(track1, POS_ZONES[0])
        print("  (dwelling for 8 seconds...)")
        time.sleep(8.0)
        self.gen.zone_exit(track1, POS_ZONES[0])
        time.sleep(0.2)

        # Delete track
        self.gen.track_delete(track1)
        time.sleep(0.3)

        # New track 5 meters away - should NOT stitch (>1.8m threshold)
        track2 = Track(self.gen.next_track_id(), 6.0, 1.0, 1.70)
        self.gen.track_create(track2)
        time.sleep(0.2)

        # Try gate
        self.gen.zone_entry(track2, GATE_ZONE)
        time.sleep(0.3)

        self.gen.line_cross_forward(track2, EXIT_LINE)
        time.sleep(0.2)

        print("=== New track should be UNAUTHORIZED (too far to stitch) ===\n")

    def scenario_multiple_pos_zones(self):
        """Accumulated dwell across multiple POS zones"""
        print("\n=== SCENARIO: Multiple POS Zone Visits ===")

        track = Track(self.gen.next_track_id(), 2.0, 2.0, 1.78)
        self.gen.track_create(track)
        time.sleep(0.2)

        # Visit POS_1 briefly (4s)
        self.gen.zone_entry(track, POS_ZONES[0])
        print("  (4s in POS_1...)")
        time.sleep(4.0)
        self.gen.zone_exit(track, POS_ZONES[0])
        time.sleep(0.3)

        # Visit POS_2 briefly (4s) - total now 8s, should authorize
        self.gen.zone_entry(track, POS_ZONES[1])
        print("  (4s in POS_2 - cumulative should authorize...)")
        time.sleep(4.0)
        self.gen.zone_exit(track, POS_ZONES[1])
        time.sleep(0.3)

        # Gate zone
        self.gen.zone_entry(track, GATE_ZONE)
        time.sleep(0.3)

        # Exit
        self.gen.line_cross_forward(track, EXIT_LINE)
        time.sleep(0.2)

        print("=== Should be AUTHORIZED via accumulated dwell ===\n")

    def scenario_rapid_events(self):
        """Rapid fire events - stress test"""
        print("\n=== SCENARIO: Rapid Events (stress test) ===")

        tracks = []

        # Create 5 tracks rapidly
        print("  Creating 5 tracks rapidly...")
        for i in range(5):
            track = Track(self.gen.next_track_id(), 1.0 + i * 0.5, 2.0, 1.70 + i * 0.02)
            tracks.append(track)
            self.gen.track_create(track)
            time.sleep(0.05)  # 50ms between

        time.sleep(0.3)

        # All enter POS zones simultaneously
        print("  All entering POS zones...")
        for i, track in enumerate(tracks):
            self.gen.zone_entry(track, POS_ZONES[i % len(POS_ZONES)])
            time.sleep(0.05)

        # Short dwell (not enough to authorize)
        time.sleep(2.0)

        # All exit POS zones
        print("  All exiting POS zones...")
        for i, track in enumerate(tracks):
            self.gen.zone_exit(track, POS_ZONES[i % len(POS_ZONES)])
            time.sleep(0.05)

        time.sleep(0.2)

        # All cross exit line
        print("  All crossing exit line...")
        for track in tracks:
            self.gen.line_cross_forward(track, EXIT_LINE)
            time.sleep(0.05)

        print("=== 5 rapid journeys should complete (all unauthorized) ===\n")

    def scenario_malformed_events(self):
        """Edge case: events for non-existent tracks"""
        print("\n=== SCENARIO: Events for Non-Existent Track ===")

        # Zone entry for track that was never created
        ghost_track = Track(99999, 0.0, 0.0, 0.0)
        self.gen.zone_entry(ghost_track, POS_ZONES[0])
        time.sleep(0.2)

        self.gen.zone_exit(ghost_track, POS_ZONES[0])
        time.sleep(0.2)

        # Delete track that doesn't exist
        self.gen.track_delete(ghost_track)
        time.sleep(0.2)

        print("=== Should handle gracefully without crash ===\n")

    def scenario_out_of_order_events(self):
        """Edge case: out of order events"""
        print("\n=== SCENARIO: Out of Order Events ===")

        track = Track(self.gen.next_track_id(), 3.0, 3.0, 1.75)

        # Zone exit BEFORE create (shouldn't crash)
        self.gen.zone_exit(track, POS_ZONES[0])
        time.sleep(0.1)

        # Now create
        self.gen.track_create(track)
        time.sleep(0.2)

        # Zone entry
        self.gen.zone_entry(track, POS_ZONES[0])
        time.sleep(0.2)

        # Another zone entry without exit (double entry)
        self.gen.zone_entry(track, POS_ZONES[1])
        time.sleep(0.2)

        # Delete
        self.gen.track_delete(track)
        time.sleep(0.2)

        print("=== Should handle gracefully ===\n")

    def scenario_reentry_detection(self):
        """Re-entry: person exits and returns within time window"""
        print("\n=== SCENARIO: Re-entry Detection ===")

        # First journey
        track1 = Track(self.gen.next_track_id(), 2.0, 2.0, 1.82)  # Specific height
        self.gen.track_create(track1)
        time.sleep(0.2)

        # Authorize
        self.gen.zone_entry(track1, POS_ZONES[0])
        print("  (dwelling 8s to authorize...)")
        time.sleep(8.0)
        self.gen.zone_exit(track1, POS_ZONES[0])
        time.sleep(0.2)

        # Exit store
        self.gen.line_cross_forward(track1, EXIT_LINE)
        time.sleep(0.2)

        print("  (person left - waiting 5 seconds then returning...)")
        time.sleep(5.0)

        # Person returns (within 30s window, similar height)
        track2 = Track(self.gen.next_track_id(), 1.0, 1.0, 1.83)  # Similar height
        self.gen.track_create(track2)
        time.sleep(0.2)

        # Quick journey back through
        self.gen.zone_entry(track2, GATE_ZONE)
        time.sleep(0.2)

        self.gen.line_cross_forward(track2, EXIT_LINE)
        time.sleep(0.2)

        print("=== Should detect re-entry and link journeys ===\n")

    def scenario_long_running(self, duration_minutes: int = 2):
        """Long running test with mixed traffic"""
        print(f"\n=== SCENARIO: Long Running Test ({duration_minutes} minutes) ===")

        end_time = time.time() + (duration_minutes * 60)
        journey_count = 0

        while time.time() < end_time:
            # Random scenario
            scenario = random.choice([
                'authorized', 'unauthorized', 'stitch', 'rapid'
            ])

            journey_count += 1
            print(f"\n[Journey #{journey_count}] Running mini-scenario: {scenario}")

            track = Track(self.gen.next_track_id(),
                         random.uniform(0.5, 5.0),
                         random.uniform(0.5, 5.0),
                         random.uniform(1.50, 2.00))

            self.gen.track_create(track)
            time.sleep(0.1)

            # POS zone visit
            pos_zone = random.choice(POS_ZONES)
            self.gen.zone_entry(track, pos_zone)

            if scenario == 'authorized':
                time.sleep(random.uniform(7.5, 10.0))  # Enough to authorize
            else:
                time.sleep(random.uniform(0.5, 3.0))  # Not enough

            self.gen.zone_exit(track, pos_zone)
            time.sleep(0.1)

            if scenario == 'stitch':
                # Delete and recreate nearby
                self.gen.track_delete(track)
                time.sleep(0.3)
                track = Track(self.gen.next_track_id(),
                             track.x + random.uniform(-0.1, 0.1),
                             track.y + random.uniform(-0.1, 0.1),
                             track.height + random.uniform(-0.02, 0.02))
                self.gen.track_create(track)
                time.sleep(0.1)

            # Gate zone
            self.gen.zone_entry(track, GATE_ZONE)
            time.sleep(0.2)

            # Exit
            self.gen.line_cross_forward(track, EXIT_LINE)
            time.sleep(0.2)

        print(f"\n=== Completed {journey_count} journeys over {duration_minutes} minutes ===\n")


def main():
    parser = argparse.ArgumentParser(description='Xovis Traffic Generator for Gateway PoC Testing')
    parser.add_argument('--host', default='localhost', help='MQTT broker host')
    parser.add_argument('--port', type=int, default=1883, help='MQTT broker port')
    parser.add_argument('--topic', default='xovis/live', help='MQTT topic')
    parser.add_argument('--scenario', default='all',
                       choices=['all', 'normal', 'unauthorized', 'stitch', 'stitch_timeout',
                               'stitch_far', 'multi_pos', 'rapid', 'malformed',
                               'out_of_order', 'reentry', 'long'],
                       help='Test scenario to run')
    parser.add_argument('--duration', type=int, default=2,
                       help='Duration in minutes for long-running test')

    args = parser.parse_args()

    gen = XovisTrafficGenerator(args.host, args.port, args.topic)

    try:
        gen.connect()
        time.sleep(1)  # Let gateway initialize

        scenarios = TestScenarios(gen)

        if args.scenario == 'all':
            print("\n" + "="*60)
            print("Running ALL test scenarios")
            print("="*60)

            scenarios.scenario_normal_authorized_journey()
            scenarios.scenario_unauthorized_journey()
            scenarios.scenario_track_stitch()
            scenarios.scenario_stitch_timeout()
            scenarios.scenario_stitch_too_far()
            scenarios.scenario_multiple_pos_zones()
            scenarios.scenario_rapid_events()
            scenarios.scenario_malformed_events()
            scenarios.scenario_out_of_order_events()
            scenarios.scenario_reentry_detection()

            print("\n" + "="*60)
            print("ALL SCENARIOS COMPLETED")
            print("="*60 + "\n")

        elif args.scenario == 'normal':
            scenarios.scenario_normal_authorized_journey()
        elif args.scenario == 'unauthorized':
            scenarios.scenario_unauthorized_journey()
        elif args.scenario == 'stitch':
            scenarios.scenario_track_stitch()
        elif args.scenario == 'stitch_timeout':
            scenarios.scenario_stitch_timeout()
        elif args.scenario == 'stitch_far':
            scenarios.scenario_stitch_too_far()
        elif args.scenario == 'multi_pos':
            scenarios.scenario_multiple_pos_zones()
        elif args.scenario == 'rapid':
            scenarios.scenario_rapid_events()
        elif args.scenario == 'malformed':
            scenarios.scenario_malformed_events()
        elif args.scenario == 'out_of_order':
            scenarios.scenario_out_of_order_events()
        elif args.scenario == 'reentry':
            scenarios.scenario_reentry_detection()
        elif args.scenario == 'long':
            scenarios.scenario_long_running(args.duration)

    except KeyboardInterrupt:
        print("\nInterrupted by user")
    finally:
        gen.disconnect()


if __name__ == '__main__':
    main()
