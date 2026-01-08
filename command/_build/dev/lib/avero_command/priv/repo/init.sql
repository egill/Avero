-- Enable TimescaleDB extension
CREATE EXTENSION IF NOT EXISTS timescaledb;

-- ============================================
-- EVENTS TABLE (hypertable for time-series)
-- ============================================
CREATE TABLE IF NOT EXISTS events (
    id BIGSERIAL,
    time TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    site TEXT NOT NULL,
    event_type TEXT NOT NULL,

    -- Entity references
    person_id INTEGER,
    gate_id INTEGER,
    sensor_id TEXT,
    zone TEXT,

    -- Common fields (denormalized for query performance)
    authorized BOOLEAN,
    auth_method TEXT,
    duration_ms INTEGER,

    -- Full event data as JSON
    data JSONB NOT NULL DEFAULT '{}',

    PRIMARY KEY (id, time)
);

-- Convert to hypertable for time-series optimization
SELECT create_hypertable('events', 'time', if_not_exists => TRUE);

-- Indexes for common queries
CREATE INDEX IF NOT EXISTS idx_events_site_type ON events (site, event_type, time DESC);
CREATE INDEX IF NOT EXISTS idx_events_person ON events (person_id, time DESC) WHERE person_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_events_gate ON events (gate_id, time DESC) WHERE gate_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_events_sensor ON events (sensor_id, time DESC) WHERE sensor_id IS NOT NULL;

-- ============================================
-- INCIDENTS TABLE
-- ============================================
CREATE TABLE IF NOT EXISTS incidents (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    -- Classification
    type TEXT NOT NULL,
    severity TEXT NOT NULL,  -- high, medium, info
    category TEXT NOT NULL,  -- loss_prevention, equipment, safety, customer_exp, business_intel

    -- Location
    site TEXT NOT NULL,
    gate_id INTEGER,

    -- State
    status TEXT NOT NULL DEFAULT 'new',  -- new, acknowledged, in_progress, resolved, dismissed
    acknowledged_at TIMESTAMPTZ,
    acknowledged_by TEXT,
    resolved_at TIMESTAMPTZ,
    resolved_by TEXT,
    resolution TEXT,

    -- Context
    context JSONB NOT NULL DEFAULT '{}',
    related_person_id INTEGER,
    related_events JSONB DEFAULT '[]',

    -- Actions
    suggested_actions JSONB DEFAULT '[]',
    executed_actions JSONB DEFAULT '[]'
);

