CREATE EXTENSION pgcrypto;

-- A single ESP32 unit uniquely identified by its MAC address.
CREATE TABLE unit(
    mac MACADDR NOT NULL,
    shutdown BOOLEAN NOT NULL DEFAULT FALSE,
    PRIMARY KEY (mac)
);

-- Snapshots of the flow rate (ticks per second).
CREATE TABLE flow(
    mac MACADDR NOT NULL,
    creation TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    flow SMALLINT NOT NULL,
    PRIMARY KEY (mac, creation),
    CONSTRAINT "FK_flow.mac"
        FOREIGN KEY (mac)
        REFERENCES unit (mac)
);

-- Snapshots of the leak events.
CREATE TABLE leak(
    mac MACADDR NOT NULL,
    creation TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (mac, creation),
    CONSTRAINT "FK_leak.mac"
        FOREIGN KEY (mac)
        REFERENCES unit (mac)
);

-- Snapshots of the control requests (i.e., shutdown and reset).
CREATE TABLE control(
    mac MACADDR NOT NULL,
    creation TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    -- TRUE if shutdown request and FALSE if reset request.
    shutdown BOOLEAN NOT NULL,
    PRIMARY KEY (mac, creation),
    CONSTRAINT "FK_control.mac"
        FOREIGN KEY (mac)
        REFERENCES unit (mac)
);

-- Mapping of session IDs to the associated MAC addresses.
CREATE TABLE session(
    id UUID NOT NULL DEFAULT gen_random_uuid(),
    mac MACADDR NOT NULL,
    PRIMARY KEY (id),
    CONSTRAINT "FK_session.mac"
        FOREIGN KEY (mac)
        REFERENCES unit (mac)
);
