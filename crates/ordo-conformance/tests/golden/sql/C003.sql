-- C003
-- source:
--   doneOrders($from: 2026-02-01)[placed, total]
--   sort placed

SELECT
  "t0"."placed" AS "placed",
  "t0"."total" AS "total"
FROM "orders" AS "t0"
WHERE "t0"."$space" = 'conformance'
  AND (("t0"."status" = 'done') AND COALESCE("t0"."placed" >= DATE '2026-02-01', false))
ORDER BY "t0"."placed" ASC NULLS LAST, "t0"."id" COLLATE "C" ASC NULLS LAST;
