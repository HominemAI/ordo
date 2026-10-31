CREATE TABLE "members" (
  "$space" text NOT NULL,
  "id" text NOT NULL,
  "rev" bigint NOT NULL DEFAULT 1,
  "principal" text NOT NULL,
  "displayName" text NOT NULL,
  "kind" text NOT NULL,
  "memberCount" bigint NOT NULL,
  "role" text,
  "status" text NOT NULL,
  "joined" date NOT NULL,
  CONSTRAINT "members$pk" PRIMARY KEY ("$space", "id")
);
CREATE TABLE "groupMembers" (
  "$space" text NOT NULL,
  "id" text NOT NULL,
  "rev" bigint NOT NULL DEFAULT 1,
  "group" text NOT NULL,
  "member" text NOT NULL,
  CONSTRAINT "groupMembers$pk" PRIMARY KEY ("$space", "id")
);
CREATE TABLE "vendors" (
  "$space" text NOT NULL,
  "id" text NOT NULL,
  "rev" bigint NOT NULL DEFAULT 1,
  "name" text NOT NULL,
  "region" text NOT NULL,
  "active" boolean NOT NULL DEFAULT true,
  CONSTRAINT "vendors$pk" PRIMARY KEY ("$space", "id")
);
CREATE TABLE "categories" (
  "$space" text NOT NULL,
  "id" text NOT NULL,
  "rev" bigint NOT NULL DEFAULT 1,
  "name" text NOT NULL,
  "parent" text,
  "depth" bigint NOT NULL,
  "$path" text NOT NULL,
  CONSTRAINT "categories$pk" PRIMARY KEY ("$space", "id")
);
CREATE TABLE "products" (
  "$space" text NOT NULL,
  "id" text NOT NULL,
  "rev" bigint NOT NULL DEFAULT 1,
  "vendor" text NOT NULL,
  "category" text NOT NULL,
  "name" text NOT NULL,
  "price" bigint NOT NULL,
  "active" boolean NOT NULL DEFAULT true,
  CONSTRAINT "products$pk" PRIMARY KEY ("$space", "id")
);
CREATE TABLE "customers" (
  "$space" text NOT NULL,
  "id" text NOT NULL,
  "rev" bigint NOT NULL DEFAULT 1,
  "name" text NOT NULL,
  "email" bytea,
  "email$bidx" bytea NOT NULL,
  "tier" text NOT NULL DEFAULT 'basic',
  CONSTRAINT "customers$pk" PRIMARY KEY ("$space", "id")
);
CREATE TABLE "orders" (
  "$space" text NOT NULL,
  "id" text NOT NULL,
  "rev" bigint NOT NULL DEFAULT 1,
  "customer" text NOT NULL,
  "placed" date NOT NULL,
  "status" text NOT NULL DEFAULT 'open',
  "total" bigint NOT NULL,
  "memo" bytea NOT NULL,
  CONSTRAINT "orders$pk" PRIMARY KEY ("$space", "id")
);
CREATE TABLE "orderItems" (
  "$space" text NOT NULL,
  "id" text NOT NULL,
  "rev" bigint NOT NULL DEFAULT 1,
  "order" text NOT NULL,
  "product" text NOT NULL,
  "qty" bigint NOT NULL,
  "amount" bigint NOT NULL,
  CONSTRAINT "orderItems$pk" PRIMARY KEY ("$space", "id")
);
CREATE TABLE "projects" (
  "$space" text NOT NULL,
  "id" text NOT NULL,
  "rev" bigint NOT NULL DEFAULT 1,
  "name" text NOT NULL,
  "owner" text NOT NULL,
  CONSTRAINT "projects$pk" PRIMARY KEY ("$space", "id")
);
CREATE TABLE "projectMembers" (
  "$space" text NOT NULL,
  "id" text NOT NULL,
  "rev" bigint NOT NULL DEFAULT 1,
  "project" text NOT NULL,
  "member" text NOT NULL,
  "canWrite" boolean NOT NULL DEFAULT false,
  CONSTRAINT "projectMembers$pk" PRIMARY KEY ("$space", "id")
);
CREATE TABLE "tasks" (
  "$space" text NOT NULL,
  "id" text NOT NULL,
  "rev" bigint NOT NULL DEFAULT 1,
  "project" text NOT NULL,
  "assignee" text,
  "title" text NOT NULL,
  "status" text NOT NULL DEFAULT 'open',
  CONSTRAINT "tasks$pk" PRIMARY KEY ("$space", "id")
);
CREATE TABLE "taskNotes" (
  "$space" text NOT NULL,
  "id" text NOT NULL,
  "rev" bigint NOT NULL DEFAULT 1,
  "task" text NOT NULL,
  "body" text NOT NULL,
  CONSTRAINT "taskNotes$pk" PRIMARY KEY ("$space", "id")
);
CREATE TABLE "boards" (
  "$space" text NOT NULL,
  "id" text NOT NULL,
  "rev" bigint NOT NULL DEFAULT 1,
  "name" text NOT NULL,
  CONSTRAINT "boards$pk" PRIMARY KEY ("$space", "id")
);
CREATE TABLE "cards" (
  "$space" text NOT NULL,
  "id" text NOT NULL,
  "rev" bigint NOT NULL DEFAULT 1,
  "board" text NOT NULL,
  "title" text NOT NULL,
  "done" boolean NOT NULL DEFAULT false,
  CONSTRAINT "cards$pk" PRIMARY KEY ("$space", "id")
);
CREATE TABLE "folders" (
  "$space" text NOT NULL,
  "id" text NOT NULL,
  "rev" bigint NOT NULL DEFAULT 1,
  "owner" text NOT NULL,
  "name" text NOT NULL,
  "parent" text,
  "depth" bigint NOT NULL,
  "$path" text NOT NULL,
  CONSTRAINT "folders$pk" PRIMARY KEY ("$space", "id")
);
CREATE TABLE "ledgerEntries" (
  "$space" text NOT NULL,
  "id" text NOT NULL,
  "rev" bigint NOT NULL DEFAULT 1,
  "externalId" text NOT NULL,
  "owner" text NOT NULL,
  "amount" bigint NOT NULL,
  "note" text NOT NULL DEFAULT '',
  CONSTRAINT "ledgerEntries$pk" PRIMARY KEY ("$space", "id")
);
CREATE TABLE "coupons" (
  "$space" text NOT NULL,
  "id" text NOT NULL,
  "rev" bigint NOT NULL DEFAULT 1,
  "code" text NOT NULL,
  "slug" text NOT NULL,
  "value" bigint NOT NULL,
  CONSTRAINT "coupons$pk" PRIMARY KEY ("$space", "id")
);
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
ALTER TABLE "groupMembers" ADD CONSTRAINT "groupMembers$group$fk" FOREIGN KEY ("$space", "group") REFERENCES "members" ("$space", "id");
ALTER TABLE "groupMembers" ADD CONSTRAINT "groupMembers$member$fk" FOREIGN KEY ("$space", "member") REFERENCES "members" ("$space", "id");
ALTER TABLE "categories" ADD CONSTRAINT "categories$parent$fk" FOREIGN KEY ("$space", "parent") REFERENCES "categories" ("$space", "id");
ALTER TABLE "products" ADD CONSTRAINT "products$vendor$fk" FOREIGN KEY ("$space", "vendor") REFERENCES "vendors" ("$space", "id");
ALTER TABLE "products" ADD CONSTRAINT "products$category$fk" FOREIGN KEY ("$space", "category") REFERENCES "categories" ("$space", "id");
ALTER TABLE "orders" ADD CONSTRAINT "orders$customer$fk" FOREIGN KEY ("$space", "customer") REFERENCES "customers" ("$space", "id");
ALTER TABLE "orderItems" ADD CONSTRAINT "orderItems$order$fk" FOREIGN KEY ("$space", "order") REFERENCES "orders" ("$space", "id");
ALTER TABLE "orderItems" ADD CONSTRAINT "orderItems$product$fk" FOREIGN KEY ("$space", "product") REFERENCES "products" ("$space", "id");
ALTER TABLE "projects" ADD CONSTRAINT "projects$owner$fk" FOREIGN KEY ("$space", "owner") REFERENCES "members" ("$space", "id");
ALTER TABLE "projectMembers" ADD CONSTRAINT "projectMembers$project$fk" FOREIGN KEY ("$space", "project") REFERENCES "projects" ("$space", "id");
ALTER TABLE "projectMembers" ADD CONSTRAINT "projectMembers$member$fk" FOREIGN KEY ("$space", "member") REFERENCES "members" ("$space", "id");
ALTER TABLE "tasks" ADD CONSTRAINT "tasks$project$fk" FOREIGN KEY ("$space", "project") REFERENCES "projects" ("$space", "id");
ALTER TABLE "tasks" ADD CONSTRAINT "tasks$assignee$fk" FOREIGN KEY ("$space", "assignee") REFERENCES "members" ("$space", "id");
ALTER TABLE "taskNotes" ADD CONSTRAINT "taskNotes$task$fk" FOREIGN KEY ("$space", "task") REFERENCES "tasks" ("$space", "id");
ALTER TABLE "cards" ADD CONSTRAINT "cards$board$fk" FOREIGN KEY ("$space", "board") REFERENCES "boards" ("$space", "id");
ALTER TABLE "folders" ADD CONSTRAINT "folders$owner$fk" FOREIGN KEY ("$space", "owner") REFERENCES "members" ("$space", "id");
ALTER TABLE "folders" ADD CONSTRAINT "folders$parent$fk" FOREIGN KEY ("$space", "parent") REFERENCES "folders" ("$space", "id");
ALTER TABLE "ledgerEntries" ADD CONSTRAINT "ledgerEntries$owner$fk" FOREIGN KEY ("$space", "owner") REFERENCES "members" ("$space", "id");
ALTER TABLE "boardsAccess" ADD CONSTRAINT "boardsAccess$board$fk" FOREIGN KEY ("$space", "board") REFERENCES "boards" ("$space", "id");
ALTER TABLE "boardsAccess" ADD CONSTRAINT "boardsAccess$member$fk" FOREIGN KEY ("$space", "member") REFERENCES "members" ("$space", "id");
ALTER TABLE "categories" ADD CONSTRAINT "categories$name$uq" UNIQUE NULLS NOT DISTINCT ("$space", "name");
ALTER TABLE "customers" ADD CONSTRAINT "customers$email$uq" UNIQUE ("$space", "email$bidx");
ALTER TABLE "ledgerEntries" ADD CONSTRAINT "ledgerEntries$externalId$uq" UNIQUE NULLS NOT DISTINCT ("$space", "externalId");
ALTER TABLE "coupons" ADD CONSTRAINT "coupons$code$uq" UNIQUE NULLS NOT DISTINCT ("$space", "code");
ALTER TABLE "coupons" ADD CONSTRAINT "coupons$slug$uq" UNIQUE NULLS NOT DISTINCT ("$space", "slug");
ALTER TABLE "groupMembers" ADD CONSTRAINT "groupMembers$group$member$uq" UNIQUE NULLS NOT DISTINCT ("$space", "group", "member");
ALTER TABLE "boardsAccess" ADD CONSTRAINT "boardsAccess$board$member$uq" UNIQUE NULLS NOT DISTINCT ("$space", "board", "member");
CREATE INDEX "groupMembers$member$ix" ON "groupMembers" ("$space", "member");
CREATE INDEX "categories$parent$ix" ON "categories" ("$space", "parent");
CREATE INDEX "products$vendor$ix" ON "products" ("$space", "vendor");
CREATE INDEX "products$category$ix" ON "products" ("$space", "category");
CREATE INDEX "orders$customer$ix" ON "orders" ("$space", "customer");
CREATE INDEX "orderItems$order$ix" ON "orderItems" ("$space", "order");
CREATE INDEX "orderItems$product$ix" ON "orderItems" ("$space", "product");
CREATE INDEX "projects$owner$ix" ON "projects" ("$space", "owner");
CREATE INDEX "projectMembers$project$ix" ON "projectMembers" ("$space", "project");
CREATE INDEX "projectMembers$member$ix" ON "projectMembers" ("$space", "member");
CREATE INDEX "tasks$project$ix" ON "tasks" ("$space", "project");
CREATE INDEX "tasks$assignee$ix" ON "tasks" ("$space", "assignee");
CREATE INDEX "taskNotes$task$ix" ON "taskNotes" ("$space", "task");
CREATE INDEX "cards$board$ix" ON "cards" ("$space", "board");
CREATE INDEX "folders$owner$ix" ON "folders" ("$space", "owner");
CREATE INDEX "folders$parent$ix" ON "folders" ("$space", "parent");
CREATE INDEX "ledgerEntries$owner$ix" ON "ledgerEntries" ("$space", "owner");
CREATE INDEX "boardsAccess$member$ix" ON "boardsAccess" ("$space", "member");
