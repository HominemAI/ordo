-- C008
-- source:
--   cheapActive($cap: 10000)[name, sale: price - 500]
--   sort sale

SELECT
  "t0"."name" AS "name",
  ("t0"."price" - 500) AS "sale"
FROM "products" AS "t0"
WHERE "t0"."$space" = 'conformance'
  AND ("t0"."active" = true)
  AND COALESCE("t0"."price" < 10000, false)
ORDER BY ("t0"."price" - 500) ASC NULLS LAST, "t0"."id" COLLATE "C" ASC NULLS LAST;
