-- The sample DuckDB database of the duckdb lane: the unit tests' catalog and value fixture, and
-- the content of dev/fixtures/sample.duckdb. Every object kind Tabler shows (schemas, tables,
-- views, keys, checks, indexes, sequences, macros, an ENUM type, comments) and a row of every
-- value type.

CREATE SCHEMA crm;
CREATE TYPE mood AS ENUM ('sad', 'ok', 'happy');
CREATE SEQUENCE order_seq START 1000;

CREATE TABLE crm.customers (
    id INTEGER PRIMARY KEY,
    name VARCHAR NOT NULL,
    email VARCHAR UNIQUE,
    feeling mood,
    tags VARCHAR[],
    created_at TIMESTAMP DEFAULT current_timestamp
);
COMMENT ON TABLE crm.customers IS 'People who buy';
COMMENT ON COLUMN crm.customers.email IS 'Login';

CREATE TABLE customers (
    id INTEGER PRIMARY KEY,
    name VARCHAR NOT NULL
);

CREATE TABLE orders (
    id BIGINT PRIMARY KEY DEFAULT nextval('order_seq'),
    customer_id INTEGER NOT NULL REFERENCES customers (id),
    total DECIMAL(12, 2) NOT NULL CHECK (total >= 0),
    status VARCHAR DEFAULT 'new',
    placed_at TIMESTAMPTZ,
    meta JSON
);
CREATE INDEX orders_placed ON orders (placed_at);
CREATE UNIQUE INDEX orders_customer_status ON orders (customer_id, lower(status));

CREATE TABLE order_items (
    order_id BIGINT REFERENCES orders (id),
    line INTEGER,
    sku VARCHAR NOT NULL,
    qty INTEGER DEFAULT 1,
    PRIMARY KEY (order_id, line)
);

-- No key: edits match every column.
CREATE TABLE events (
    happened_at TIMESTAMP,
    kind VARCHAR,
    payload STRUCT(source VARCHAR, attempts INTEGER)
);

CREATE VIEW recent_orders AS
    SELECT * FROM orders WHERE placed_at > TIMESTAMPTZ '2024-01-01 00:00:00+00';
COMMENT ON VIEW recent_orders IS 'Orders of 2024 on';

CREATE MACRO add_tax(x, rate := 0.1) AS x * (1 + rate);
CREATE MACRO top_orders(n) AS TABLE SELECT * FROM orders ORDER BY total DESC LIMIT n;

INSERT INTO crm.customers (id, name, email, feeling, tags) VALUES
    (1, 'Ada', 'ada@example.com', 'happy', ['vip', 'early']),
    (2, 'Linus', NULL, 'ok', []);
INSERT INTO customers VALUES (1, 'Ada'), (2, 'Linus'), (3, 'Grace');
INSERT INTO orders (customer_id, total, status, placed_at, meta) VALUES
    (1, 19.99, 'paid', TIMESTAMPTZ '2024-03-01 10:00:00+00', '{"coupon": "SPRING"}'),
    (1, 5.00, 'new', TIMESTAMPTZ '2023-12-24 18:30:00+00', NULL),
    (2, 120.50, 'shipped', TIMESTAMPTZ '2024-06-15 08:15:30+00', '{"gift": true}');
INSERT INTO order_items VALUES (1000, 1, 'A-1', 2), (1000, 2, 'B-7', 1), (1002, 1, 'C-3', 5);
INSERT INTO events VALUES
    (TIMESTAMP '2024-01-01 00:00:00', 'login', {'source': 'web', 'attempts': 1}),
    (TIMESTAMP '2024-01-01 00:00:00', 'login', {'source': 'web', 'attempts': 1});

CREATE TABLE all_types (
    id INTEGER PRIMARY KEY,
    b BOOLEAN,
    ti TINYINT,
    si SMALLINT,
    i INTEGER,
    bi BIGINT,
    hi HUGEINT,
    uti UTINYINT,
    usi USMALLINT,
    ui UINTEGER,
    ubi UBIGINT,
    uhi UHUGEINT,
    f FLOAT,
    d DOUBLE,
    num DECIMAL(18, 3),
    v VARCHAR,
    bl BLOB,
    bt BIT,
    dt DATE,
    tm TIME,
    ttz TIMETZ,
    ts TIMESTAMP,
    ts_s TIMESTAMP_S,
    ts_ms TIMESTAMP_MS,
    ts_ns TIMESTAMP_NS,
    tstz TIMESTAMPTZ,
    iv INTERVAL,
    u UUID,
    j JSON,
    e mood,
    l INTEGER[],
    a VARCHAR[2],
    s STRUCT(a INTEGER, b VARCHAR),
    m MAP(VARCHAR, INTEGER),
    un UNION(num INTEGER, str VARCHAR),
    bn BIGNUM
);
INSERT INTO all_types VALUES (
    1, true, -8, -16, -32, 9007199254740993, -170141183460469231731687303715884105728,
    255, 65535, 4294967295, 18446744073709551615, 340282366920938463463374607431768211455,
    0.5, 0.1, 12345.678, 'héllo', '\xAA\x00ab'::BLOB, '10101'::BIT,
    DATE '2024-01-31', TIME '12:00:00.5', TIMETZ '12:00:00+02', TIMESTAMP '2024-01-31 12:00:00.5',
    TIMESTAMP_S '2024-01-31 12:00:00', TIMESTAMP_MS '2024-01-31 12:00:00.123',
    TIMESTAMP_NS '2024-01-31 12:00:00.123456789', TIMESTAMPTZ '2024-01-31 12:00:00+02',
    INTERVAL '1 year 2 months 3 days 04:05:06.789', '6bea4f37-ddba-4e31-95b0-15bb71efdb48',
    '{"a": [1, 2]}', 'happy', [1, 2, NULL], ['x', 'y'], {'a': 1, 'b': 'it''s'},
    MAP {'k': 1, 'j': 2}, union_value(str := 'u'), (-42)::BIGNUM
);
INSERT INTO all_types (id) VALUES (2);
