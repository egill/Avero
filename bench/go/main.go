// Gate latency benchmark - measures TCP command to door moving
//
// Based on production gateway-poc RS485 and CloudPlus implementations.
package main

import (
	"flag"
	"fmt"
	"net"
	"sort"
	"time"

	"go.bug.st/serial"
)

// CloudPlus protocol constants
const (
	STX         = 0x02
	ETX         = 0x03
	CmdOpenDoor = 0x2C
)

// RS485 protocol constants
const (
	RS485StartCmd  = 0x7E
	RS485StartResp = 0x7F
	RS485CmdQuery  = 0x10
	RS485FrameLen  = 18
)

// Door status codes
const (
	DoorClosedProperly     = 0x00
	DoorLeftOpenProperly   = 0x01
	DoorRightOpenProperly  = 0x02 // Resting position = closed
	DoorInMotion           = 0x03
	DoorFireSignalOpening  = 0x04
)

type DoorStatus int

const (
	StatusClosed DoorStatus = iota
	StatusOpen
	StatusMoving
	StatusUnknown
)

func doorStatusFromCode(code byte) DoorStatus {
	switch code {
	case DoorClosedProperly, DoorRightOpenProperly:
		return StatusClosed
	case DoorLeftOpenProperly, DoorFireSignalOpening:
		return StatusOpen
	case DoorInMotion:
		return StatusMoving
	default:
		return StatusUnknown
	}
}

func buildCloudPlusOpenFrame() []byte {
	frame := make([]byte, 9)
	frame[0] = STX
	frame[1] = 0x00 // rand
	frame[2] = CmdOpenDoor
	frame[3] = 0xff // address (broadcast)
	frame[4] = 0x01 // door 1
	frame[5] = 0x00 // len low
	frame[6] = 0x00 // len high
	// checksum: XOR of all bytes before checksum
	var checksum byte
	for i := 0; i < 7; i++ {
		checksum ^= frame[i]
	}
	frame[7] = checksum
	frame[8] = ETX
	return frame
}

func buildRS485Query() []byte {
	frame := make([]byte, 8)
	frame[0] = RS485StartCmd
	frame[1] = 0x00
	frame[2] = 0x01 // machine number
	frame[3] = RS485CmdQuery
	frame[4] = 0x00
	frame[5] = 0x00
	frame[6] = 0x00
	// checksum: sum all bytes, bitwise NOT
	var sum byte
	for i := 0; i < 7; i++ {
		sum += frame[i]
	}
	frame[7] = ^sum
	return frame
}

func pollDoorStatus(rs485 serial.Port, queryCmd []byte) (DoorStatus, bool) {
	rs485.Write(queryCmd)

	// Read with accumulation until we have enough data
	buf := make([]byte, 64)
	totalRead := 0
	deadline := time.Now().Add(300 * time.Millisecond)

	for totalRead < len(buf) && time.Now().Before(deadline) {
		n, err := rs485.Read(buf[totalRead:])
		if err != nil {
			break
		}
		if n > 0 {
			totalRead += n
			if totalRead >= RS485FrameLen {
				break
			}
		}
	}

	if totalRead < RS485FrameLen {
		return StatusUnknown, false
	}

	// Find 0x7F start byte
	for i := 0; i <= totalRead-RS485FrameLen; i++ {
		if buf[i] == RS485StartResp {
			frame := buf[i : i+RS485FrameLen]
			// Validate checksum: sum + 1 should be 0
			var sum byte
			for _, b := range frame {
				sum += b
			}
			if sum+1 == 0 {
				statusByte := frame[4] // Door status at byte 4
				return doorStatusFromCode(statusByte), true
			}
		}
	}
	return StatusUnknown, false
}

func waitForClosed(rs485 serial.Port, queryCmd []byte, timeout time.Duration) bool {
	deadline := time.Now().Add(timeout)
	for time.Now().Before(deadline) {
		if status, ok := pollDoorStatus(rs485, queryCmd); ok && status == StatusClosed {
			return true
		}
		time.Sleep(50 * time.Millisecond)
	}
	return false
}

