import { Fragment, useEffect, useRef, useState } from 'react';
import { addFavorite, getServers, removeFavorite } from './api';
import type { ServerInfo } from './types';

const QUETOO_PROTOCOL = 2027;

type SortCol = 'hostname' | 'map' | 'gameplay' | 'players' | 'ping';
type SortDir = 'asc' | 'desc';

interface ServerBrowserProps {
  onBack: () => void;
  installed: boolean;
  onJoin: (addr: string) => void;
}

function cmp<T>(a: T, b: T): number {
  if (a < b) return -1;
  if (a > b) return 1;
  return 0;
}

function sortServers(servers: ServerInfo[], col: SortCol, dir: SortDir): ServerInfo[] {
  const sorted = [...servers].sort((a, b) => {
    // favorites always pinned above non-favorites within any sort
    if (a.favorite !== b.favorite) return a.favorite ? -1 : 1;
    let v = 0;
    switch (col) {
      case 'hostname': v = cmp(a.hostname.toLowerCase(), b.hostname.toLowerCase()); break;
      case 'map':      v = cmp(a.map.toLowerCase(), b.map.toLowerCase()); break;
      case 'gameplay': v = cmp(a.gameplay.toLowerCase(), b.gameplay.toLowerCase()); break;
      case 'players':  v = cmp(a.clients, b.clients); break;
      case 'ping':     v = cmp(a.ping, b.ping); break;
    }
    return dir === 'asc' ? v : -v;
  });
  return sorted;
}

