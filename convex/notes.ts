import { getAuthUserId } from "@convex-dev/auth/server";
import { paginationOptsValidator, paginationResultValidator } from "convex/server";
import { ConvexError, v } from "convex/values";
import { mutation, query } from "./_generated/server";

const noteFields = {
  key: v.string(),
  title: v.string(),
  body: v.string(),
  deleted: v.boolean(),
  updated_at: v.number(),
};

export const version = query({
  args: {},
  returns: v.number(),
  handler: async (ctx) => {
    const userId = await getAuthUserId(ctx);
    if (!userId) throw new ConvexError("Sign in to access your notes");
    const notebook = await ctx.db.query("notebookVersions")
      .withIndex("by_user", q => q.eq("userId", userId)).unique();
    return notebook?.version ?? 0;
  },
});

export const listPage = query({
  args: { paginationOpts: paginationOptsValidator },
  returns: paginationResultValidator(v.object(noteFields)),
  handler: async (ctx, { paginationOpts }) => {
    const userId = await getAuthUserId(ctx);
    if (!userId) {
      throw new ConvexError("Sign in to access your notes");
    }
    if (!Number.isInteger(paginationOpts.numItems) || paginationOpts.numItems < 1) {
      throw new ConvexError("Page size must be a positive integer");
    }

    const { page, ...pagination } = await ctx.db
      .query("notes")
      .withIndex("by_user_and_key", (index) => index.eq("userId", userId))
      .paginate({
        ...paginationOpts,
        numItems: Math.min(100, paginationOpts.numItems),
        maximumRowsRead: Math.min(100, paginationOpts.maximumRowsRead ?? 100),
        maximumBytesRead: Math.min(2 * 1024 * 1024, paginationOpts.maximumBytesRead ?? Infinity),
      });
    return {
      page: page.map(({ key, title, body, deleted, updated_at, _creationTime }) => ({
        key,
        title,
        body,
        deleted,
        updated_at: updated_at ?? Math.floor(_creationTime),
      })),
      ...pagination,
    };
  },
});

export const put = mutation({
  args: noteFields,
  returns: v.null(),
  handler: async (ctx, note) => {
    const userId = await getAuthUserId(ctx);
    if (!userId) {
      throw new ConvexError("Sign in to sync your notes");
    }

    if (!Number.isSafeInteger(note.updated_at) || note.updated_at < 0) {
      throw new ConvexError("Invalid note timestamp");
    }

    const previous = await ctx.db
      .query("notes")
      .withIndex("by_user_and_key", (index) =>
        index.eq("userId", userId).eq("key", note.key),
      )
      .unique();

    if (previous) {
      await ctx.db.patch(previous._id, note);
    } else {
      await ctx.db.insert("notes", { ...note, userId });
    }
    const notebook = await ctx.db.query("notebookVersions")
      .withIndex("by_user", q => q.eq("userId", userId)).unique();
    if (notebook) {
      await ctx.db.patch(notebook._id, { version: notebook.version + 1 });
    } else {
      await ctx.db.insert("notebookVersions", { userId, version: 1 });
    }
    return null;
  },
});
