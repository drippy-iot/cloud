CREATE EXTENSION pgcrypto;

CREATE TABLE unit(
    mac MACADDR NOT NULL,
    shutdown BOOLEAN NOT NULL,
    PRIMARY KEY (mac)
);

CREATE TABLE status(
    mac MACADDR NOT NULL,
    creation TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    flow SMALLINT NOT NULL,
    PRIMARY KEY (mac, creation),
    CONSTRAINT "FK_status.mac"
        FOREIGN KEY (mac)
        REFERENCES unit (mac)
);

CREATE TABLE leak(
    mac MACADDR NOT NULL,
    creation TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (mac, creation),
    CONSTRAINT "FK_leak.mac"
        FOREIGN KEY (mac)
        REFERENCES unit (mac)
);

CREATE TABLE push(
    mac MACADDR NOT NULL,
    endpoint VARCHAR(1024) NOT NULL,
    auth BYTEA NOT NULL,
    p256dh BYTEA NOT NULL,
    PRIMARY KEY (mac, endpoint),
    CONSTRAINT "FK_push.mac"
        FOREIGN KEY (mac)
        REFERENCES unit (mac)
);

CREATE TABLE sms(
    mac MACADDR NOT NULL,
    phone VARCHAR(16) NOT NULL,
    PRIMARY KEY (mac, phone),
    CONSTRAINT "FK_sms.mac"
        FOREIGN KEY (mac)
        REFERENCES unit (mac)
);

CREATE TABLE session(
    id UUID NOT NULL DEFAULT gen_random_uuid(),
    mac MACADDR NOT NULL,
    PRIMARY KEY (id),
    CONSTRAINT "FK_session.mac"
        FOREIGN KEY (mac)
        REFERENCES unit (mac)
);
