// Read an explicitly supplied blob through the caller's permissions.
export default {
  async call(input, ctx) {
    const [campaign] = await ctx.op('oneCampaign', { id: input.campaign });
    if (!campaign) throw new Error('This campaign is unavailable.');
    const assets = await ctx.op('campaignFiles', { campaign: input.campaign });
    const asset = assets.find(row => row.file === input.file);
    if (!asset) throw new Error('This file does not belong to the selected campaign.');
    const file = await ctx.blob(input.file);
    const bytes = await file.bytes();
    console.log('Inspected one campaign file.');
    return { name: asset.name, type: file.type, size: bytes.length };
  },
};
