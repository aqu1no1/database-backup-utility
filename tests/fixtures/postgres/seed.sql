CREATE TABLE customers (
    id SERIAL PRIMARY KEY,
    name TEXT NOT NULL,
    created_at DATE NOT NULL
);

CREATE TABLE orders (
    id SERIAL PRIMARY KEY,
    customer_id INTEGER NOT NULL REFERENCES customers (id),
    total_cents INTEGER NOT NULL CHECK (total_cents >= 0)
);

INSERT INTO customers (name, created_at) VALUES
    ('Cliente Alfa', '2026-01-10'),
    ('Cliente Beta', '2026-02-20'),
    ('Cliente Gama', '2026-03-30');

INSERT INTO orders (customer_id, total_cents) VALUES
    (1, 1500),
    (1, 4990),
    (2, 12000);
