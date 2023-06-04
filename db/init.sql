CREATE EXTENSION pgcrypto;

-- A single ESP32 unit uniquely identified by its MAC address.
CREATE TABLE unit(
    mac MACADDR NOT NULL,
    -- Keeps track of pending requests (which have not been transmitted
    -- to the hardware yet). TRUE if open-request. FALSE if close-request.
    request BOOLEAN DEFAULT NULL,
    PRIMARY KEY (mac)
);

-- Ping events containing the flow rate (in ticks per second) and leak detection.
CREATE TABLE ping(
    mac MACADDR NOT NULL,
    creation TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    flow SMALLINT NOT NULL,
    leak BOOLEAN NOT NULL,
    PRIMARY KEY (mac, creation),
    CONSTRAINT "FK_ping.mac"
        FOREIGN KEY (mac)
        REFERENCES unit (mac)
);

-- Snapshots of the bypass requests.
CREATE TABLE control(
    mac MACADDR NOT NULL,
    creation TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    -- TRUE if open-request. FALSE if close-request.
    request BOOLEAN NOT NULL,
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
