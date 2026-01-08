# Grafana Dashboard Tasks

## Missing Metrics/Panels for Dashboard

### ACC (Payment Terminal) Metrics
- [ ] **ACC Events Today** - Panel showing `gateway_acc_events_total` (exists in Prometheus, not in Grafana)
- [ ] **ACC Matched vs Unmatched** - Split panel showing:
  - `gateway_acc_matched_total`
  - `gateway_acc_events_total - gateway_acc_matched_total` (unmatched)
- [ ] **ACC Late Events** - `gateway_acc_late_total` (ACC arrived after person in gate zone)
- [ ] **ACC No Journey** - `gateway_acc_no_journey_total` (matched but no journey found)

### Exit Metrics Breakdown
- [ ] **Exits by Authorization** - Stat panel splitting exits by `authorized` label:
  - Paid exits: `gateway_exits_total{authorized="true"}`
  - Unpaid exits: `gateway_exits_total{authorized="false"}`
- [ ] **ACC-Linked Exits** - Exits where ACC was matched vs not (need to add label to metric)

### Journey Metrics
- [ ] **Journey Outcomes Today** - Breakdown by outcome (paid_exit, unpaid_exit, abandoned)
- [ ] **Average Journey Duration** - Time from entry to exit
- [ ] **Average POS Dwell Time** - Time spent at POS zones

### Real-time Metrics
- [ ] **Active Tracks by Zone** - Current persons in each zone type (entry, pos, gate)
- [ ] **Gate Zone Queue** - Persons waiting at gate

## Existing Panels (available for embedding)

| Panel ID | Title | Type | Use For |
|----------|-------|------|---------|
| 2 | Concurrent Persons | stat | Active tracks |
| 4 | Exits (1h) | stat | Total exits |
| 5 | Gate Opens (1h) | stat | Gate openings |
| 6 | Payments (1h) | stat | Payment events |
| 25 | POS Visits Unpaid (1h) | stat | Unpaid POS visits |
| 26 | Exits Lost (1h) | stat | Lost tracking |
| 8 | People Tracking | timeseries | Track history |
| 11 | Exits by Type | timeseries | Exit breakdown |
| 15 | Gate State History | state-timeline | Gate state |

## Notes

- ACC metrics exist in Rust gateway (`src/infra/metrics.rs`) but not all are in Grafana dashboards
- `gateway_exits_total` has `authorized` label but no dedicated panel for the breakdown
- Consider adding "today" variants (midnight reset) for daily operations view
