import { useEffect, useState } from 'react';
import {
  checkLauncherUpdate,
  chooseInstallDir,
  confirmDialog,
  getStatus,
  installOrUpdate,
  joinServer,
  onInstallProgress,
  play,
  reinstall,
  rollbackUpdate,
  setChannel,
  setInstallDir,
  syncData,
  uninstall,
  type LauncherUpdate,
} from './api';
import type { Channel, InstallPhase, Status } from './types';
import Settings from './Settings';
import ServerBrowser from './ServerBrowser';
import './styles.css';

type Phase = 'loading' | 'idle' | 'working' | 'error';

const PHASE_LABELS: Record<InstallPhase, string> = {
  download: 'Downloading',
  snapshot: 'Backing up current version',
  extract: 'Installing files',
  verify: 'Verifying',
  data: 'Updating game data',
};

export default function App() {
  const [status, setStatus] = useState<Status | null>(null);
  const [phase, setPhase] = useState<Phase>('loading');
  const [message, setMessage] = useState<string>('Checking for updates…');
  const [percent, setPercent] = useState<number>(0);
  const [detail, setDetail] = useState<string>('');
  const [view, setView] = useState<'launcher' | 'settings' | 'servers'>('launcher');
  const [launcherUpdate, setLauncherUpdate] = useState<LauncherUpdate | null>(null);
  const [updating, setUpdating] = useState(false);

  async function refresh() {
    setPhase('loading');
    setMessage('Checking for updates…');
    try {
      const s = await getStatus();
      setStatus(s);
      // When the engine is current, keep game data current before showing Play.
      // Offline-safe: sync_data resolves (skipped) rather than throwing, so the
      // user can still play with existing data.
      if (s.state.state === 'upToDate') {
        setPhase('working');
        setMessage('Updating game data…');
        setPercent(0);
        setDetail('');
        try {
          await syncData(false);
        } catch {
          // ignore: allow Play with whatever data is present
        }
      }
      setPhase('idle');
    } catch (e) {
      setMessage(String(e));
      setPhase('error');
    }
  }

  useEffect(() => {
    refresh();
    checkLauncherUpdate().then(setLauncherUpdate);
    const un = onInstallProgress((p) => {
      setMessage(PHASE_LABELS[p.phase] ?? p.phase);
      setPercent(p.percent);
      setDetail(p.detail);
    });
    return () => {
      un.then((fn) => fn());
    };
  }, []);

  async function run(op: () => Promise<void>, startMessage: string) {
    setPhase('working');
    setPercent(0);
    setDetail('');
    setMessage(startMessage);
    try {
      await op();
      await refresh();
    } catch (e) {
      setMessage(String(e));
      setPhase('error');
    }
  }

  async function handleChooseDir() {
    const dir = await chooseInstallDir();
    if (!dir) return;
    await setInstallDir(dir);
    await refresh();
  }

  async function handleSetChannel(channel: Channel) {
    if (!status || status.channel === channel) return;
    setPhase('loading');
    setMessage(channel === 'preRelease' ? 'Switching to Pre-Release…' : 'Switching to Stable…');
    try {
      setStatus(await setChannel(channel));
      setPhase('idle');
    } catch (e) {
      setMessage(String(e));
      setPhase('error');
    }
  }

  async function handleInstall() {
    if (!status) return;
    const dir = status.installDir ?? status.defaultInstallDir;
    if (!dir) {
      setMessage('No default install folder for this platform; choose one.');
      setPhase('error');
      return;
    }
    // Set the dir inside run() so it's covered by the error view and the
    // working phase blocks a second click during the IPC round-trip.
    await run(async () => {
      if (!status.installDir) await setInstallDir(dir);
      await installOrUpdate();
    }, 'Starting…');
  }

  async function handleUninstall() {
    if (!status?.installDir) return;
    const ok = await confirmDialog(
      `This deletes Quetoo from ${status.installDir}. Continue?`,
      'Uninstall Quetoo',
    );
    if (!ok) return;
    const purge = await confirmDialog(
      'Also delete your settings, screenshots, demos and downloaded maps? ' +
        'These are shared by ALL Quetoo versions and mods on this machine.',
      'Delete personal data?',
    );
    setView('launcher');
    await run(() => uninstall(purge), 'Uninstalling…');
  }

  async function handleReinstall() {
    if (!status?.installDir) return;
    const ok = await confirmDialog(
      `This deletes everything in ${status.installDir} and re-downloads the full game (~1 GB). Continue?`,
      'Reinstall Quetoo',
    );
    if (!ok) return;
    await run(reinstall, 'Reinstalling…');
  }

  async function handleRollback() {
    const ok = await confirmDialog(
      'Restore the previous version from the local backup?',
      'Roll back update',
    );
    if (!ok) return;
    await run(rollbackUpdate, 'Rolling back…');
  }

  async function handleJoin(addr: string) {
    if (installed) {
      try {
        await joinServer(addr);
      } catch (e) {
        setMessage(String(e));
        setPhase('error');
        setView('launcher');
      }
    } else {
      setView('launcher');
      await handleInstall();
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

  const installed =
    status?.state.state === 'upToDate' || status?.state.state === 'updateAvailable';

  if (view === 'settings') {
    return (
      <Settings
        onBack={() => setView('launcher')}
        installDir={status?.installDir ?? null}
        installed={installed}
        onUninstall={handleUninstall}
      />
    );
  }

  if (view === 'servers') {
    return (
      <ServerBrowser
        onBack={() => setView('launcher')}
        installed={installed}
        onJoin={handleJoin}
      />
    );
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
          {/* Both nav buttons disabled while an operation runs: Settings hosts
              Uninstall and the browser hosts Join — neither may start
              concurrently with an in-flight install. */}
          <button
            className="gear"
            disabled={phase === 'working'}
            onClick={() => setView('servers')}
            title="Server Browser"
          >
            Servers
          </button>
          <button
            className="gear"
            disabled={phase === 'working'}
            onClick={() => setView('settings')}
            title="Settings"
          >
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
          <p className="status">
            {percent}%{detail ? ` — ${detail}` : ''}
          </p>
        </>
      )}

      {phase === 'idle' && status && (
        <>
          <div className="channel-toggle" role="group" aria-label="Release channel">
            <button
              className={status.channel === 'stable' ? 'active' : ''}
              disabled={status.channel === 'stable'}
              onClick={() => handleSetChannel('stable')}
            >
              Stable
            </button>
            <button
              className={status.channel === 'preRelease' ? 'active' : ''}
              disabled={status.channel === 'preRelease' || !status.preReleaseAvailable}
              onClick={() => handleSetChannel('preRelease')}
              title={
                status.preReleaseAvailable
                  ? 'Latest snapshot build from the main branch'
                  : 'Pre-Release builds are not available for your platform'
              }
            >
              Pre-Release
            </button>
          </div>
          {!status.preReleaseAvailable && (
            <p className="status channel-note">
              Pre-Release builds aren’t available for your platform.
            </p>
          )}

          <p className="status">
            Install folder:{' '}
            {status.installDir ??
              (status.defaultInstallDir ? (
                <em>{status.defaultInstallDir} (default)</em>
              ) : (
                <em>not set</em>
              ))}
          </p>
          <button onClick={handleChooseDir}>
            {status.installDir
              ? 'Change install folder'
              : status.defaultInstallDir
                ? 'Choose a different folder'
                : 'Choose install folder'}
          </button>

          {status.state.state === 'notInstalled' ? (
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

          {installed && (
            <div className="btn-row">
              {status.canRollback && (
                <button onClick={handleRollback}>↩ Roll back update</button>
              )}
              <button
                onClick={() =>
                  run(() => syncData(true).then(() => undefined), 'Verifying game data…')
                }
              >
                Verify data
              </button>
              <button onClick={handleReinstall}>Reinstall</button>
            </div>
          )}
        </>
      )}
    </main>
  );
}
