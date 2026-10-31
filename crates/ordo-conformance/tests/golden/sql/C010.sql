-- C010
-- source:
--   spendByCustomer($from: 2026-01-01)[*, -n]
--   sort -spent

SELECT
  "t1"."customer" AS "customer",
  "t1"."spent" AS "spent"
FROM (
  SELECT
    "t0"."customer" AS "customer",
    sum("t0"."total") AS "spent",
    "t0"."customer" AS "$tb0"
  FROM "orders" AS "t0"
  WHERE "t0"."$space" = 'conformance'
  AND (("t0"."status" = 'done') AND COALESCE("t0"."placed" >= DATE '2026-01-01', false))
  GROUP BY "t0"."customer"
  ORDER BY "t0"."customer" COLLATE "C" ASC NULLS LAST
) AS "t1"
ORDER BY "t1"."spent" DESC NULLS LAST, "t1"."$tb0" COLLATE "C" ASC NULLS LAST;
