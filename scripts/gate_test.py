#!/usr/bin/env python3
"""
Gate Behavior Investigation Tool

Investigates how the gate responds to multiple open commands at various intervals.
Records all state transitions and timing to predict gate behavior patterns.

Usage:
    python3 gate_test.py --tcp 192.168.0.245:8000 --rs485 /dev/ttyAMA4
"""

import argparse
import socket
import serial
import time
import struct
import threading
import select
from dataclasses import dataclass
from typing import Optional, List, Tuple
from datetime import datetime

# CloudPlus protocol constants
STX = 0x02
ETX = 0x03
CMD_HEARTBEAT = 0x56
CMD_OPEN_DOOR = 0x2C

# RS485 protocol constants
RS485_START_CMD = 0x7E
RS485_START_RSP = 0x7F
RS485_CMD_QUERY = 0x10

# Door status codes (from RS485 protocol)
DOOR_CLOSED_PROPERLY = 0x00
DOOR_LEFT_OPEN_PROPERLY = 0x01  # Left wing open = door OPEN
DOOR_RIGHT_OPEN_PROPERLY = 0x02  # Right wing open = resting position = CLOSED
DOOR_IN_MOTION = 0x03
DOOR_FIRE_SIGNAL = 0x04


class DoorStatus:
    CLOSED = "CLOSED"
    OPEN = "OPEN"
    MOVING = "MOVING"
    UNKNOWN = "UNKNOWN"

    @staticmethod
    def from_code(code: int) -> str:
        # Match Rust implementation exactly
        if code == DOOR_CLOSED_PROPERLY:
            return DoorStatus.CLOSED
        elif code == DOOR_LEFT_OPEN_PROPERLY:
            return DoorStatus.OPEN
        elif code == DOOR_RIGHT_OPEN_PROPERLY:
            return DoorStatus.CLOSED  # Right open = resting position = closed
        elif code == DOOR_IN_MOTION:
            return DoorStatus.MOVING
        elif code == DOOR_FIRE_SIGNAL:
            return DoorStatus.OPEN
        return DoorStatus.UNKNOWN


@dataclass
class StateEvent:
    time_ms: int
    event: str


def calculate_checksum(data: bytes) -> int:
    """XOR all bytes together."""
    result = 0
    for b in data:
        result ^= b
    return result


def build_heartbeat_ack(rand: int, oem_code: int) -> bytes:
    """Build heartbeat acknowledgment frame."""
    hi = (oem_code >> 8) & 0xFF
    lo = oem_code & 0xFF
    # Format: [STX, rand, cmd, address, door, data_len_lo, data_len_hi, data..., checksum, ETX]
    frame = bytes([STX, rand, CMD_HEARTBEAT, 0x00, 0x00, 0x02, 0x00, hi, lo])
    checksum = calculate_checksum(frame)
    return frame + bytes([checksum, ETX])


