import { randomUUID } from 'node:crypto';
import { test, expect } from '@playwright/test';
import { stack, owner, rows, refused } from '../support.js';
const unique = () => `studio-${randomUUID()}`;

test('staff edit public content with revision checks and remove it again', async ({ request }) => {
  const headers = await owner(request);
  for (const [add, read, edit, remove, publicRead, original] of [
    [
      'addProduct',
      'studioProducts',
      'editProduct',
      'deleteProduct',
      'catalogue',
      {
        slug: unique(),
        name: unique(),
        tagline: 'Before',
        description: 'Details',
        price: '19.50',
        features: ['Warm light'],
        rank: 1,
        internalNote: 'Private launch date',
      },
    ],
    [
      'addPlan',
      'studioPlans',
      'editPlan',
      'deletePlan',
      'pricing',
      {
        name: unique(),
        tagline: 'Before',
        price: '12.00',
        period: 'month',
        features: ['Support'],
        highlighted: false,
        rank: 1,
      },
    ],
    [
      'addTestimonial',
      'studioReviews',
      'editReview',
      'deleteReview',
      'reviews',
      { author: unique(), role: 'Designer', company: 'Studio', quote: 'Before', stars: 4, rank: 1 },
    ],
  ] as const) {
    const result = await rows(await request.post(`${stack().door}/op/${add}`, { headers, data: original }));
    const id = result[0].created;
    try {
      const current = (await rows(await request.get(`${stack().door}/op/${read}`, { headers }))).find(
        (r) => r.id === id,
      )!;
      const data: Record<string, unknown> = { ...original, id, rev: current.rev };
      delete data.slug;
      if ('quote' in original) data.quote = 'After';
      else data.tagline = 'After';
      await rows(await request.post(`${stack().door}/op/${edit}`, { headers, data }));
      await refused(await request.post(`${stack().door}/op/${edit}`, { headers, data }), 409, 'cas_miss');
      const published = (await rows(await request.get(`${stack().door}/op/${publicRead}`))).find((r) => r.id === id)!;
      expect(published).toMatchObject('quote' in original ? { quote: 'After' } : { tagline: 'After' });
      expect(published).not.toHaveProperty('internalNote');
    } finally {
      await rows(await request.delete(`${stack().door}/op/${remove}`, { headers, params: { id } }));
    }
    expect((await rows(await request.get(`${stack().door}/op/${publicRead}`))).some((r) => r.id === id)).toBe(false);
  }
});

test('staff triage an enquiry, see the report change and delete personal data', async ({ request }) => {
  const headers = await owner(request);
  const id = (
    await rows(
      await request.post(`${stack().door}/op/contact`, {
        data: { name: unique(), email: `${unique()}@example.org`, company: 'Studio', message: 'Please call' },
      }),
    )
  )[0].created;
  try {
    const row = (await rows(await request.get(`${stack().door}/op/inbox`, { headers }))).find((r) => r.id === id)!;
    const before =
      (await rows(await request.get(`${stack().door}/op/enquiryReport`, { headers }))).find((r) => r.status === 'won')
        ?.total ?? 0;
    await rows(
      await request.post(`${stack().door}/op/triageEnquiry`, {
        headers,
        data: { id, rev: row.rev, status: 'won', notes: 'Agreed on a plan' },
      }),
    );
    const after = await rows(await request.get(`${stack().door}/op/enquiryReport`, { headers }));
    expect(Number(after.find((r) => r.status === 'won')?.total)).toBe(Number(before) + 1);
    const filtered = await rows(
      await request.get(`${stack().door}/op/inbox`, {
        headers,
        params: { $refine: JSON.stringify({ where: [{ field: 'status', op: 'eq', value: 'won' }] }) },
      }),
    );
    expect(filtered.some((r) => r.id === id && r.notes === 'Agreed on a plan')).toBe(true);
    await refused(await request.get(`${stack().door}/op/inbox`), 403, 'access_denied');
  } finally {
    await rows(await request.delete(`${stack().door}/op/deleteEnquiry`, { headers, params: { id } }));
  }
  expect((await rows(await request.get(`${stack().door}/op/inbox`, { headers }))).some((r) => r.id === id)).toBe(false);
});

