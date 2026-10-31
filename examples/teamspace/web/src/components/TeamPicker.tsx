import type { ListedSpace } from '@hominemai/ordo-sdk/auth/share.js';

interface Props {
  readonly teams: readonly ListedSpace[];
  readonly onPick: (space: string) => void;
}

/** A person in more than one team chooses which to work in. */
export function TeamPicker({ teams, onPick }: Props) {
  return (
    <section className="card" aria-labelledby="team-title">
      <h2 id="team-title">Choose a team</h2>
      <p className="hint">You are in more than one. You can switch later.</p>
      <ul className="plain">
        {teams.map((t) => (
          <li key={t.space}>
            <button onClick={() => onPick(t.space)}>
              {t.name ?? t.space}
              {t.role !== null && <span className="hint"> ({t.role})</span>}
            </button>
          </li>
        ))}
      </ul>
    </section>
  );
}