export default function ServerBrowser({ onBack, installed, onJoin }: ServerBrowserProps) {
  const [servers, setServers] = useState<ServerInfo[]>([]);
  const [masterOk, setMasterOk] = useState(true);
  const [loading, setLoading] = useState(false);
  const [sortCol, setSortCol] = useState<SortCol>('ping');
  const [sortDir, setSortDir] = useState<SortDir>('asc');
  const [hideEmpty, setHideEmpty] = useState(false);
  const [hideFull, setHideFull] = useState(false);
  const [search, setSearch] = useState('');
  const [ipInput, setIpInput] = useState('');
  const [expandedAddr, setExpandedAddr] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const inflightRef = useRef<Promise<void> | null>(null);

  function refresh(): Promise<void> {
    if (inflightRef.current) return inflightRef.current;
    const p = (async () => {
      setLoading(true);
      try {
        const result = await getServers();
        setServers(result.servers);
        setMasterOk(result.masterOk);
      } catch {
        // leave existing list; errors surfaced via masterOk banner or empty state
      } finally {
        setLoading(false);
        inflightRef.current = null;
      }
    })();
    inflightRef.current = p;
    return p;
  }

  useEffect(() => {
    refresh();
    const id = setInterval(() => { refresh(); }, 10_000);
    return () => clearInterval(id);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  function handleSort(col: SortCol) {
    if (col === sortCol) {
      setSortDir((d) => (d === 'asc' ? 'desc' : 'asc'));
    } else {
      setSortCol(col);
      setSortDir('asc');
    }
  }

  async function refreshAfterMutation() {
    // If a refresh is already in flight, wait for it to finish first,
    // then kick off a fresh one so callers see up-to-date data.
    if (inflightRef.current) await inflightRef.current;
    await refresh();
  }

  async function handleAddFavorite() {
    const addr = ipInput.trim();
    if (!addr) return;
    setError(null);
    try {
      await addFavorite(addr);
      setIpInput('');
      await refreshAfterMutation();
    } catch (e) {
      setError(String(e));
      // intentionally keep ipInput so the user can correct it
    }
  }

  async function handleToggleFavorite(e: React.MouseEvent, server: ServerInfo) {
    e.stopPropagation();
    setError(null);
    try {
      if (server.favorite) {
        await removeFavorite(server.addr);
      } else {
        await addFavorite(server.addr);
      }
      await refreshAfterMutation();
    } catch (err) {
      setError(String(err));
    }
  }

  function handleRowClick(addr: string) {
    setExpandedAddr((prev) => (prev === addr ? null : addr));
  }

  const needle = search.toLowerCase();
  const filtered = servers.filter((s) => {
    if (hideEmpty && s.clients === 0 && s.bots === 0) return false;
    if (hideFull && s.clients >= s.maxClients) return false;
    if (needle) {
      const matchHost = s.hostname.toLowerCase().includes(needle);
      const matchMap = s.map.toLowerCase().includes(needle);
      if (!matchHost && !matchMap) return false;
    }
    return true;
  });

  const displayed = sortServers(filtered, sortCol, sortDir);

  function sortIndicator(col: SortCol) {
    if (col !== sortCol) return null;
    return <span className="sort-arrow">{sortDir === 'asc' ? ' ▲' : ' ▼'}</span>;
  }

  function isProtocolMismatch(s: ServerInfo) {
    return s.protocol !== QUETOO_PROTOCOL && s.protocol !== 0;
  }

  function isDead(s: ServerInfo) {
    return s.hostname === '(no response)';
  }

  return (
    <main className="app sb-view">
      <div className="sb-header">
        <button onClick={onBack}>← Back</button>
        <h2>Servers</h2>
        <button onClick={() => { setError(null); refresh(); }} disabled={loading}>{loading ? 'Refreshing…' : 'Refresh'}</button>
      </div>

      {!masterOk && (
        <div className="sb-banner">Master server unreachable — showing favorites only</div>
      )}

      <div className="sb-toolbar">
        <div className="sb-add-ip">
          <input
            type="text"
            placeholder="ip:port"
            value={ipInput}
            onChange={(e) => setIpInput(e.target.value)}
            onKeyDown={(e) => { if (e.key === 'Enter') handleAddFavorite(); }}
          />
          <button onClick={handleAddFavorite} disabled={!ipInput.trim()}>Add Favorite</button>
        </div>
        <label className="sb-check">
          <input type="checkbox" checked={hideEmpty} onChange={(e) => setHideEmpty(e.target.checked)} />
          Hide empty
        </label>
        <label className="sb-check">
          <input type="checkbox" checked={hideFull} onChange={(e) => setHideFull(e.target.checked)} />
          Hide full
        </label>
        <input
          className="sb-search"
          type="text"
          placeholder="Search hostname or map…"
          value={search}
          onChange={(e) => setSearch(e.target.value)}
        />
      </div>

      {error && <div className="sb-banner error">{error}</div>}

      {loading && servers.length === 0 && <p className="status">Refreshing…</p>}
      {!loading && displayed.length === 0 && <p className="status">No servers found.</p>}

      {displayed.length > 0 && (
        <div className="sb-table-wrap">
          <table className="sb-table">
            <thead>
              <tr>
                <th className="col-star" title="Favorite">★</th>
                <th className="col-server" onClick={() => handleSort('hostname')}>
                  Server{sortIndicator('hostname')}
                </th>
                <th className="col-map" onClick={() => handleSort('map')}>
                  Map{sortIndicator('map')}
                </th>
                <th className="col-mode" onClick={() => handleSort('gameplay')}>
                  Mode{sortIndicator('gameplay')}
                </th>
                <th className="col-players" onClick={() => handleSort('players')}>
                  Players{sortIndicator('players')}
                </th>
                <th className="col-ping" onClick={() => handleSort('ping')}>
                  Ping{sortIndicator('ping')}
                </th>
                <th className="col-join"></th>
              </tr>
            </thead>
            <tbody>
              {displayed.map((s) => {
                const mismatch = isProtocolMismatch(s);
                const dead = isDead(s);
                const dim = mismatch || dead;
                const expanded = expandedAddr === s.addr;
                const joinDisabled = dead || mismatch;

                let joinTitle: string | undefined;
                if (mismatch) joinTitle = 'different game version';
                else if (dead) joinTitle = 'Server is not responding';
                else if (!installed) joinTitle = undefined;

                const joinLabel = !installed ? 'Install to join' : 'Join';

                const playersCell = dead
                  ? '—/—'
                  : `${s.clients}/${s.maxClients}${s.bots > 0 ? ` +${s.bots} bots` : ''}`;

                const pingCell = dead ? '—' : String(s.ping);

                return (
                  <Fragment key={s.addr}>
                    <tr
                      className={dim ? 'dim' : ''}
                      onClick={() => handleRowClick(s.addr)}
                    >
                      <td className="col-star">
                        <button
                          className={`sb-star${s.favorite ? ' active' : ''}`}
                          onClick={(e) => handleToggleFavorite(e, s)}
                          title={s.favorite ? 'Remove favorite' : 'Add favorite'}
                        >
                          {s.favorite ? '★' : '☆'}
                        </button>
                      </td>
                      <td className="col-server">
                        {s.hostname}
                        {mismatch && <span className="sb-tag">v{s.protocol}</span>}
                      </td>
                      <td className="col-map">{s.map}</td>
                      <td className="col-mode">{s.gameplay}</td>
                      <td className="col-players">{playersCell}</td>
                      <td className="col-ping">{pingCell}</td>
                      <td className="col-join">
                        <button
                          className="primary sb-join"
                          disabled={joinDisabled}
                          title={joinTitle}
                          onClick={(e) => { e.stopPropagation(); onJoin(s.addr); }}
                        >
                          {joinLabel}
                        </button>
                      </td>
                    </tr>
                    {expanded && (
                      <tr className="sb-players-row">
                        <td colSpan={7}>
                          {s.players.length === 0 ? (
                            <em className="sb-no-players">No players</em>
                          ) : (
                            <table className="sb-players">
                              <thead>
                                <tr>
                                  <th>Name</th>
                                  <th>Score</th>
                                  <th>Ping</th>
                                </tr>
                              </thead>
                              <tbody>
                                {s.players.map((p, i) => (
                                  <tr key={i}>
                                    <td>{p.name || <em>unnamed</em>}{p.bot ? ' (bot)' : ''}</td>
                                    <td>{p.score}</td>
                                    <td>{p.ping}</td>
                                  </tr>
                                ))}
                              </tbody>
                            </table>
                          )}
                        </td>
                      </tr>
                    )}
                  </Fragment>
                );
              })}
            </tbody>
          </table>
        </div>
      )}
    </main>
  );
}
