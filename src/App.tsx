import { useEffect, useState } from 'react';
import {
  chooseInstallDir,
  getStatus,
  installOrUpdate,
  onDownloadProgress,
  play,
  setInstallDir,
} from './api';
import type { Status } from './types';
import './styles.css';

type Phase = 'loading' | 'idle' | 'working' | 'error';

export default function App() {
  const [status, setStatus] = useState<Status | null>(null);
  const [phase, setPhase] = useState<Phase>('loading');
  const [message, setMessage] = useState<string>('Checking for updates…');
  const [percent, setPercent] = useState<number>(0);

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

  return (
    <main className="app">
      <h1>Quetoo Launcher</h1>

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
