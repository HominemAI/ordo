import { randomUUID } from 'node:crypto';
import { test, expect } from '@playwright/test';
import { owner, rows, refused, stack } from '../support.js';

test('uploaded files support bounded reads, retain metadata and serve active content as downloads', async ({ request }) => {
  const headers = await owner(request);
  const campaign = (await rows(await request.post(`${stack().door}/op/createCampaign`, { headers, data: {
    title: `Download ${randomUUID()}`, brief: 'A private file collection', tags: [], budget: '1.00', due: null,
    created: new Date().toISOString().replace(/\.\d{3}Z$/, 'Z'),
  } })))[0].created;
  try {
    for (const [mime, content] of [
      ['text/plain', '0123456789abcdefghijklmnopqrstuvwxyz'],
      ['text/html', '<script>globalThis.fileScriptRan = true</script>'],
      ['image/svg+xml', '<svg xmlns="http://www.w3.org/2000/svg"><script>1</script></svg>'],
    ]) {
      const id = (await rows(await request.post(`${stack().door}/op/addAsset`, { headers,
        data: { campaign, name: `download-${mime}`, kind: mime, size: Buffer.byteLength(content) },
      })))[0].created;
      const upload = await request.put(`${stack().door}/blob/assets/file/${id}`, {
        headers: { ...headers, 'Content-Type': mime }, data: Buffer.from(content),
      });
      expect(upload.status()).toBe(200);
      const handle = (await rows(await request.get(`${stack().door}/op/campaignFiles`, {
        headers, params: { campaign },
      }))).find(row => row.id === id)!.file;
      for (const door of [stack().door, stack().doorB]) {
        const url = `${door}/blob/${handle}`;
        const meta = await request.get(`${url}/meta`, { headers });
        expect(meta.status()).toBe(200);
        expect(await meta.json()).toEqual({ mime, size: Buffer.byteLength(content) });
        const full = await request.get(url, { headers });
        expect(full.status()).toBe(200);
        expect(await full.text()).toBe(content);
        expect(full.headers()['accept-ranges']).toBe('bytes');
        expect(full.headers()['cache-control']).toBe('private, max-age=31536000, immutable');
        expect(full.headers()['x-content-type-options']).toBe('nosniff');
        expect(full.headers()['content-security-policy']).toBe("sandbox; default-src 'none'");
        expect(full.headers()['content-disposition']).toBe(mime === 'text/plain' ? 'inline' : 'attachment');
        expect(full.headers()['content-type']).toBe(mime === 'text/plain' ? 'text/plain; charset=utf-8' : 'application/octet-stream');
        for (const [range, from, until] of [
          ['bytes=0-0', 0, 1], ['bytes=2-5', 2, 6], ['bytes=8-', 8, content.length], ['bytes=0-2,5-8', 0, 3],
        ] as const) {
          const partial = await request.get(url, { headers: { ...headers, Range: range } });
          expect(partial.status()).toBe(206);
          expect(await partial.text()).toBe(content.slice(from, until));
          expect(partial.headers()['content-range']).toBe(`bytes ${from}-${until - 1}/${content.length}`);
        }
        // Unsupported/malformed Range is documented to fall back to a full read.
        for (const range of ['bytes=-3', 'bytes=5-2', 'not-a-range']) {
          const fallback = await request.get(url, { headers: { ...headers, Range: range } });
          expect(fallback.status()).toBe(200);
          expect(await fallback.text()).toBe(content);
        }
        await refused(await request.get(url, { headers: { ...headers, Range: 'bytes=999999-' } }), 400, 'bound_violation');
        await refused(await request.get(`${url}/meta`), 403, 'access_denied');
        await refused(await request.get(url, { headers: { Range: 'bytes=0-0' } }), 403, 'access_denied');
        expect(await (await request.get(url, { headers })).text()).toBe(content);
      }
    }
  } finally {
    await rows(await request.delete(`${stack().door}/op/deleteCampaign`, { headers, params: { id: campaign } }));
  }
});
