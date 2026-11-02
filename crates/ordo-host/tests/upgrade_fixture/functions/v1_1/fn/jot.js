// jot, version 1.1: as version 1, with new code that answers `two`.
export default {
  async call(input, ctx) {
    await ctx.op("writeJournal", { body: input.body });
    const mine = await ctx.op("myJournal", {});
    return { code: "two", count: mine.length };
  },
};