class GateInvestigator:
    def __init__(self, tcp_addr: str, rs485_device: str, baud: int = 19200):
        self.tcp_addr = tcp_addr
        self.rs485_device = rs485_device
        self.baud = baud
        self.tcp_socket: Optional[socket.socket] = None
        self.rs485_port: Optional[serial.Serial] = None
        self.last_status = DoorStatus.UNKNOWN
        self.start_time = time.time()
        self.events: List[StateEvent] = []
        self.open_cmd_count = 0
        self.heartbeat_count = 0
        self.heartbeat_thread: Optional[threading.Thread] = None
        self.heartbeat_stop = threading.Event()
        self.socket_lock = threading.Lock()

    def elapsed_ms(self) -> int:
        return int((time.time() - self.start_time) * 1000)

    def reset_timer(self):
        self.start_time = time.time()
        self.events.clear()
        self.open_cmd_count = 0

    def connect_tcp(self) -> bool:
        try:
            host, port = self.tcp_addr.split(":")
            print(f"Connecting to CloudPlus at {self.tcp_addr}...")
            self.tcp_socket = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
            self.tcp_socket.settimeout(5.0)
            self.tcp_socket.connect((host, int(port)))
            self.tcp_socket.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
            self.tcp_socket.setblocking(False)
            print("CloudPlus connected ✓")

            # Start heartbeat handler thread
            self.heartbeat_stop.clear()
            self.heartbeat_thread = threading.Thread(target=self._heartbeat_loop, daemon=True)
            self.heartbeat_thread.start()
            print("Heartbeat handler started ✓")

            return True
        except Exception as e:
            print(f"TCP connection failed: {e}")
            return False

    def _heartbeat_loop(self):
        """Background thread to handle CloudPlus heartbeats."""
        buf = b""
        while not self.heartbeat_stop.is_set():
            if not self.tcp_socket:
                break

            try:
                # Use select to wait for data with timeout
                ready, _, _ = select.select([self.tcp_socket], [], [], 0.1)
                if not ready:
                    continue

                # Read available data
                with self.socket_lock:
                    if not self.tcp_socket:
                        break
                    try:
                        data = self.tcp_socket.recv(4096)
                        if not data:
                            print("  [HB] Connection closed by peer")
                            break
                        buf += data
                    except BlockingIOError:
                        continue
                    except Exception as e:
                        print(f"  [HB] Read error: {e}")
                        break

                # Parse frames from buffer
                while len(buf) >= 9:
                    # Find STX
                    stx_idx = buf.find(bytes([STX]))
                    if stx_idx == -1:
                        buf = b""
                        break
                    if stx_idx > 0:
                        buf = buf[stx_idx:]

                    if len(buf) < 7:
                        break

                    # Parse header
                    rand = buf[1]
                    cmd = buf[2]
                    # Device→Server: length bytes are swapped (high, low)
                    data_len = buf[6] | (buf[5] << 8)

                    frame_len = 7 + data_len + 2  # header + data + checksum + ETX
                    if len(buf) < frame_len:
                        break

                    # Verify ETX
                    if buf[frame_len - 1] != ETX:
                        buf = buf[1:]  # Skip bad STX, resync
                        continue

                    # Process frame
                    if cmd == CMD_HEARTBEAT and data_len >= 21:
                        # Extract OEM code from heartbeat data (bytes 19-20)
                        frame_data = buf[7:7 + data_len]
                        oem_code = frame_data[19] | (frame_data[20] << 8)

                        # Send ack
                        ack = build_heartbeat_ack(rand, oem_code)
                        with self.socket_lock:
                            if self.tcp_socket:
                                try:
                                    self.tcp_socket.sendall(ack)
                                    self.heartbeat_count += 1
                                    if self.heartbeat_count <= 3 or self.heartbeat_count % 10 == 0:
                                        print(f"  [HB] Heartbeat #{self.heartbeat_count} acked")
                                except Exception as e:
                                    print(f"  [HB] Send error: {e}")
                                    break

                    # Consume frame
                    buf = buf[frame_len:]

            except Exception as e:
                print(f"  [HB] Error: {e}")
                time.sleep(0.1)

    def stop_heartbeat(self):
        """Stop the heartbeat handler thread."""
        self.heartbeat_stop.set()
        if self.heartbeat_thread:
            self.heartbeat_thread.join(timeout=1.0)

    def connect_rs485(self) -> bool:
        try:
            print(f"Opening RS485 at {self.rs485_device} @ {self.baud}baud...")
            self.rs485_port = serial.Serial(
                self.rs485_device,
                baudrate=self.baud,
                timeout=0.2,
                bytesize=serial.EIGHTBITS,
                parity=serial.PARITY_NONE,
                stopbits=serial.STOPBITS_ONE,
            )
            print("RS485 opened ✓")
            return True
        except Exception as e:
            print(f"RS485 open failed: {e}")
            return False

    def send_open(self) -> bool:
        self.open_cmd_count += 1
        cmd_num = self.open_cmd_count
        t = self.elapsed_ms()

        # Build CloudPlus open command
        # Format: [STX, rand, cmd, address, door, data_len_lo, data_len_hi]
        frame = bytes([STX, 0x00, CMD_OPEN_DOOR, 0xff, 0x01, 0x00, 0x00])
        checksum = calculate_checksum(frame)
        frame = frame + bytes([checksum, ETX])

        print(f"[{t:>6}ms] >>> OPEN_CMD_{cmd_num}")
        self.events.append(StateEvent(t, f"OPEN_CMD_{cmd_num}"))

        # Try to send with socket lock for thread safety
        for attempt in range(2):
            if not self.tcp_socket:
                if not self.connect_tcp():
                    return False
            try:
                with self.socket_lock:
                    if self.tcp_socket:
                        self.tcp_socket.sendall(frame)
                return True
            except (BrokenPipeError, ConnectionResetError, OSError) as e:
                if attempt == 0:
                    print(f"  Connection lost, reconnecting...")
                    self.stop_heartbeat()
                    self.tcp_socket = None
                else:
                    print(f"Send failed: {e}")
                    return False
        return False

    def poll_door_status(self, debug: bool = False) -> Optional[str]:
        """Poll RS485 for door status. Matches Rust implementation exactly."""
        if not self.rs485_port:
            return None

        # Build query command (8 bytes) - matches Rust build_query_command()
        # Format: [start, undefined, machine_num, cmd, data0, data1, data2, checksum]
        frame = [RS485_START_CMD, 0x00, 0x01, RS485_CMD_QUERY, 0x00, 0x00, 0x00]
        checksum = (~sum(frame)) & 0xFF
        cmd = bytes(frame + [checksum])

        try:
            # Clear any pending input
            self.rs485_port.reset_input_buffer()
            self.rs485_port.write(cmd)

            # Read response - may need multiple reads like Rust
            response = b""
            deadline = time.time() + 0.2  # 200ms timeout like Rust
            while len(response) < 18 and time.time() < deadline:
                chunk = self.rs485_port.read(64 - len(response))
                if chunk:
                    response += chunk
                    # Check if we have enough for a frame
                    if len(response) >= 18:
                        break

            if len(response) >= 18:
                # Find response start byte (0x7F) - handles noise/sync issues
                for i in range(len(response)):
                    if response[i] == RS485_START_RSP:
                        if i + 18 <= len(response):
                            frame_data = response[i:i+18]

                            # Validate checksum: sum all bytes (including checksum), add 1, should be 0
                            frame_sum = sum(frame_data) & 0xFF
                            if (frame_sum + 1) & 0xFF != 0:
                                if debug:
                                    print(f"  [RS485] checksum failed: sum={frame_sum}")
                                continue

                            # Door status is at offset 4 from start byte
                            status_byte = frame_data[4]
                            if debug:
                                hex_resp = ' '.join(f'{b:02X}' for b in frame_data)
                                print(f"  [RS485] door=0x{status_byte:02X}: {hex_resp}")
                            return DoorStatus.from_code(status_byte)

            if debug and len(response) > 0:
                hex_resp = ' '.join(f'{b:02X}' for b in response)
                print(f"  [RS485] incomplete ({len(response)} bytes): {hex_resp}")
        except Exception as e:
            if debug:
                print(f"  [RS485] error: {e}")

        return None

    def record_status_change(self, new_status: str) -> bool:
        if new_status != self.last_status:
            t = self.elapsed_ms()
            print(f"[{t:>6}ms] DOOR: {self.last_status} -> {new_status}")
            self.events.append(StateEvent(t, new_status))
            self.last_status = new_status
            return True
        return False

    def wait_for_closed(self, timeout_s: float = 30.0) -> bool:
        deadline = time.time() + timeout_s
        while time.time() < deadline:
            status = self.poll_door_status()
            if status:
                self.record_status_change(status)
                if status == DoorStatus.CLOSED:
                    return True
            time.sleep(0.05)
        return False

    def monitor_until_closed(self, timeout_s: float = 60.0):
        """Monitor door until it closes after opening."""
        deadline = time.time() + timeout_s
        saw_open_or_moving = False
        poll_count = 0
        none_count = 0

        while time.time() < deadline:
            status = self.poll_door_status()
            poll_count += 1

            if status is None:
                none_count += 1
            elif status:
                self.record_status_change(status)
                if status in (DoorStatus.OPEN, DoorStatus.MOVING):
                    saw_open_or_moving = True
                elif status == DoorStatus.CLOSED and saw_open_or_moving:
                    # Door opened and now closed - confirm it's stable
                    time.sleep(0.5)
                    status2 = self.poll_door_status()
                    if status2 == DoorStatus.CLOSED:
                        return
            time.sleep(0.25)  # Poll every 250ms like gateway

        # Debug: if we never saw states, print poll stats
        if not saw_open_or_moving:
            print(f"  ⚠ RS485 polling: {poll_count} polls, {none_count} returned None")

    def analyze_events(self) -> dict:
        result = {
            "first_cmd_at": 0,
            "cmd_to_moving_ms": None,
            "cmd_to_open_ms": None,
            "open_duration_ms": None,
            "total_open_cmds": self.open_cmd_count,
        }

        first_cmd = None
        first_moving = None
        first_open = None
        last_closed = None

        for e in self.events:
            if first_cmd is None and e.event.startswith("OPEN_CMD"):
                first_cmd = e.time_ms
            if first_moving is None and e.event == DoorStatus.MOVING:
                first_moving = e.time_ms
            if first_open is None and e.event == DoorStatus.OPEN:
                first_open = e.time_ms
            if e.event == DoorStatus.CLOSED:
                last_closed = e.time_ms

        if first_cmd is not None:
            result["first_cmd_at"] = first_cmd
        if first_cmd is not None and first_moving is not None:
            result["cmd_to_moving_ms"] = first_moving - first_cmd
        if first_cmd is not None and first_open is not None:
            result["cmd_to_open_ms"] = first_open - first_cmd
        if first_open is not None and last_closed is not None and last_closed > first_open:
            result["open_duration_ms"] = last_closed - first_open

        return result

    def print_timeline(self):
        print("\n  Timeline:")
        for e in self.events:
            print(f"    [{e.time_ms:>6}ms] {e.event}")

    def run_test(self, name: str, open_intervals_ms: List[int]) -> dict:
        print(f"\n{'=' * 60}")
        print(f"TEST: {name}")
        print(f"  Open commands: {len(open_intervals_ms) + 1} with intervals {open_intervals_ms}ms")
        print("=" * 60)

        print("\n  Waiting for door to be CLOSED...")
        if not self.wait_for_closed(30.0):
            print("  ⚠ Timeout waiting for door to close")
            return {}

        self.reset_timer()
        time.sleep(0.2)

        # Send first open
        self.send_open()

        # Send subsequent opens at specified intervals
        for interval_ms in open_intervals_ms:
            time.sleep(interval_ms / 1000.0)
            status = self.poll_door_status()
            if status:
                self.record_status_change(status)
            self.send_open()

        # Monitor until door closes
        self.monitor_until_closed(60.0)

        self.print_timeline()

        result = self.analyze_events()
        print("\n  Results:")
        print(f"    Commands sent: {result['total_open_cmds']}")
        if result["cmd_to_moving_ms"] is not None:
            print(f"    Cmd → Moving: {result['cmd_to_moving_ms']}ms")
        if result["cmd_to_open_ms"] is not None:
            print(f"    Cmd → Open: {result['cmd_to_open_ms']}ms")
        if result["open_duration_ms"] is not None:
            print(f"    Open duration: {result['open_duration_ms']}ms ({result['open_duration_ms']/1000:.1f}s)")

        return result


