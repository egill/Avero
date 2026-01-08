#!/usr/bin/env python3
"""
Chat with Journey Data - Natural language interface to journey analysis.

Uses Claude to interpret queries and analyze customer journey data.

Usage:
    python chat-journeys.py                    # Interactive REPL
    python chat-journeys.py "your question"   # Single query mode

Environment:
    ANTHROPIC_API_KEY - Your Anthropic API key
    AVERO_API_URL     - Command API URL (default: https://command.e18n.net)
"""

import os
import sys
import json
import time
import uuid
import subprocess
from datetime import datetime, timedelta
from pathlib import Path
from typing import Optional

import anthropic
import requests
from dotenv import load_dotenv
from rich.console import Console
from rich.markdown import Markdown
from rich.panel import Panel
from rich.table import Table
from rich.prompt import Prompt
from rich.syntax import Syntax

# Load environment
load_dotenv()

console = Console()

# Configuration
API_URL = os.getenv("AVERO_API_URL", "https://command.e18n.net")
ANTHROPIC_API_KEY = os.getenv("ANTHROPIC_API_KEY")
RPI_HOST = os.getenv("RPI_HOST", "avero@100.80.187.3")
ISSUES_DIR = Path(__file__).parent / "issues"

# Rate limiting
LAST_REQUEST_TIME = 0
MIN_REQUEST_INTERVAL = 1.0  # seconds


def rate_limit():
    """Ensure minimum interval between API requests."""
    global LAST_REQUEST_TIME
    now = time.time()
    elapsed = now - LAST_REQUEST_TIME
    if elapsed < MIN_REQUEST_INTERVAL:
        time.sleep(MIN_REQUEST_INTERVAL - elapsed)
    LAST_REQUEST_TIME = time.time()


# ============================================================================
# Tool implementations
# ============================================================================

def query_journeys(
    site: Optional[str] = None,
    exit_type: Optional[str] = None,
    since: Optional[str] = None,
    until: Optional[str] = None,
    person_id: Optional[str] = None,
    pos_filter: Optional[str] = None,
    min_dwell: Optional[int] = None,
    has_acc: Optional[bool] = None,
    limit: int = 50,
) -> dict:
    """Query journeys from the API with filters."""
    rate_limit()

    params = {"limit": limit}
    if site:
        params["site"] = site
    if exit_type:
        params["exit_type"] = exit_type
    if since:
        params["since"] = since
    if until:
        params["until"] = until
    if person_id:
        params["person_id"] = str(person_id)
    if pos_filter:
        params["pos_filter"] = pos_filter
    if min_dwell:
        params["min_dwell"] = min_dwell
    if has_acc is not None:
        params["has_acc"] = "true" if has_acc else "false"

    try:
        response = requests.get(f"{API_URL}/api/journeys", params=params, timeout=30)
        response.raise_for_status()
        return response.json()
    except requests.RequestException as e:
        return {"error": str(e), "journeys": []}


def get_journey_detail(journey_id: str) -> dict:
    """Get a single journey with full event details."""
    rate_limit()

    # Try by ID first, then by session_id
    try:
        # First try numeric ID
        response = requests.get(f"{API_URL}/api/journeys/{journey_id}", timeout=30)
        if response.status_code == 200:
            return response.json()

        # Try by session_id (UUID)
        response = requests.get(
            f"{API_URL}/api/journeys/by-session/{journey_id}", timeout=30
        )
        if response.status_code == 200:
            return response.json()

        return {"error": f"Journey not found: {journey_id}"}
    except requests.RequestException as e:
        return {"error": str(e)}


def get_journey_stats(
    site: Optional[str] = None,
    since: Optional[str] = None,
    until: Optional[str] = None,
) -> dict:
    """Get aggregate statistics for journeys."""
    rate_limit()

    params = {}
    if site:
        params["site"] = site
    if since:
        params["since"] = since
    if until:
        params["until"] = until

    try:
        response = requests.get(f"{API_URL}/api/journeys/stats", params=params, timeout=30)
        response.raise_for_status()
        return response.json()
    except requests.RequestException as e:
        return {"error": str(e)}


