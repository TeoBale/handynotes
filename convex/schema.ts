import { authTables } from "@convex-dev/auth/server";
import { defineSchema, defineTable } from "convex/server";
import { v } from "convex/values";

export default defineSchema({
  ...authTables,
  notebookVersions: defineTable({ userId: v.id("users"), version: v.number() })
    .index("by_user", ["userId"]),
  notes: defineTable({
    userId: v.id("users"),
    key: v.string(),
    title: v.string(),
    body: v.string(),
    deleted: v.boolean(),
    updated_at: v.optional(v.number()),
  }).index("by_user_and_key", ["userId", "key"]),
});