CREATE INDEX IF NOT EXISTS idx_incidents_status ON incidents (status, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_incidents_site ON incidents (site, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_incidents_type ON incidents (type, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_incidents_severity ON incidents (severity, created_at DESC);

-- ============================================
-- ACTIONS LOG (audit trail)
-- ============================================
CREATE TABLE IF NOT EXISTS action_logs (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    incident_id UUID REFERENCES incidents(id) ON DELETE CASCADE,
    executed_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    action_type TEXT NOT NULL,
    executed_by TEXT,
    result TEXT,
    details JSONB DEFAULT '{}'
);

CREATE INDEX IF NOT EXISTS idx_action_logs_incident ON action_logs (incident_id, executed_at DESC);

-- ============================================
-- SITE CONFIGURATION
-- ============================================
CREATE TABLE IF NOT EXISTS site_configs (
    site TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    timezone TEXT DEFAULT 'UTC',

    -- Operating hours (for idle detection, etc.)
    operating_hours JSONB DEFAULT '{"start": "08:00", "end": "22:00"}',

    -- Scenario thresholds (overrides defaults)
    scenario_config JSONB DEFAULT '{}',

    -- Notification routing
    notification_config JSONB DEFAULT '{}',

    created_at TIMESTAMPTZ DEFAULT NOW(),
    updated_at TIMESTAMPTZ DEFAULT NOW()
);

-- Insert default sites
INSERT INTO site_configs (site, name, timezone) VALUES
    ('netto', 'Netto Reykjavik', 'Atlantic/Reykjavik'),
    ('grandi', 'Grandi Test Site', 'Atlantic/Reykjavik'),
    ('docker-test', 'Docker Test Site', 'UTC')
ON CONFLICT (site) DO NOTHING;

-- ============================================
-- PERSON JOURNEYS (for analytics)
-- ============================================
CREATE TABLE IF NOT EXISTS person_journeys (
    id BIGSERIAL,
    time TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    site TEXT NOT NULL,
    person_id INTEGER NOT NULL,
    session_id TEXT,

    -- Journey summary
    started_at TIMESTAMPTZ NOT NULL,
    ended_at TIMESTAMPTZ,
    duration_ms INTEGER,

    -- Outcome
    outcome TEXT,  -- paid_exit, unpaid_exit, abandoned, returned, lost
    exit_type TEXT,  -- exit_confirmed, tracking_lost, returned_to_store
    authorized BOOLEAN,
    auth_method TEXT,
    receipt_id TEXT,

    -- Gate details
    gate_opened_by TEXT,  -- xovis or sensor
    tailgated BOOLEAN DEFAULT false,

    -- Payment details
    payment_zone TEXT,  -- Which POS zone they paid at
    total_pos_dwell_ms INTEGER,  -- Sum of all POS zone dwell times

    -- Dwell tracking
    dwell_threshold_met BOOLEAN DEFAULT false,
    dwell_zone TEXT,  -- Zone where dwell threshold was met

    -- Group tracking (from Xovis GROUP tracks)
    is_group BOOLEAN DEFAULT false,  -- True if this was a GROUP track
    member_count INTEGER DEFAULT 1,  -- Number of people in the group
    group_id INTEGER,  -- Group track ID if part of a group

    -- Path data
    zones_visited JSONB DEFAULT '[]',
    events JSONB DEFAULT '[]',

    PRIMARY KEY (id, time)
);

SELECT create_hypertable('person_journeys', 'time', if_not_exists => TRUE);

CREATE INDEX IF NOT EXISTS idx_journeys_site ON person_journeys (site, time DESC);
CREATE INDEX IF NOT EXISTS idx_journeys_outcome ON person_journeys (outcome, time DESC);
CREATE INDEX IF NOT EXISTS idx_journeys_exit_type ON person_journeys (exit_type, time DESC);
CREATE INDEX IF NOT EXISTS idx_journeys_is_group ON person_journeys (is_group, time DESC);

-- ============================================
-- CONTINUOUS AGGREGATES (for reporting)
-- ============================================

-- Hourly stats aggregate
CREATE MATERIALIZED VIEW IF NOT EXISTS hourly_stats
WITH (timescaledb.continuous) AS
SELECT
    time_bucket('1 hour', time) AS hour,
    site,
    COUNT(*) FILTER (WHERE event_type = 'exits' AND data->>'type' = 'exit.confirmed') AS exits,
    COUNT(*) FILTER (WHERE event_type = 'exits' AND data->>'type' = 'exit.confirmed' AND authorized = true) AS authorized_exits,
    COUNT(*) FILTER (WHERE event_type = 'exits' AND data->>'type' = 'exit.confirmed' AND authorized = false) AS unauthorized_exits,
    COUNT(*) FILTER (WHERE event_type = 'tracking' AND data->>'type' = 'tailgating.detected') AS tailgating,
    COUNT(*) FILTER (WHERE event_type = 'gates' AND data->>'type' = 'gate.fault') AS gate_faults,
    COUNT(*) AS total_events
FROM events
GROUP BY hour, site
WITH NO DATA;

-- Refresh policy: update every hour, keep last 90 days
SELECT add_continuous_aggregate_policy('hourly_stats',
    start_offset => INTERVAL '90 days',
    end_offset => INTERVAL '1 hour',
    schedule_interval => INTERVAL '1 hour',
    if_not_exists => TRUE);

-- ============================================
-- SCHEMA MIGRATIONS TABLE (for Ecto)
-- ============================================
CREATE TABLE IF NOT EXISTS schema_migrations (
    version BIGINT PRIMARY KEY,
    inserted_at TIMESTAMP WITHOUT TIME ZONE
);