def query_logs(pattern: str, since_hours: int = 1) -> dict:
    """Search RPi logs for a pattern."""
    try:
        since_time = datetime.now() - timedelta(hours=since_hours)
        since_str = since_time.strftime("%Y-%m-%d %H:%M:%S")

        cmd = [
            "ssh", RPI_HOST,
            f"journalctl -u gateway-poc --since '{since_str}' --no-pager 2>/dev/null | grep -i '{pattern}' | tail -50"
        ]
        result = subprocess.run(cmd, capture_output=True, text=True, timeout=30)

        lines = result.stdout.strip().split("\n") if result.stdout.strip() else []
        return {
            "pattern": pattern,
            "since_hours": since_hours,
            "matches": len(lines),
            "lines": lines[:50],  # Limit to 50 lines
        }
    except subprocess.TimeoutExpired:
        return {"error": "SSH timeout", "pattern": pattern}
    except Exception as e:
        return {"error": str(e), "pattern": pattern}


def create_issue(
    issue_type: str,
    severity: str,
    journey_id: str,
    description: str,
    context: dict,
) -> dict:
    """Create an issue in the issues-to-review.jsonl file."""
    ISSUES_DIR.mkdir(exist_ok=True)
    issues_file = ISSUES_DIR / "issues-to-review.jsonl"
    counters_file = ISSUES_DIR / "issue-counters.json"

    issue_id = str(uuid.uuid4())
    now = datetime.utcnow().isoformat() + "Z"

    issue = {
        "id": issue_id,
        "type": issue_type,
        "severity": severity,
        "journey_id": journey_id,
        "description": description,
        "context": context,
        "status": "new",
        "created_at": now,
        "reviewed_at": None,
        "notes": None,
    }

    # Append to issues file
    with open(issues_file, "a") as f:
        f.write(json.dumps(issue) + "\n")

    # Update counter
    counters = {}
    if counters_file.exists():
        with open(counters_file) as f:
            counters = json.load(f)

    counters[issue_type] = counters.get(issue_type, 0) + 1

    with open(counters_file, "w") as f:
        json.dump(counters, f, indent=2)

    return {
        "created": True,
        "issue_id": issue_id,
        "type": issue_type,
        "severity": severity,
    }


def get_issue_stats() -> dict:
    """Get current issue statistics."""
    counters_file = ISSUES_DIR / "issue-counters.json"
    issues_file = ISSUES_DIR / "issues-to-review.jsonl"

    counters = {}
    if counters_file.exists():
        with open(counters_file) as f:
            counters = json.load(f)

    # Count by status
    status_counts = {"new": 0, "reviewed": 0, "dismissed": 0, "promoted": 0}
    if issues_file.exists():
        with open(issues_file) as f:
            for line in f:
                if line.strip():
                    issue = json.loads(line)
                    status = issue.get("status", "new")
                    status_counts[status] = status_counts.get(status, 0) + 1

    return {
        "lifetime_counters": counters,
        "current_status": status_counts,
    }


# ============================================================================
# Claude tool definitions
# ============================================================================