func main() {
	gateAddr := flag.String("gate-addr", "192.168.0.245:8000", "CloudPlus TCP address")
	rs485Device := flag.String("rs485-device", "/dev/ttyUSB0", "RS485 serial device")
	rs485Baud := flag.Int("rs485-baud", 19200, "RS485 baud rate")
	trials := flag.Int("trials", 20, "Number of trials")
	delay := flag.Int("delay", 5, "Delay between trials (seconds)")
	flag.Parse()

	fmt.Println("Gate Latency Benchmark (Go)")
	fmt.Println("===========================")
	fmt.Printf("Gate TCP: %s\n", *gateAddr)
	fmt.Printf("RS485: %s @ %d baud\n", *rs485Device, *rs485Baud)
	fmt.Printf("Trials: %d\n", *trials)
	fmt.Println()

	// Open RS485 port
	mode := &serial.Mode{
		BaudRate: *rs485Baud,
		DataBits: 8,
		Parity:   serial.NoParity,
		StopBits: serial.OneStopBit,
	}
	rs485, err := serial.Open(*rs485Device, mode)
	if err != nil {
		fmt.Printf("Failed to open RS485: %v\n", err)
		return
	}
	defer rs485.Close()
	rs485.SetReadTimeout(300 * time.Millisecond)
	fmt.Println("RS485 port opened")

	// Test RS485
	queryCmd := buildRS485Query()
	fmt.Print("Testing RS485... ")
	var statusOK DoorStatus
	found := false
	for attempt := 0; attempt < 5; attempt++ {
		if status, ok := pollDoorStatus(rs485, queryCmd); ok {
			statusOK = status
			found = true
			break
		}
		time.Sleep(200 * time.Millisecond)
	}
	if !found {
		fmt.Println("FAILED - no response")
		return
	}
	statusNames := map[DoorStatus]string{
		StatusClosed: "CLOSED", StatusOpen: "OPEN", StatusMoving: "MOVING", StatusUnknown: "UNKNOWN",
	}
	fmt.Printf("OK (door status: %s)\n", statusNames[statusOK])

	openFrame := buildCloudPlusOpenFrame()
	var results []int64

	// Wait for door to be closed
	fmt.Println("Waiting for door to be closed...")
	if !waitForClosed(rs485, queryCmd, 30*time.Second) {
		fmt.Println("Door not closed - timeout")
		return
	}
	fmt.Println("Door is closed. Starting benchmark.")
	fmt.Println()

	for trial := 1; trial <= *trials; trial++ {
		// Ensure door is closed
		if !waitForClosed(rs485, queryCmd, 30*time.Second) {
			fmt.Printf("Trial %2d: SKIPPED - door not closed\n", trial)
			continue
		}

		// Small delay for stable state
		time.Sleep(200 * time.Millisecond)

		// Connect fresh for each trial (gate closes idle connections)
		gate, err := net.DialTimeout("tcp", *gateAddr, 5*time.Second)
		if err != nil {
			fmt.Printf("Trial %2d: TCP connect failed: %v\n", trial, err)
			continue
		}
		if tcpConn, ok := gate.(*net.TCPConn); ok {
			tcpConn.SetNoDelay(true)
		}

		// Send open command and start timer
		cmdSent := time.Now()
		gate.Write(openFrame)
		gate.Close() // Close connection after sending

		fmt.Printf("Trial %2d: Command sent... ", trial)

		// Poll RS485 until door is moving (250ms minimum between polls)
		movingDetected := false
		deadline := time.Now().Add(10 * time.Second)
		lastPoll := time.Now().Add(-250 * time.Millisecond) // Allow immediate first poll

		for time.Now().Before(deadline) {
			// Ensure minimum 250ms since last poll
			sinceLast := time.Since(lastPoll)
			if sinceLast < 250*time.Millisecond {
				time.Sleep(250*time.Millisecond - sinceLast)
			}
			lastPoll = time.Now()

			if status, ok := pollDoorStatus(rs485, queryCmd); ok && status == StatusMoving {
				elapsed := time.Since(cmdSent).Milliseconds()
				results = append(results, elapsed)
				fmt.Printf("%d ms\n", elapsed)
				movingDetected = true
				break
			}
		}

		if !movingDetected {
			fmt.Println("TIMEOUT")
		}

		// Wait between trials
		if trial < *trials {
			time.Sleep(time.Duration(*delay) * time.Second)
		}
	}

	// Calculate statistics
	fmt.Println()
	fmt.Println("===========================")
	fmt.Println("Results:")
	if len(results) > 0 {
		var sum int64
		for _, v := range results {
			sum += v
		}
		avg := sum / int64(len(results))

		sorted := make([]int64, len(results))
		copy(sorted, results)
		sort.Slice(sorted, func(i, j int) bool { return sorted[i] < sorted[j] })

		min := sorted[0]
		max := sorted[len(sorted)-1]
		p50 := sorted[len(sorted)/2]
		p95Idx := int(float64(len(sorted)) * 0.95)
		if p95Idx >= len(sorted) {
			p95Idx = len(sorted) - 1
		}
		p95 := sorted[p95Idx]

		fmt.Printf("  Successful: %d/%d\n", len(results), *trials)
		fmt.Printf("  Min: %d ms\n", min)
		fmt.Printf("  Max: %d ms\n", max)
		fmt.Printf("  Avg: %d ms\n", avg)
		fmt.Printf("  P50: %d ms\n", p50)
		fmt.Printf("  P95: %d ms\n", p95)
	} else {
		fmt.Println("  No successful trials!")
	}
}
