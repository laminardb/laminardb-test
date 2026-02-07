-- Phase 5: CDC test table + publication
-- Postgres must be started with wal_level=logical (see docker-compose.yml)

CREATE TABLE orders (
    id SERIAL PRIMARY KEY,
    customer_id INT NOT NULL,
    amount NUMERIC(10,2) NOT NULL,
    status VARCHAR(20) NOT NULL DEFAULT 'pending',
    ts BIGINT NOT NULL
);

-- Publication for CDC (laminardb subscribes to this)
CREATE PUBLICATION laminar_pub FOR TABLE orders;
