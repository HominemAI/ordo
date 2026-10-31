-- C007
-- source:
--   activeProducts[label: name, price]
--   sort price

SELECT
  "t0"."name" AS "label",
  "t0"."price" AS "price"
FROM "products" AS "t0"
WHERE "t0"."$space" = 'conformance'
  AND ("t0"."active" = true)
ORDER BY "t0"."price" ASC NULLS LAST, "t0"."id" COLLATE "C" ASC NULLS LAST;
