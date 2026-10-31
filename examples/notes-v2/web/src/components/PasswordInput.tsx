import { useState } from 'react';

interface Props {
  readonly id: string;
  readonly label: string;
  readonly value: string;
  readonly onChange: (v: string) => void;
  /** `new-password` where a password is chosen, `current-password` where one is typed to get in. */
  readonly autoComplete: 'new-password' | 'current-password';
  readonly describedBy?: string;
  readonly required?: boolean;
  readonly disabled?: boolean;
  readonly invalid?: boolean;
}

/** A password field with a button that shows what was typed. Pasting works. */
export function PasswordInput({ id, label, value, onChange, autoComplete, describedBy, required, disabled, invalid }: Props) {
  const [shown, setShown] = useState(false);
  return (
    <>
      <label htmlFor={id}>{label}</label>
      <div className="row">
        <input
          id={id}
          type={shown ? 'text' : 'password'}
          value={value}
          onChange={(e) => onChange(e.target.value)}
          autoComplete={autoComplete}
          autoCorrect="off"
          autoCapitalize="off"
          spellCheck={false}
          maxLength={256}
          required={required}
          disabled={disabled}
          aria-invalid={invalid || undefined}
          aria-describedby={describedBy}
        />
        <button type="button" aria-pressed={shown} onClick={() => setShown((s) => !s)} disabled={disabled}>
          {shown ? 'Hide' : 'Show'} <span className="visually-hidden">password</span>
        </button>
      </div>
    </>
  );
}