def main():
    parser = argparse.ArgumentParser(description="Gate behavior investigation tool")
    parser.add_argument("--tcp", default="192.168.0.245:8000", help="CloudPlus TCP address")
    parser.add_argument("--rs485", default="/dev/ttyAMA4", help="RS485 device path")
    parser.add_argument("--baud", type=int, default=19200, help="RS485 baud rate")
    parser.add_argument("--fast", action="store_true", help="Skip waiting between tests")
    parser.add_argument("--monitor-only", action="store_true", help="Only monitor door state")
    parser.add_argument("--wait", type=int, default=3, help="Wait seconds between tests")
    args = parser.parse_args()

    print("\n╔══════════════════════════════════════════════════════════╗")
    print("║         GATE BEHAVIOR INVESTIGATION TOOL                 ║")
    print("╚══════════════════════════════════════════════════════════╝")
    print(f"\nConfiguration:")
    print(f"  CloudPlus TCP: {args.tcp}")
    print(f"  RS485 device:  {args.rs485} @ {args.baud}baud")
    print()

    investigator = GateInvestigator(args.tcp, args.rs485, args.baud)

    if not investigator.connect_rs485():
        return 1

    if args.monitor_only:
        print("\n=== MONITOR MODE (no commands) ===\n")
        try:
            while True:
                status = investigator.poll_door_status()
                if status:
                    investigator.record_status_change(status)
                time.sleep(0.1)
        except KeyboardInterrupt:
            print("\nMonitor stopped.")
        return 0

    if not investigator.connect_tcp():
        return 1

    results: List[Tuple[str, dict]] = []
    wait_s = 0 if args.fast else args.wait

    # ═══════════════════════════════════════════════════════════════════
    # PHASE 1: Baseline measurements (3 runs for consistency)
    # ═══════════════════════════════════════════════════════════════════
    print("\n\n▶ PHASE 1: Baseline (3 runs)")
    for i in range(3):
        r = investigator.run_test(f"Baseline #{i+1}", [])
        results.append((f"Baseline #{i+1}", r))
        if wait_s and i < 2:
            print(f"\n  ⏳ Waiting {wait_s}s...")
            time.sleep(wait_s)

    # Calculate average baseline
    baseline_durations = [r.get("open_duration_ms", 0) for _, r in results if r.get("open_duration_ms")]
    avg_baseline = sum(baseline_durations) // len(baseline_durations) if baseline_durations else 10000
    print(f"\n  📊 Average baseline open duration: {avg_baseline}ms ({avg_baseline/1000:.1f}s)")

    if wait_s:
        time.sleep(wait_s)

    # ═══════════════════════════════════════════════════════════════════
    # PHASE 2: Double open at 0.5s increments (0.5s to 5s)
    # Tests: during opening, while open
    # ═══════════════════════════════════════════════════════════════════
    print("\n\n▶ PHASE 2: Double open - early phase (0.5s to 5s)")
    for delay_ms in range(500, 5500, 500):
        name = f"Double @{delay_ms/1000:.1f}s"
        r = investigator.run_test(name, [delay_ms])
        results.append((name, r))
        if wait_s:
            print(f"\n  ⏳ Waiting {wait_s}s...")
            time.sleep(wait_s)

    # ═══════════════════════════════════════════════════════════════════
    # PHASE 3: Double open at 0.5s increments (6s to 12s)
    # Tests: while open, during closing, after closed
    # ═══════════════════════════════════════════════════════════════════
    print("\n\n▶ PHASE 3: Double open - late phase (6s to 12s)")
    for delay_ms in range(6000, 12500, 500):
        name = f"Double @{delay_ms/1000:.1f}s"
        r = investigator.run_test(name, [delay_ms])
        results.append((name, r))
        if wait_s:
            print(f"\n  ⏳ Waiting {wait_s}s...")
            time.sleep(wait_s)

    # ═══════════════════════════════════════════════════════════════════
    # PHASE 4: Triple opens to test cumulative effect
    # ═══════════════════════════════════════════════════════════════════
    print("\n\n▶ PHASE 4: Triple opens")
    for delay_ms in [500, 1000, 2000, 3000]:
        name = f"Triple @{delay_ms/1000:.1f}s"
        r = investigator.run_test(name, [delay_ms, delay_ms])
        results.append((name, r))
        if wait_s:
            print(f"\n  ⏳ Waiting {wait_s}s...")
            time.sleep(wait_s)

    # Print summary
    print("\n\n╔══════════════════════════════════════════════════════════════════════════╗")
    print("║                              SUMMARY                                      ║")
    print("╚══════════════════════════════════════════════════════════════════════════╝\n")
    print(f"{'Test':<20} {'Cmds':>5} {'Cmd→Open':>10} {'Duration':>10} {'Delta':>10} {'Door State at Cmd2':>20}")
    print("-" * 80)

    # Use average of baseline runs
    baseline_results = [(n, r) for n, r in results if n.startswith("Baseline")]
    baseline_durations = [r.get("open_duration_ms", 0) for _, r in baseline_results if r.get("open_duration_ms")]
    avg_baseline = sum(baseline_durations) // len(baseline_durations) if baseline_durations else 10000

    for name, r in results:
        cmd_open = f"{r.get('cmd_to_open_ms', 0)}ms" if r.get("cmd_to_open_ms") else "-"
        if r.get("open_duration_ms"):
            ms = r["open_duration_ms"]
            delta = ms - avg_baseline
            sign = "+" if delta >= 0 else ""
            duration = f"{ms/1000:.1f}s"
            delta_str = f"{sign}{delta}ms"
        else:
            duration = "-"
            delta_str = "-"

        # Determine door state when 2nd command was sent (for double opens)
        door_state = "-"
        if "Double @" in name:
            delay_str = name.replace("Double @", "").replace("s", "")
            try:
                delay_s = float(delay_str)
                if delay_s < 2.5:
                    door_state = "OPENING"
                elif delay_s < 7.5:
                    door_state = "OPEN"
                elif delay_s < 10.5:
                    door_state = "CLOSING"
                else:
                    door_state = "CLOSED"
            except:
                pass

        print(f"{name:<20} {r.get('total_open_cmds', 0):>5} {cmd_open:>10} {duration:>10} {delta_str:>10} {door_state:>20}")

    print(f"\n  📊 Average baseline: {avg_baseline}ms ({avg_baseline/1000:.1f}s)")

    print("\n\n╔══════════════════════════════════════════════════════════════════════════╗")
    print("║                              ANALYSIS                                     ║")
    print("╚══════════════════════════════════════════════════════════════════════════╝\n")

    # Group results by phase
    double_results = [(n, r) for n, r in results if n.startswith("Double @")]

    # Analyze by door state phase
    print("Extension by door state when 2nd command sent:")
    print("-" * 50)

    phases = {
        "OPENING (0-2.5s)": [],
        "OPEN (2.5-7.5s)": [],
        "CLOSING (7.5-10.5s)": [],
        "CLOSED (>10.5s)": [],
    }

    for name, r in double_results:
        if not r.get("open_duration_ms"):
            continue
        delay_str = name.replace("Double @", "").replace("s", "")
        try:
            delay_s = float(delay_str)
            delta = r["open_duration_ms"] - avg_baseline
            if delay_s < 2.5:
                phases["OPENING (0-2.5s)"].append(delta)
            elif delay_s < 7.5:
                phases["OPEN (2.5-7.5s)"].append(delta)
            elif delay_s < 10.5:
                phases["CLOSING (7.5-10.5s)"].append(delta)
            else:
                phases["CLOSED (>10.5s)"].append(delta)
        except:
            pass

    for phase, deltas in phases.items():
        if deltas:
            avg = sum(deltas) // len(deltas)
            print(f"  {phase:<25} avg: {avg:+5}ms  (n={len(deltas)})")
        else:
            print(f"  {phase:<25} no data")

    print("\n\nInvestigation complete.")
    return 0


if __name__ == "__main__":
    exit(main())