test('a campaign keeps its JSON tags, files and reminder, then deletes owned children', async ({ request }) => {
  const headers = await owner(request);
  const tag = unique();
  const payload = {
    title: unique(),
    brief: 'Autumn launch',
    tags: [tag, 'launch'],
    budget: '1500.25',
    due: '2027-01-15',
    created: new Date().toISOString().replace(/\.\d{3}Z$/, 'Z'),
  };
  const created = await rows(
    await request.post(`${stack().door}/op/createCampaign`, {
      headers: { ...headers, 'Idempotency-Key': tag },
      data: payload,
    }),
  );
  const replay = await request.post(`${stack().door}/op/createCampaign`, {
    headers: { ...headers, 'Idempotency-Key': tag },
    data: payload,
  });
  expect(await rows(replay)).toEqual(created);
  const id = created[0].created;
  let fileId = '';
  let handle = '';
  try {
    const found = await rows(await request.get(`${stack().door}/op/campaignsTagged`, { headers, params: { tag } }));
    expect(found).toHaveLength(1);
    expect(found[0]).toMatchObject({ id, tags: [tag, 'launch'], status: 'draft' });
    await rows(
      await request.post(`${stack().door}/op/editCampaign`, {
        headers,
        data: {
          id,
          rev: found[0].rev,
          title: payload.title,
          brief: 'Ready',
          tags: payload.tags,
          budget: payload.budget,
          due: null,
          status: 'published',
        },
      }),
    );
    expect(
      (await rows(await request.get(`${stack().door}/op/campaignReport`, { headers }))).some(
        (r) => r.status === 'published' && Number(r.total) >= 1,
      ),
    ).toBe(true);
    fileId = (
      await rows(
        await request.post(`${stack().door}/op/addAsset`, {
          headers,
          data: { campaign: id, name: 'brief.txt', kind: 'text/plain', size: 12 },
        }),
      )
    )[0].created;
    const upload = await request.put(`${stack().door}/blob/assets/file/${fileId}`, {
      headers: { ...headers, 'Content-Type': 'text/plain' },
      data: Buffer.from('Launch brief'),
    });
    expect(upload.ok(), await upload.text()).toBe(true);
    handle = (
      await rows(await request.get(`${stack().door}/op/campaignFiles`, { headers, params: { campaign: id } }))
    )[0].file;
    const download = await request.get(`${stack().door}/blob/${handle}`, { headers });
    expect(download.status()).toBe(200);
    expect(await download.text()).toBe('Launch brief');
    // Private file fields require a bearer token, even with a valid handle.
    await refused(await request.get(`${stack().door}/blob/${handle}`), 403, 'access_denied');
    await rows(
      await request.post(`${stack().door}/op/setReminder`, {
        headers,
        data: { campaign: id, due: new Date(Date.now() - 60_000).toISOString().replace(/\.\d{3}Z$/, 'Z') },
      }),
    );
    await expect
      .poll(
        async () =>
          (await rows(await request.get(`${stack().door}/op/campaignReminders`, { headers }))).find(
            (r) => r.campaign === id,
          )?.fired,
        { timeout: 90_000 },
      )
      .toBeTruthy();
    await rows(await request.delete(`${stack().door}/op/clearReminder`, { headers, params: { campaign: id } }));
    await rows(
      await request.post(`${stack().door}/op/setReminder`, {
        headers,
        data: { campaign: id, due: '2030-01-01T00:00:00Z' },
      }),
    );
  } finally {
    await rows(await request.delete(`${stack().door}/op/deleteCampaign`, { headers, params: { id } }));
  }
  expect(
    await rows(await request.get(`${stack().door}/op/campaignFiles`, { headers, params: { campaign: id } })),
  ).toEqual([]);
  expect(
    (await rows(await request.get(`${stack().door}/op/campaignReminders`, { headers }))).some((r) => r.campaign === id),
  ).toBe(false);
  await refused(await request.get(`${stack().door}/blob/${handle}`, { headers }), 404, 'not_found');
});

