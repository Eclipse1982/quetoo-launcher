import { useEffect, useState } from 'react';
import {
  BIND_FIELDS,
  CVAR_FIELDS,
  type CvarField,
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
  return e.key.toLowerCase();
}

function groupBy<T>(items: T[], key: (t: T) => string): [string, T[]][] {
  const m = new Map<string, T[]>();
  for (const it of items) {
    const k = key(it);
    if (!m.has(k)) m.set(k, []);
    m.get(k)!.push(it);
  }
  return [...m.entries()];
}

function CvarInput({ field, value, onChange }: {
  field: CvarField; value: string; onChange: (v: string) => void;
}) {
  const f = field.field;
  switch (f.kind) {
    case 'checkbox':
      return (
        <input type="checkbox" checked={(parseFloat(value) || 0) !== 0}
          onChange={(e) => onChange(e.target.checked ? '1' : '0')} />
      );
    case 'slider':
      return (
        <span className="slider-wrap">
          <input type="range" min={f.min} max={f.max} step={f.step}
            value={Number(value) || 0}
            onChange={(e) => onChange(e.target.value)} />
          <span className="slider-value">{value}</span>
        </span>
      );
    case 'select':
      return (
        <select value={value} onChange={(e) => onChange(e.target.value)}>
          {f.options.map((o) => (
            <option key={o.value} value={o.value}>{o.label}</option>
          ))}
        </select>
      );
    case 'number':
      return (
        <input type="number" min={f.min} max={f.max} step={f.step ?? 1}
          value={value} onChange={(e) => onChange(e.target.value)} />
      );
    default:
      return <input value={value} onChange={(e) => onChange(e.target.value)} />;
  }
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

    const finish = (key: string) => {
      setSettings((s) =>
        s ? { ...s, bindings: { ...s.bindings, [capturing]: key } } : s,
      );
      setCapturing(null);
    };

    const onKey = (e: KeyboardEvent) => {
      e.preventDefault();
      // Backspace/Delete clears the bind; Escape cancels without clearing.
      if (e.key === 'Backspace' || e.key === 'Delete') {
        finish('');
      } else if (e.key === 'Escape') {
        setCapturing(null);
      } else {
        finish(eventToKey(e));
      }
    };
    const onMouse = (e: MouseEvent) => {
      e.preventDefault();
      finish(`mouse ${e.button + 1}`);
    };
    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      if (e.deltaY === 0) return;
      // exact Quetoo key names
      finish(e.deltaY > 0 ? 'mouse wheel down' : 'mouse wheel up');
    };
    window.addEventListener('keydown', onKey);
    window.addEventListener('mousedown', onMouse);
    window.addEventListener('wheel', onWheel, { passive: false });
    return () => {
      window.removeEventListener('keydown', onKey);
      window.removeEventListener('mousedown', onMouse);
      window.removeEventListener('wheel', onWheel);
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

  const cvarGroups = groupBy(CVAR_FIELDS, (f) => f.section);
  const bindGroups = groupBy(BIND_FIELDS, (f) => f.section);

  return (
    <main className="app settings">
      <div className="settings-head">
        <button onClick={onBack}>← Back</button>
        <h2>Settings</h2>
      </div>

      {cvarGroups.map(([section, fields]) => (
        <section key={section}>
          <h3>{section}</h3>
          {fields.map((f) => (
            <label key={f.cvar} className="row">
              <span>
                {f.label}
                {f.hint && <span className="hint"> {f.hint}</span>}
              </span>
              <CvarInput
                field={f}
                value={settings.cvars[f.cvar] ?? ''}
                onChange={(v) => setCvar(f.cvar, v)}
              />
            </label>
          ))}
        </section>
      ))}

      {bindGroups.map(([section, fields]) => (
        <section key={section}>
          <h3>{section} <span className="hint">click a key, then press a key to bind — Backspace clears, Esc cancels</span></h3>
          {fields.map(({ command, label }) => {
            const boundKey = settings.bindings[command] ?? '';
            return (
              <div key={command} className="row">
                <span>{label}</span>
                <button
                  className={capturing === command ? 'key capturing' : 'key'}
                  title={boundKey === '' ? 'unbound' : boundKey}
                  onClick={() => setCapturing(command)}
                >
                  {capturing === command ? 'press a key…' : (boundKey || '—')}
                </button>
              </div>
            );
          })}
        </section>
      ))}

      <div className="settings-actions">
        <button onClick={handleReset}>Reset to defaults</button>
        <button className="primary" onClick={handleSave}>Save</button>
      </div>
      {status && <p className="status">{status}</p>}
    </main>
  );
}
