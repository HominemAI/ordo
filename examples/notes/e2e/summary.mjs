// Turn Playwright's JSON report into a short Markdown list: one line per use
// case, passed or failed. CI appends it to the job summary
// ($GITHUB_STEP_SUMMARY); run by hand, it prints to the terminal.
//
//     node summary.mjs [report/results.json]
//     node summary.mjs --screenshots [report-screenshots/results.json]
//
// With --screenshots it writes the "Screenshots (report only)" section instead:
// each screen matched or changed, and where the expected, actual and diff
// pictures of a change are. Screenshots block nothing (docs/howto-e2e.md).

import { appendFileSync, existsSync, readFileSync, readdirSync, statSync } from 'node:fs';
import { relative } from 'node:path';

const args = process.argv.slice(2);
const shots = args.includes('--screenshots');
const given = args.find((a) => !a.startsWith('--'));
const file =
  given ?? new URL(shots ? 'report-screenshots/results.json' : 'report/results.json', import.meta.url).pathname;
const out = process.env.GITHUB_STEP_SUMMARY;
const write = (text) => (out ? appendFileSync(out, text) : process.stdout.write(text));

if (shots) {
  screenshotsSection();
  process.exit(0);
}

if (!existsSync(file)) {
  write(`### Notes end-to-end\n\nNo results: ${file} was not written. The stack or Playwright did not start; read the job log.\n`);
  process.exit(0);
}

const report = JSON.parse(readFileSync(file, 'utf8'));
const rows = [];
const walk = (suite) => {
  for (const s of suite.suites ?? []) walk(s);
  for (const spec of suite.specs ?? []) {
    for (const t of spec.tests) {
      const last = t.results.at(-1);
      const status = last?.status ?? 'skipped';
      const error = last?.errors?.[0]?.message?.split('\n')[0]?.replace(/\u001b\[[0-9;]*m/g, '') ?? '';
      rows.push({ project: t.projectName, file: spec.file, title: spec.title, status, error });
    }
  }
};
for (const s of report.suites ?? []) walk(s);

const mark = { passed: 'passed', failed: 'FAILED', timedOut: 'FAILED (timed out)', skipped: 'skipped', interrupted: 'interrupted' };
const failed = rows.filter((r) => r.status === 'failed' || r.status === 'timedOut' || r.status === 'interrupted');
let text = `### Notes end-to-end: ${rows.length - failed.length} of ${rows.length} use cases passed\n\n`;
text += '| Use case | Where | Result |\n|---|---|---|\n';
for (const r of rows) {
  const cell = (s) => s.replace(/\|/g, '\\|');
  const why = r.error && r.status !== 'passed' ? `<br><sub>${cell(r.error.slice(0, 200))}</sub>` : '';
  text += `| ${cell(r.title)} | ${r.project} | ${mark[r.status] ?? r.status}${why} |\n`;
}
if (failed.length > 0) {
  text += '\nTraces, screenshots and video of each failure are in the `notes-e2e-report` artifact. ';
  text += 'Open `report/html/index.html`, or `npx playwright show-trace <trace.zip>`.\n';
}
write(text + '\n');

/** The "Screenshots (report only)" section. */
function screenshotsSection() {
  const here = new URL('.', import.meta.url).pathname;
  let text = '### Screenshots (report only)\n\n';
  if (!existsSync(file)) {
    write(`${text}No results: ${file} was not written. The stack or Playwright did not start. This step blocks nothing; read its log.\n\n`);
    return;
  }
  const report = JSON.parse(readFileSync(file, 'utf8'));
  const found = [];
  const walk = (suite) => {
    for (const s of suite.suites ?? []) walk(s);
    for (const spec of suite.specs ?? []) {
      for (const t of spec.tests) {
        const last = t.results.at(-1);
        const message = (last?.errors?.[0]?.message ?? '').replace(/\u001b\[[0-9;]*m/g, '');
        const pictures = (last?.attachments ?? []).filter((a) => a.path && /-(expected|actual|diff)\.png$/.test(a.name));
        found.push({ project: t.projectName, title: spec.title, status: last?.status ?? 'skipped', message, pictures });
      }
    }
  };
  for (const s of report.suites ?? []) walk(s);

  const kind = (r) => {
    if (r.status === 'passed') return 'matched';
    if (r.status === 'skipped') return 'skipped';
    if (/A snapshot doesn't exist/.test(r.message)) return 'no picture yet';
    if (r.pictures.length > 0 || /Screenshot comparison failed/.test(r.message)) return 'CHANGED';
    return 'did not reach the screen';
  };
  const notMatched = found.filter((r) => !['matched', 'skipped'].includes(kind(r)));
  text += `${found.length - notMatched.length} of ${found.length} screens matched. This section blocks nothing.\n\n`;
  text += '| Screen | Where | Result |\n|---|---|---|\n';
  for (const r of found) text += `| ${r.title.replace(/\|/g, '\\|')} | ${r.project} | ${kind(r)} |\n`;
  for (const r of notMatched) {
    text += `\n**${r.title}** (${r.project}): ${kind(r)}\n\n`;
    const why = r.message.split('\n').find((l) => l.trim() !== '') ?? '';
    if (why) text += `> ${why.slice(0, 240)}\n\n`;
    for (const a of r.pictures) text += `- ${a.name}: \`${relative(here, a.path)}\` in the artifact \`notes-e2e-screenshots\`\n`;
  }
  if (notMatched.length > 0) {
    text += '\nThe expected, actual and diff pictures are in the artifact `notes-e2e-screenshots`, under `test-results-screenshots/`. ';
    text += 'If the change is wanted, run this workflow again with `update-screenshots` on, then commit the pictures from the artifact (docs/howto-e2e.md).\n';
  }

  if (process.env.E2E_UPDATING_SCREENSHOTS) {
    text += '\n#### Pictures written by this run\n\n';
    text += 'Download the artifact `notes-e2e-screenshots`, copy its `screenshots/` folder over `examples/notes/e2e/screenshots/`, and commit. This run does not commit.\n\n';
    text += '| File | Bytes |\n|---|---|\n';
    const walkDir = (d) => {
      if (!existsSync(d)) return;
      for (const name of readdirSync(d).sort()) {
        const path = `${d}/${name}`;
        if (statSync(path).isDirectory()) walkDir(path);
        else text += `| \`${relative(here, path)}\` | ${statSync(path).size} |\n`;
      }
    };
    walkDir(`${here}screenshots`);
  }
  write(text + '\n');
}
