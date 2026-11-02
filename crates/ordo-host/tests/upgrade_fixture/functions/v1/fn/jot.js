// jot, version 1: write a journal entry as the caller, then count theirs.
export default {
  async call(input, ctx) {
    await ctx.op("writeJournal", { body: input.body });
    const mine = await ctx.op("myJournal", {});
    return { code: "one", count: mine.length };
  },
};