TOOLS = [
    {
        "name": "query_journeys",
        "description": """Query customer journeys from the database with filters.

Use this to find journeys matching specific criteria:
- site: 'netto' or 'grandi'
- exit_type: 'exit_confirmed' (normal exit), 'tracking_lost' (lost tracking), 'returned_to_store' (returned)
  - Aliases: 'exits', 'lost', 'returns'
- since/until: ISO8601 timestamps or dates (YYYY-MM-DD)
- person_id: Track ID to search for
- pos_filter: 'with_pos' (spent >=7s at POS), 'without_pos', 'unpaid_with_pos' (unpaid but had POS time)
- has_acc: true/false - whether ACC payment was matched

Returns journeys with: outcome, exit_type, authorized, acc_matched, total_pos_dwell_ms, zones_visited, etc.""",
        "input_schema": {
            "type": "object",
            "properties": {
                "site": {"type": "string", "description": "Site filter (netto, grandi)"},
                "exit_type": {"type": "string", "description": "Exit type filter"},
                "since": {"type": "string", "description": "Start of time range (ISO8601 or YYYY-MM-DD)"},
                "until": {"type": "string", "description": "End of time range"},
                "person_id": {"type": "string", "description": "Person/track ID to find"},
                "pos_filter": {"type": "string", "description": "POS filter: with_pos, without_pos, unpaid_with_pos"},
                "has_acc": {"type": "boolean", "description": "Filter by ACC match status"},
                "limit": {"type": "integer", "description": "Max results (default 50)", "default": 50},
            },
        },
    },
    {
        "name": "get_journey_detail",
        "description": """Get full details of a specific journey including all events.

Use this to investigate a specific journey in depth. Pass either the numeric ID or the session_id (UUID).

Returns the complete journey with all events (zone entries/exits, gate commands, ACC matches, etc.).""",
        "input_schema": {
            "type": "object",
            "properties": {
                "journey_id": {"type": "string", "description": "Journey ID (numeric) or session_id (UUID)"},
            },
            "required": ["journey_id"],
        },
    },
    {
        "name": "get_journey_stats",
        "description": """Get aggregate statistics for journeys.

Returns counts by exit_type, outcome, authorization status, ACC match rate, average dwell times, etc.
Useful for understanding overall patterns before drilling into specifics.""",
        "input_schema": {
            "type": "object",
            "properties": {
                "site": {"type": "string", "description": "Site filter"},
                "since": {"type": "string", "description": "Start of time range"},
                "until": {"type": "string", "description": "End of time range"},
            },
        },
    },
    {
        "name": "query_logs",
        "description": """Search gateway-poc logs on the Raspberry Pi for a pattern.

Use this to correlate journey issues with log events. Useful patterns:
- 'acc_unmatched' - ACC events that didn't match any customer
- 'stitch_expired_lost' - Tracks that were lost (stitch failed)
- 'gate_entry_not_authorized' - Gate blocked events
- A specific track_id to see all events for that track""",
        "input_schema": {
            "type": "object",
            "properties": {
                "pattern": {"type": "string", "description": "Grep pattern to search for"},
                "since_hours": {"type": "integer", "description": "How many hours back to search (default 1)", "default": 1},
            },
            "required": ["pattern"],
        },
    },
    {
        "name": "create_issue",
        "description": """Create an issue for tracking and investigation.

Use this when you've identified a problem worth investigating further.

Issue types:
- exit_no_acc: Customer exited with POS dwell but no ACC match
- tracking_lost_with_pos: Tracking lost after significant POS time
- acc_match_tracking_lost: ACC matched but tracking lost before exit
- gate_cmd_no_exit: Gate command sent but no exit confirmation
- high_acc_unmatched_rate: Systemic ACC issues
- high_stitch_expiry_rate: Systemic tracking issues

Severity: high, medium, low""",
        "input_schema": {
            "type": "object",
            "properties": {
                "issue_type": {"type": "string", "description": "Type of issue"},
                "severity": {"type": "string", "enum": ["high", "medium", "low"]},
                "journey_id": {"type": "string", "description": "Related journey ID"},
                "description": {"type": "string", "description": "Human-readable description"},
                "context": {"type": "object", "description": "Additional context data"},
            },
            "required": ["issue_type", "severity", "journey_id", "description", "context"],
        },
    },
    {
        "name": "get_issue_stats",
        "description": """Get current issue tracking statistics.

Returns lifetime counters by issue type and current status breakdown (new, reviewed, dismissed, promoted).""",
        "input_schema": {
            "type": "object",
            "properties": {},
        },
    },
]


def execute_tool(name: str, args: dict) -> str:
    """Execute a tool and return JSON result."""
    if name == "query_journeys":
        result = query_journeys(**args)
    elif name == "get_journey_detail":
        result = get_journey_detail(**args)
    elif name == "get_journey_stats":
        result = get_journey_stats(**args)
    elif name == "query_logs":
        result = query_logs(**args)
    elif name == "create_issue":
        result = create_issue(**args)
    elif name == "get_issue_stats":
        result = get_issue_stats()
    else:
        result = {"error": f"Unknown tool: {name}"}

    return json.dumps(result, indent=2, default=str)


# ============================================================================
# Claude conversation
# ============================================================================

SYSTEM_PROMPT = """You are a journey data analyst for a retail gate control system. You help investigate customer journey anomalies and tracking issues.

The system tracks customers from entry to exit:
1. Customer enters store (tracked by Xovis sensors)
2. Customer visits POS zones (tracked dwell time)
3. If dwell >= 7 seconds at POS, customer becomes "authorized"
4. ACC (payment terminal) events can also authorize customers
5. When authorized customer enters gate zone, gate opens
6. Journey ends when customer crosses exit line or tracking is lost

Key fields to understand:
- exit_type: "exit_confirmed" (normal), "tracking_lost" (lost before exit), "returned_to_store"
- authorized: Customer was authorized to exit
- acc_matched: Payment terminal event was correlated
- total_pos_dwell_ms: Time spent at POS zones
- outcome: "paid_exit", "unpaid_exit", "lost_authorized", "lost_unauthorized", "returned"

Common issues to investigate:
- exit_no_acc: Exited with POS dwell but no payment match (ACC timing? Cash payment?)
- tracking_lost_with_pos: Lost tracking after significant POS time (sensor gap? stitch failure?)
- acc_match_tracking_lost: Payment matched but tracking lost (gate area issue?)

When analyzing issues:
1. First query relevant journeys to understand the scope
2. Get detailed events for specific journeys of interest
3. Correlate with logs if needed
4. Create issues for findings worth investigating

Be concise in your responses. Focus on actionable insights."""


