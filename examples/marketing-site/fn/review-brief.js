// Deterministic editorial checks. The host enforces the caller's campaign ACL.
export default {
  async call(input, ctx) {
    const [campaign] = await ctx.op('oneCampaign', { id: input.campaign });
    if (!campaign) throw new Error('This campaign is unavailable.');
    const words = String(campaign.brief || '').trim().split(/\s+/).filter(Boolean).length;
    const checks = [
      { label: 'A descriptive title', passed: String(campaign.title).trim().length >= 8 },
      { label: 'A brief of at least 20 words', passed: words >= 20 },
      { label: 'A planned budget', passed: Number(campaign.budget) > 0 },
      { label: 'A due date', passed: campaign.due != null },
    ];
    return { words, checks, ready: checks.every(check => check.passed) };
  },
};
