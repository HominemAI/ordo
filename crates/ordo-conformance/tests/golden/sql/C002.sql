-- C002
-- source:
--   cheapActive($cap: 5000)

SELECT
  "t0"."name" AS "name",
  "t0"."price" AS "price",
  "t0"."category" AS "category"
FROM "products" AS "t0"
WHERE "t0"."$space" = 'conformance'
  AND ("t0"."active" = true)
  AND COALESCE("t0"."price" < 5000, false)
ORDER BY "t0"."id" COLLATE "C" ASC NULLS LAST;