def chat(client: anthropic.Anthropic, user_message: str, messages: list) -> str:
    """Send a message and handle tool use."""
    messages.append({"role": "user", "content": user_message})

    while True:
        response = client.messages.create(
            model="claude-sonnet-4-20250514",
            max_tokens=4096,
            system=SYSTEM_PROMPT,
            tools=TOOLS,
            messages=messages,
        )

        # Collect all content blocks
        assistant_content = []
        tool_calls = []

        for block in response.content:
            if block.type == "text":
                assistant_content.append(block)
            elif block.type == "tool_use":
                assistant_content.append(block)
                tool_calls.append(block)

        # Add assistant message
        messages.append({"role": "assistant", "content": assistant_content})

        # If no tool calls, we're done
        if not tool_calls:
            # Return text response
            text_parts = [b.text for b in response.content if b.type == "text"]
            return "\n".join(text_parts)

        # Execute tools and add results
        tool_results = []
        for tool_call in tool_calls:
            console.print(f"[dim]Calling {tool_call.name}...[/dim]")
            result = execute_tool(tool_call.name, tool_call.input)
            tool_results.append({
                "type": "tool_result",
                "tool_use_id": tool_call.id,
                "content": result,
            })

        messages.append({"role": "user", "content": tool_results})

        # If stop reason is end_turn, return what we have
        if response.stop_reason == "end_turn":
            text_parts = [b.text for b in response.content if b.type == "text"]
            return "\n".join(text_parts) if text_parts else "[Tool execution complete]"


# ============================================================================
# Main CLI
# ============================================================================

def print_welcome():
    """Print welcome message."""
    console.print(Panel.fit(
        "[bold cyan]Chat with Journey Data[/bold cyan]\n\n"
        "Ask questions about customer journeys in natural language.\n\n"
        "[dim]Examples:[/dim]\n"
        "  - Show me journeys where tracking was lost after POS\n"
        "  - Why did journey abc123 not get authorized?\n"
        "  - What's the ACC match rate for today?\n"
        "  - Create an issue for this journey\n\n"
        "[dim]Commands:[/dim]\n"
        "  - help    Show this help\n"
        "  - stats   Show issue statistics\n"
        "  - clear   Clear conversation history\n"
        "  - quit    Exit",
        title="Journey Analyzer",
    ))


def main():
    """Main entry point."""
    if not ANTHROPIC_API_KEY:
        console.print("[red]Error: ANTHROPIC_API_KEY not set[/red]")
        console.print("Set it in your environment or create a .env file in the scripts directory")
        sys.exit(1)

    client = anthropic.Anthropic(api_key=ANTHROPIC_API_KEY)
    messages = []

    # Single query mode
    if len(sys.argv) > 1:
        query = " ".join(sys.argv[1:])
        try:
            response = chat(client, query, messages)
            console.print(Markdown(response))
        except Exception as e:
            console.print(f"[red]Error: {e}[/red]")
        return

    # Interactive mode
    print_welcome()
    console.print()

    while True:
        try:
            user_input = Prompt.ask("[bold green]>[/bold green]")
        except (KeyboardInterrupt, EOFError):
            console.print("\n[dim]Goodbye![/dim]")
            break

        user_input = user_input.strip()
        if not user_input:
            continue

        # Handle commands
        if user_input.lower() in ("quit", "exit", "q"):
            console.print("[dim]Goodbye![/dim]")
            break

        if user_input.lower() == "help":
            print_welcome()
            continue

        if user_input.lower() == "clear":
            messages = []
            console.print("[dim]Conversation cleared[/dim]")
            continue

        if user_input.lower() == "stats":
            stats = get_issue_stats()
            console.print(Panel(
                Syntax(json.dumps(stats, indent=2), "json", theme="monokai"),
                title="Issue Statistics",
            ))
            continue

        # Send to Claude
        try:
            with console.status("[bold cyan]Thinking...[/bold cyan]"):
                response = chat(client, user_input, messages)
            console.print()
            console.print(Markdown(response))
            console.print()
        except anthropic.APIError as e:
            console.print(f"[red]API Error: {e}[/red]")
        except Exception as e:
            console.print(f"[red]Error: {e}[/red]")


if __name__ == "__main__":
    main()
