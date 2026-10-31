CREATE TABLE "boardsAccess" (
  "$space" text NOT NULL,
  "id" text NOT NULL,
  "rev" bigint NOT NULL DEFAULT 1,
  "board" text NOT NULL,
  "member" text NOT NULL,
  "level" text NOT NULL,
  CONSTRAINT "boardsAccess$level$enum" CHECK ("level" IN ('read', 'write', 'admin')),
  CONSTRAINT "boardsAccess$pk" PRIMARY KEY ("$space", "id")
);
ALTER TABLE "boardsAccess" ADD CONSTRAINT "boardsAccess$board$fk" FOREIGN KEY ("$space", "board") REFERENCES "boards" ("$space", "id");
ALTER TABLE "boardsAccess" ADD CONSTRAINT "boardsAccess$member$fk" FOREIGN KEY ("$space", "member") REFERENCES "members" ("$space", "id");
ALTER TABLE "boardsAccess" ADD CONSTRAINT "boardsAccess$board$member$uq" UNIQUE NULLS NOT DISTINCT ("$space", "board", "member");
CREATE INDEX "boardsAccess$member$ix" ON "boardsAccess" ("$space", "member");
