// A mail catcher for the Ordo bench stack: SMTP in, and `GET /messages?to=`
// to read what came. Every message is kept in memory until the process ends.
//
//   BENCH_MAIL_SMTP_PORT  SMTP port (default 4725)
//   BENCH_MAIL_HTTP_PORT  HTTP port (default 4726)
//
// Taken from examples/notes/e2e/stand-ins.mjs, the mail part only.
import { createServer as createTcp } from 'node:net';
import { createServer as createHttp } from 'node:http';

const smtpPort = Number(process.env.BENCH_MAIL_SMTP_PORT ?? '4725');
const httpPort = Number(process.env.BENCH_MAIL_HTTP_PORT ?? '4726');
const messages = [];

const smtp = createTcp((sock) => {
  sock.setEncoding('utf8');
  let buf = '';
  let inData = false;
  let mail = { from: '', to: [], data: '' };
  const say = (line) => sock.write(`${line}\r\n`);
  say('220 bench mail catcher ESMTP');
  sock.on('data', (chunk) => {
    buf += chunk;
    for (;;) {
      if (inData) {
        const end = buf.indexOf('\r\n.\r\n');
        if (end < 0) return;
        mail.data = buf.slice(0, end).replace(/^\.\./gm, '.');
        buf = buf.slice(end + 5);
        inData = false;
        messages.push({ ...mail, at: Date.now() });
        mail = { from: '', to: [], data: '' };
        say('250 2.0.0 kept');
        continue;
      }
      const nl = buf.indexOf('\r\n');
      if (nl < 0) return;
      const line = buf.slice(0, nl);
      buf = buf.slice(nl + 2);
      const verb = line.slice(0, 4).toUpperCase();
      if (verb === 'EHLO') sock.write('250-bench\r\n250-8BITMIME\r\n250 SMTPUTF8\r\n');
      else if (verb === 'HELO') say('250 bench');
      else if (verb === 'MAIL') {
        mail.from = (/<([^>]*)>/.exec(line) ?? [])[1] ?? '';
        say('250 2.1.0 ok');
      } else if (verb === 'RCPT') {
        mail.to.push(((/<([^>]*)>/.exec(line) ?? [])[1] ?? '').toLowerCase());
        say('250 2.1.5 ok');
      } else if (verb === 'DATA') {
        inData = true;
        say('354 end with <CRLF>.<CRLF>');
        // The body starts on the next line. A leading CRLF lets a body that
        // is only the end marker still match it.
        buf = `\r\n${buf}`;
      } else if (verb === 'RSET') {
        mail = { from: '', to: [], data: '' };
        say('250 ok');
      } else if (verb === 'NOOP') say('250 ok');
      else if (verb === 'QUIT') {
        say('221 bye');
        sock.end();
        return;
      } else say('502 5.5.2 not here');
    }
  });
  sock.on('error', () => {});
});

const http = createHttp((req, res) => {
  const url = new URL(req.url ?? '/', 'http://localhost');
  const send = (code, body) => {
    res.writeHead(code, { 'content-type': 'application/json' });
    res.end(JSON.stringify(body));
  };
  if (req.method === 'GET' && url.pathname === '/messages') {
    const to = url.searchParams.get('to')?.toLowerCase();
    return send(200, to ? messages.filter((m) => m.to.includes(to)) : messages);
  }
  if (req.method === 'GET' && url.pathname === '/health') return send(200, { ok: true });
  send(404, { error: 'not_found' });
});

smtp.listen(smtpPort);
http.listen(httpPort);
for (const sig of ['SIGINT', 'SIGTERM']) process.on(sig, () => process.exit(0));
