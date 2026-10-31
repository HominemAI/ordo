// Process groups the runner started (the agent, the stack), so a stopped
// runner stops them too instead of leaving a stack holding the ports.

const groups = new Set();

export const track = (pid) => groups.add(pid);
export const untrack = (pid) => groups.delete(pid);

/** Stop every tracked group: SIGINT, then SIGKILL after `graceMs`. */
export async function stopAll(graceMs = 15_000) {
  for (const pid of groups) {
    try {
      process.kill(-pid, 'SIGINT');
    } catch {}
  }
  const until = Date.now() + graceMs;
  const alive = (pid) => {
    try {
      process.kill(-pid, 0);
      return true;
    } catch {
      return false;
    }
  };
  while ([...groups].some(alive) && Date.now() < until) await new Promise((r) => setTimeout(r, 250));
  for (const pid of groups) {
    try {
      process.kill(-pid, 'SIGKILL');
    } catch {}
  }
  groups.clear();
}