test('a visitor cannot read or mutate any workspace surface', async ({ request }) => {
  for (const op of [
    'studioProducts',
    'studioPlans',
    'studioReviews',
    'inbox',
    'enquiryReport',
    'campaignList',
    'campaignReport',
    'campaignReminders',
    'people',
    'allAssets',
  ]) {
    await refused(await request.get(`${stack().door}/op/${op}`), 403, 'access_denied');
  }
  for (const op of [
    'editProduct',
    'editPlan',
    'editReview',
    'triageEnquiry',
    'createCampaign',
    'editCampaign',
    'addAsset',
    'setReminder',
    'grantCampaignsAccess',
  ]) {
    await refused(await request.post(`${stack().door}/op/${op}`, { data: {} }), 403, 'access_denied');
  }
});

test('editorial review runs as the caller and a stale approval leaves no history row', async ({ request }) => {
  const headers = await owner(request);
  const original = {
    title: unique(),
    brief: 'A short brief.',
    tags: [],
    budget: '0.00',
    due: null,
    created: new Date().toISOString().replace(/\.\d{3}Z$/, 'Z'),
  };
  const id = (await rows(await request.post(`${stack().door}/op/createCampaign`, { headers, data: original })))[0]
    .created;
  try {
    const review = await request.post(`${stack().door}/fn/reviewBrief`, { headers, data: { campaign: id } });
    expect(review.status(), await review.text()).toBe(200);
    expect((await review.json()).result).toMatchObject({ words: 3, ready: false });
    await refused(
      await request.post(`${stack().door}/fn/reviewBrief`, { data: { campaign: id } }),
      403,
      'access_denied',
    );
    const before = (await rows(await request.get(`${stack().door}/op/campaignList`, { headers }))).find(
      (r) => r.id === id,
    )!;
    const data = {
      id,
      rev: before.rev,
      note: 'Editorial approval',
      at: new Date().toISOString().replace(/\.\d{3}Z$/, 'Z'),
    };
    // The second write fails after the campaign update: neither may land.
    await refused(
      await request.post(`${stack().door}/op/approveCampaign`, { headers, data: { ...data, note: '' } }),
      409,
      'check_violation',
    );
    expect(
      (await rows(await request.get(`${stack().door}/op/campaignList`, { headers }))).find((r) => r.id === id),
    ).toEqual(before);
    expect(
      await rows(await request.get(`${stack().door}/op/approvalHistory`, { headers, params: { campaign: id } })),
    ).toEqual([]);
    await rows(await request.post(`${stack().door}/op/approveCampaign`, { headers, data }));
    await refused(await request.post(`${stack().door}/op/approveCampaign`, { headers, data }), 409, 'cas_miss');
    const history = await rows(
      await request.get(`${stack().door}/op/approvalHistory`, { headers, params: { campaign: id } }),
    );
    expect(history).toHaveLength(1);
    expect(history[0].note).toBe('Editorial approval');
    const after = (await rows(await request.get(`${stack().door}/op/campaignList`, { headers }))).find(
      (r) => r.id === id,
    )!;
    expect(after.status).toBe('published');
    await refused(
      await request.post(`${stack().door}/op/editCampaign`, {
        headers,
        data: { ...original, created: undefined, id, rev: after.rev, status: 'impossible' },
      }),
      409,
      'check_violation',
    );
    await refused(
      await request.post(`${stack().door}/op/editCampaign`, {
        headers,
        data: { ...original, created: undefined, id, rev: after.rev, status: 'draft', budget: '-1.00' },
      }),
      409,
      'check_violation',
    );
    expect(
      (await rows(await request.get(`${stack().door}/op/campaignList`, { headers }))).find((r) => r.id === id),
    ).toEqual(after);
  } finally {
    await rows(await request.delete(`${stack().door}/op/deleteCampaign`, { headers, params: { id } }));
  }
  expect(
    await rows(await request.get(`${stack().door}/op/approvalHistory`, { headers, params: { campaign: id } })),
  ).toEqual([]);
});
