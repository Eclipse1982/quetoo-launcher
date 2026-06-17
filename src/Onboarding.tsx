import { useState } from 'react';

interface OnboardingProps {
  defaultDir: string | null;
  initialDir: string | null;
  chooseDir: () => Promise<string | null>;
  /** Set install dir + player name, mark onboarded, and start the install. */
  onComplete: (name: string, dir: string) => void;
  /** Mark onboarded without installing. */
  onSkip: () => void;
}

export default function Onboarding({
  defaultDir,
  initialDir,
  chooseDir,
  onComplete,
  onSkip,
}: OnboardingProps) {
  const [name, setName] = useState('');
  const [dir, setDir] = useState<string | null>(initialDir ?? defaultDir);
  const [error, setError] = useState('');

  async function pick() {
    const d = await chooseDir();
    if (d) {
      setDir(d);
      setError('');
    }
  }

  function install() {
    if (!dir) {
      setError('Choose an install folder first.');
      return;
    }
    onComplete(name.trim(), dir);
  }

  return (
    <main className="app onboarding">
      <img src="/quetoo-logo.png" alt="Quetoo" className="logo" />
      <h1>Welcome to Quetoo</h1>
      <p className="status">
        Let's get you set up. The first install downloads the full game (~1&nbsp;GB);
        after that, updates are small.
      </p>

      <label className="onb-field">
        <span>Player name</span>
        <input
          value={name}
          onChange={(e) => setName(e.target.value)}
          placeholder="Pick a name (optional)"
          maxLength={32}
          autoFocus
        />
      </label>

      <label className="onb-field">
        <span>Install folder</span>
        <div className="onb-dir">
          <code>{dir ?? 'not set'}</code>
          <button onClick={pick}>Change…</button>
        </div>
      </label>

      {error && <p className="status error">{error}</p>}

      <button className="primary play" onClick={install} disabled={!dir}>
        Install Quetoo
      </button>
      <button className="onb-skip" onClick={onSkip}>
        Skip for now
      </button>
    </main>
  );
}
