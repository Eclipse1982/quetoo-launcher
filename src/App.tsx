import { useEffect, useState } from 'react';
import {
  checkLauncherUpdate,
  chooseInstallDir,
  getStatus,
  installOrUpdate,
  onDownloadProgress,
  play,
  setInstallDir,
  type LauncherUpdate,
} from './api';
import type { Status } from './types';
import Settings from './Settings';
import './styles.css';

type Phase = 'loading' | 'idle' | 'working' | 'error';

export default function App() {
  const [status, setStatus] = useState<Status | null>(null);
  const [phase, setPhase] = useState<Phase>('loading');
  const [message, setMessage] = useState<string>('Checking for updates…');
  const [percent, setPercent] = useState<number>(0);
  const [view, setView] = useState<'launcher' | 'settings'>('launcher');
  const [launcherUpdate, setLauncherUpdate] = useState<LauncherUpdate | null>(null);
  const [updating, setUpdating] = useState(false);

  async function refresh() {
    setPhase('loading');
    setMessage('Checking for updates…');
    try {
      setStatus(await getStatus());
      setPhase('idle');
    } catch (e) {
      setMessage(String(e));
      setPhase('error');
    }
  }

  useEffect(() => {
    refresh();
    checkLauncherUpdate().then(setLauncherUpdate);
    const un = onDownloadProgress((p) => {
      setPercent(p.total > 0 ? Math.round((p.downloaded / p.total) * 100) : 0);
    });
    return () => {
      un.then((fn) => fn());
    };
  }, []);

  async function handleChooseDir() {
    const dir = await chooseInstallDir();
    if (!dir) return;
    await setInstallDir(dir);
    await refresh();
  }

  async function handleInstall() {
    setPhase('working');
    setPercent(0);
    setMessage('Downloading…');
    try {
      await installOrUpdate();
      await refresh();
    } catch (e) {
      setMessage(String(e));
      setPhase('error');
    }
  }

  async function handlePlay() {
    try {
      await play();
    } catch (e) {
      setMessage(String(e));
      setPhase('error');
    }
  }

  if (view === 'settings') {
    return <Settings onBack={() => setView('launcher')} />;
  }

  return (
    <main className="app">
        {launcherUpdate && (
          <div className="update-banner">
            <span>Launcher {launcherUpdate.version} available</span>
            <button
              className="primary"
              disabled={updating}
              onClick={async () => {
                setUpdating(true);
                try {
                  await launcherUpdate.apply();
                } catch (e) {
                  setUpdating(false);
                  setMessage(String(e));
                  setPhase('error');
                }
              }}
            >
              {updating ? 'Updating…' : 'Update & restart'}
            </button>
          </div>
        )}
        <header className="brand">
          <img src="/quetoo-logo.png" alt="Quetoo" className="logo" />
          <button className="gear" onClick={() => setView('settings')} title="Settings">
            ⚙ Settings
          </button>
        </header>

      {phase === 'loading' && <p className="status">{message}</p>}

      {phase === 'error' && (
        <>
          <p className="status error">{message}</p>
          <button onClick={refresh}>Retry</button>
        </>
      )}

      {phase === 'working' && (
        <>
          <p className="status">{message}</p>
          <div className="bar">
            <div className="bar-fill" style={{ width: `${percent}%` }} />
          </div>
          <p className="status">{percent}%</p>
        </>
      )}

      {phase === 'idle' && status && (
        <>
          <p className="status">
            Install folder: {status.installDir ?? <em>not set</em>}
          </p>
          <button onClick={handleChooseDir}>
            {status.installDir ? 'Change install folder' : 'Choose install folder'}
          </button>

          {!status.installDir ? (
            <p className="status">Choose a folder to install Quetoo.</p>
          ) : status.state.state === 'notInstalled' ? (
            <button className="primary" onClick={handleInstall}>
              Install {status.latestVersion}
            </button>
          ) : status.state.state === 'updateAvailable' ? (
            <button className="primary" onClick={handleInstall}>
              Update {status.state.from} → {status.state.to}
            </button>
          ) : (
            <button className="primary play" onClick={handlePlay}>
              Play
            </button>
          )}
        </>
      )}
    </main>
  );
}
