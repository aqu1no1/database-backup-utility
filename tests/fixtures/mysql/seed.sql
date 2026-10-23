CREATE TABLE customers (
    id INT AUTO_INCREMENT PRIMARY KEY,
    name VARCHAR(100) NOT NULL,
    email VARCHAR(255) NOT NULL UNIQUE,
    created_at DATETIME NOT NULL DEFAULT '2026-01-01 00:00:00'
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE TABLE orders (
    id INT AUTO_INCREMENT PRIMARY KEY,
    customer_id INT NOT NULL,
    total DECIMAL(10, 2) NOT NULL,
    payload VARBINARY(16) NULL,
    CONSTRAINT fk_orders_customer FOREIGN KEY (customer_id) REFERENCES customers (id)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE TABLE audit_log (
    id INT AUTO_INCREMENT PRIMARY KEY,
    order_id INT NOT NULL,
    note VARCHAR(100) NOT NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE TRIGGER orders_audit AFTER INSERT ON orders
FOR EACH ROW INSERT INTO audit_log (order_id, note) VALUES (NEW.id, 'criado');

CREATE VIEW customer_totals AS
SELECT c.id, c.name, SUM(o.total) AS total
FROM customers c JOIN orders o ON o.customer_id = c.id
GROUP BY c.id, c.name;

INSERT INTO customers (name, email) VALUES
    ('Ana Souza', 'ana@example.com'),
    ('João Ação', 'joao@example.com'),
    ('Zoë 🚀', 'zoe@example.com');

INSERT INTO orders (customer_id, total, payload) VALUES
    (1, 10.50, X'00FF10'),
    (1, 99.99, NULL),
    (2, 0.01, X'DEADBEEF'),
    (3, 1234.56, X'');
