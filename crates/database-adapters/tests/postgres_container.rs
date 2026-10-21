#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use support::{PostgresTestDb, TestResult};

#[tokio::test]
async fn postgres_container_is_healthy() -> TestResult {
    let db = PostgresTestDb::start().await?;

    let row = db
        .client()
        .query_one("SELECT 1::INTEGER, pg_is_in_recovery()", &[])
        .await?;
    let value: i32 = row.get(0);
    let in_recovery: bool = row.get(1);

    assert_eq!(value, 1);
    assert!(!in_recovery, "o banco de teste deve aceitar escrita");
    Ok(())
}

#[tokio::test]
async fn seed_fixture_is_applied() -> TestResult {
    let db = PostgresTestDb::start_seeded().await?;

    let customers: i64 = db
        .client()
        .query_one("SELECT COUNT(*) FROM customers", &[])
        .await?
        .get(0);
    let alfa_total: i64 = db
        .client()
        .query_one(
            "SELECT SUM(o.total_cents)::BIGINT FROM orders o \
             JOIN customers c ON c.id = o.customer_id WHERE c.name = $1",
            &[&"Cliente Alfa"],
        )
        .await?
        .get(0);

    assert_eq!(customers, 3);
    assert_eq!(alfa_total, 6490);
    Ok(())
}

#[tokio::test]
async fn test_databases_are_isolated() -> TestResult {
    let first = PostgresTestDb::start_seeded().await?;
    let second = PostgresTestDb::start_seeded().await?;

    first
        .client()
        .execute("DELETE FROM orders WHERE customer_id = 1", &[])
        .await?;

    let first_orders: i64 = first
        .client()
        .query_one("SELECT COUNT(*) FROM orders", &[])
        .await?
        .get(0);
    let second_orders: i64 = second
        .client()
        .query_one("SELECT COUNT(*) FROM orders", &[])
        .await?
        .get(0);

    assert_ne!(first.port(), second.port());
    assert_eq!(first_orders, 1);
    assert_eq!(second_orders, 3);
    Ok(())
}
