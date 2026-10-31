-- C005
-- source:
--   doneOrders($from: 2026-01-01, total > 5000)[placed, total, customer.name]
--   sort -total

SELECT
  "t0"."placed" AS "placed",
  "t0"."total" AS "total",
  "t1"."name" AS "name"
FROM "orders" AS "t0"
LEFT JOIN "customers" AS "t1" ON "t0"."customer" = "t1"."id" AND "t1"."$space" = 'conformance'
WHERE "t0"."$space" = 'conformance'
  AND (("t0"."status" = 'done') AND COALESCE("t0"."placed" >= DATE '2026-01-01', false))
  AND COALESCE("t0"."total" > 5000, false)
ORDER BY "t0"."total" DESC NULLS LAST, "t0"."id" COLLATE "C" ASC NULLS LAST;
