import { useEffect, useState } from 'react';
import {
  BIND_FIELDS,
  CVAR_FIELDS,
  type CvarField,
  type Settings as QSettings,
  type SkinInfo,
} from './types';
import {
  defaultQuetooSettings,
  getQuetooSettings,
  listSkins,
  readDataImage,
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

/** Quetoo ships 6 crosshair styles as pics/ch1.png … pics/ch6.png. */
const CROSSHAIRS = [1, 2, 3, 4, 5, 6];

/** Resolve a crosshair color cvar to a CSS color. 'default' → white. */
function colorCss(v: string): string {
  if (!v || v === 'default') return '#ffffff';
  if (/^[0-9a-fA-F]{6}$/.test(v)) return `#${v}`;
  return v; // already a CSS color / #-prefixed
}

/** Crosshair section: live preview + visual picker, falling back to the plain
 *  inputs when the game isn't installed (no images to load). */
function CrosshairSection({ fields, cvars, setCvar, installed }: {
  fields: CvarField[];
  cvars: Record<string, string>;
  setCvar: (cvar: string, value: string) => void;
  installed: boolean;
}) {
  const [imgs, setImgs] = useState<Record<number, string>>({});

  useEffect(() => {
    if (!installed) return;
    let cancelled = false;
    (async () => {
      const entries: [number, string][] = [];
      for (const n of CROSSHAIRS) {
        try {
          entries.push([n, await readDataImage(`pics/ch${n}.png`)]);
        } catch {
          /* skip a missing crosshair image */
        }
      }
      if (!cancelled) setImgs(Object.fromEntries(entries));
    })();
    return () => { cancelled = true; };
  }, [installed]);

  const current = parseInt(cvars['cg_draw_crosshair'] ?? '0', 10) || 0;
  const color = colorCss(cvars['cg_draw_crosshair_color'] ?? 'default');
  const scale = parseFloat(cvars['cg_draw_crosshair_scale'] ?? '1') || 1;
  const alphaRaw = parseFloat(cvars['cg_draw_crosshair_alpha'] ?? '1');
  const alpha = isNaN(alphaRaw) ? 1 : alphaRaw;
  const curImg = imgs[current];

  return (
    <section>
      <h3>Crosshair</h3>

      {installed && (
        <>
          <div className="xhair-preview">
            {current !== 0 && curImg ? (
              <span
                className="xhair-mark"
                style={{
                  maskImage: `url(${curImg})`,
                  WebkitMaskImage: `url(${curImg})`,
                  backgroundColor: color,
                  width: `${48 * scale}px`,
                  height: `${48 * scale}px`,
                  opacity: alpha,
                }}
              />
            ) : (
              <span className="xhair-none">No crosshair</span>
            )}
          </div>

          <div className="xhair-grid">
            <button
              type="button"
              className={current === 0 ? 'xhair-opt active' : 'xhair-opt'}
              onClick={() => setCvar('cg_draw_crosshair', '0')}
            >
              Off
            </button>
            {CROSSHAIRS.map((n) => (
              <button
                key={n}
                type="button"
                className={current === n ? 'xhair-opt active' : 'xhair-opt'}
                title={`Crosshair ${n}`}
                onClick={() => setCvar('cg_draw_crosshair', String(n))}
              >
                {imgs[n] ? (
                  <span
                    className="xhair-thumb"
                    style={{
                      maskImage: `url(${imgs[n]})`,
                      WebkitMaskImage: `url(${imgs[n]})`,
                      backgroundColor: color,
                    }}
                  />
                ) : (
                  n
                )}
              </button>
            ))}
          </div>
        </>
      )}

      {fields.map((f) => {
        // The picker above replaces the numeric style field when installed.
        if (f.cvar === 'cg_draw_crosshair' && installed) return null;
        return (
          <label key={f.cvar} className="row">
            <span>
              {f.label}
              {f.hint && <span className="hint"> {f.hint}</span>}
            </span>
            <CvarInput field={f} value={cvars[f.cvar] ?? ''} onChange={(v) => setCvar(f.cvar, v)} />
          </label>
        );
      })}
    </section>
  );
}

/** Skin/model picker: grid of preview icons, falling back to a text field when
 *  nothing is installed (no skins to list). */
function SkinField({ value, onChange }: { value: string; onChange: (v: string) => void }) {
  const [skins, setSkins] = useState<SkinInfo[]>([]);
  const [open, setOpen] = useState(false);

  useEffect(() => {
    listSkins().then(setSkins).catch(() => setSkins([]));
  }, []);

  if (skins.length === 0) {
    return <input value={value} onChange={(e) => onChange(e.target.value)} />;
  }

  return (
    <div className="skin-field">
      <button type="button" className="skin-current" onClick={() => setOpen((o) => !o)}>
        {value || 'select…'} ▾
      </button>
      {open && (
        <div className="skin-grid">
          {skins.map((s) => (
            <button
              key={s.id}
              type="button"
              className={s.id === value ? 'skin-opt active' : 'skin-opt'}
              title={s.id}
              onClick={() => {
                onChange(s.id);
                setOpen(false);
              }}
            >
              {s.icon ? (
                <img src={s.icon} alt={s.id} />
              ) : (
                <span className="skin-noicon">{s.skin}</span>
              )}
              <span className="skin-label">{s.id}</span>
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

interface SettingsProps {
  onBack: () => void;
  installDir: string | null;
  installed: boolean;
  onUninstall: () => void;
}

export default function Settings({ onBack, installDir, installed, onUninstall }: SettingsProps) {
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
      onBack(); // saved — return to the main view
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

      {cvarGroups.map(([section, fields]) =>
        section === 'Crosshair' ? (
          <CrosshairSection
            key={section}
            fields={fields}
            cvars={settings.cvars}
            setCvar={setCvar}
            installed={installed}
          />
        ) : (
          <section key={section}>
            <h3>{section}</h3>
            {fields.map((f) => (
              <label key={f.cvar} className="row">
                <span>
                  {f.label}
                  {f.hint && <span className="hint"> {f.hint}</span>}
                </span>
                {f.cvar === 'skin' && installed ? (
                  <SkinField
                    value={settings.cvars[f.cvar] ?? ''}
                    onChange={(v) => setCvar('skin', v)}
                  />
                ) : (
                  <CvarInput
                    field={f}
                    value={settings.cvars[f.cvar] ?? ''}
                    onChange={(v) => setCvar(f.cvar, v)}
                  />
                )}
              </label>
            ))}
          </section>
        ),
      )}

      <p className="hint">
        Click a binding, then press a key to bind it — Backspace clears, Esc cancels.
      </p>
      {bindGroups.map(([section, fields]) => (
        <section key={section}>
          <h3>{section}</h3>
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

      <section>
        <h3>Danger zone</h3>
        <p className="hint">
          {installed
            ? `Removes the game from ${installDir}. Your personal data is kept unless you choose otherwise.`
            : 'Quetoo is not installed.'}
        </p>
        <button className="danger" disabled={!installed} onClick={onUninstall}>
          Uninstall Quetoo
        </button>
      </section>

      <div className="settings-actions">
        <span className="hint">Settings apply the next time Quetoo starts.</span>
        <button onClick={handleReset}>Reset to defaults</button>
        <button className="primary" onClick={handleSave}>Save</button>
      </div>
      {status && <p className="status">{status}</p>}
    </main>
  );
}
