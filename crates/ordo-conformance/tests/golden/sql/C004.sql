-- C004
-- source:
--   top3Orders(total < 15000)[placed, total]

SELECT
  "t1"."placed" AS "placed",
  "t1"."total" AS "total"
FROM (
  SELECT
    "t0"."placed" AS "placed",
    "t0"."total" AS "total",
    "t0"."total" AS "$s0",
    "t0"."id" AS "$tb0"
  FROM "orders" AS "t0"
  WHERE "t0"."$space" = 'conformance'
  ORDER BY "t0"."total" DESC NULLS LAST, "t0"."id" COLLATE "C" ASC NULLS LAST
  LIMIT 3
) AS "t1"
WHERE COALESCE("t1"."total" < 15000, false)
ORDER BY "t1"."$s0" DESC NULLS LAST, "t1"."$tb0" COLLATE "C" ASC NULLS LAST;
