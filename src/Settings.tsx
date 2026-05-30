import { useEffect, useState } from 'react';
import {
  BIND_FIELDS,
  CVAR_FIELDS,
  type Settings as QSettings,
} from './types';
import {
  defaultQuetooSettings,
  getQuetooSettings,
  saveQuetooSettings,
} from './api';

/** Map a keyboard/mouse event to a Quetoo key name. */
function eventToKey(e: KeyboardEvent): string {
  const map: Record<string, string> = {
    ' ': 'space',
    Control: 'left ctrl',
    Shift: 'left shift',
    Alt: 'left alt',
    ArrowUp: 'up',
    ArrowDown: 'down',
    ArrowLeft: 'left',
    ArrowRight: 'right',
    Enter: 'return',
    Tab: 'tab',
    Escape: 'escape',
  };
  if (e.key in map) return map[e.key];
  return e.key.length === 1 ? e.key.toLowerCase() : e.key.toLowerCase();
}

export default function Settings({ onBack }: { onBack: () => void }) {
  const [settings, setSettings] = useState<QSettings | null>(null);
  const [status, setStatus] = useState('');
  const [capturing, setCapturing] = useState<string | null>(null); // command being rebound

  useEffect(() => {
    getQuetooSettings().then(setSettings).catch((e) => setStatus(String(e)));
  }, []);

  useEffect(() => {
    if (!capturing) return;
    const onKey = (e: KeyboardEvent) => {
      e.preventDefault();
      // Escape cancels the rebind instead of binding the Escape key.
      if (e.key !== 'Escape') {
        setSettings((s) =>
          s ? { ...s, bindings: { ...s.bindings, [capturing]: eventToKey(e) } } : s,
        );
      }
      setCapturing(null);
    };
    const onMouse = (e: MouseEvent) => {
      e.preventDefault();
      const key = `mouse ${e.button + 1}`;
      setSettings((s) =>
        s ? { ...s, bindings: { ...s.bindings, [capturing]: key } } : s,
      );
      setCapturing(null);
    };
    window.addEventListener('keydown', onKey);
    window.addEventListener('mousedown', onMouse);
    return () => {
      window.removeEventListener('keydown', onKey);
      window.removeEventListener('mousedown', onMouse);
    };
  }, [capturing]);

  if (!settings) {
    return (
      <main className="app">
        <p className="status">{status || 'Loading settings…'}</p>
        <button onClick={onBack}>Back</button>
      </main>
    );
  }

  const setCvar = (cvar: string, value: string) =>
    setSettings({ ...settings, cvars: { ...settings.cvars, [cvar]: value } });

  async function handleSave() {
    try {
      await saveQuetooSettings(settings!);
      setStatus('Saved to autoexec.cfg');
    } catch (e) {
      setStatus(String(e));
    }
  }

  async function handleReset() {
    try {
      setSettings(await defaultQuetooSettings());
      setStatus('Reset to defaults (not yet saved)');
    } catch (e) {
      setStatus(String(e));
    }
  }

  return (
    <main className="app settings">
      <div className="settings-head">
        <button onClick={onBack}>← Back</button>
        <h2>Settings</h2>
      </div>

      <section>
        <h3>Game</h3>
        {CVAR_FIELDS.map(({ cvar, label }) => (
          <label key={cvar} className="row">
            <span>{label}</span>
            <input
              value={settings.cvars[cvar] ?? ''}
              onChange={(e) => setCvar(cvar, e.target.value)}
            />
          </label>
        ))}
      </section>

      <section>
        <h3>Controls</h3>
        {BIND_FIELDS.map(({ command, label }) => (
          <div key={command} className="row">
            <span>{label}</span>
            <button
              className={capturing === command ? 'key capturing' : 'key'}
              onClick={() => setCapturing(command)}
            >
              {capturing === command ? 'press a key…' : settings.bindings[command] ?? '—'}
            </button>
          </div>
        ))}
      </section>

      <div className="settings-actions">
        <button onClick={handleReset}>Reset to defaults</button>
        <button className="primary" onClick={handleSave}>Save</button>
      </div>
      {status && <p className="status">{status}</p>}
    </main>
  );
}
