const target = db.getSiblingDB("backup_test");

target.customers.insertMany([
  { _id: NumberInt(1), name: "Ana Souza", email: "ana@example.com" },
  { _id: NumberInt(2), name: "João Ação", email: "joao@example.com" },
  { _id: NumberInt(3), name: "Zoë 🚀", email: "zoe@example.com" },
]);
target.customers.createIndex({ email: 1 }, { unique: true, name: "email_unique" });

target.orders.insertMany([
  {
    _id: ObjectId("653000000000000000000001"),
    customer_id: NumberInt(1),
    total: NumberDecimal("10.50"),
    payload: BinData(0, "AP8Q"),
    placed_at: ISODate("2026-01-01T10:00:00Z"),
    items: [
      { sku: "A-1", quantity: NumberLong("9007199254740993") },
      { sku: "B-2", quantity: NumberLong("2") },
    ],
  },
  {
    _id: ObjectId("653000000000000000000002"),
    customer_id: NumberInt(1),
    total: NumberDecimal("99.99"),
    payload: null,
    placed_at: ISODate("2026-01-02T11:30:00Z"),
    items: [],
  },
  {
    _id: ObjectId("653000000000000000000003"),
    customer_id: NumberInt(2),
    total: NumberDecimal("0.01"),
    payload: BinData(0, "3q2+7w=="),
    placed_at: ISODate("2026-01-03T12:45:00Z"),
    items: [{ sku: "C-3", quantity: NumberLong("-1") }],
  },
  {
    _id: ObjectId("653000000000000000000004"),
    customer_id: NumberInt(3),
    total: NumberDecimal("1234.56"),
    payload: BinData(0, ""),
    placed_at: ISODate("2026-01-04T00:00:00Z"),
    items: [{ sku: "D-4", quantity: NumberLong("9223372036854775807") }],
  },
]);
target.orders.createIndex(
  { customer_id: 1, placed_at: -1 },
  { name: "customer_placed" },
);

target.createCollection("audit_log", {
  validator: { $jsonSchema: { bsonType: "object", required: ["note"] } },
});
target.audit_log.insertOne({ order_id: ObjectId("653000000000000000000001"), note: "criado" });
target.runCommand({
  insert: "audit_log",
  documents: [{ _id: "legado", order_id: ObjectId("653000000000000000000002") }],
  bypassDocumentValidation: true,
});

target.createCollection("empty_collection");

target.createView("customer_totals", "orders", [
  { $group: { _id: "$customer_id", total: { $sum: "$total" } } },
  { $sort: { _id: 1 } },
]);
