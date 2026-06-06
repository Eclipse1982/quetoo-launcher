# Quetoo Launcher

A small cross-platform desktop launcher that installs, updates, and launches
**Quetoo RailWarz** — the [Quetoo](https://github.com/jdolan/quetoo) engine with the
[Eclipse1982/quetoo](https://github.com/Eclipse1982/quetoo) RailWarz mod (Q2/TMG FFA parity).

- Auto-checks the latest official **and** RailWarz releases on open.
- Install is two layers: the official Quetoo **base** (engine + game data), then the
  matched **RailWarz overlay** (our engine + `game`/`cgame` modules) on top — so the
  client predicts movement identically to the RailWarz servers.
- First run downloads the full base bundle; later runs apply small base updates and/or
  re-apply the overlay when either source moves.
- One-click **Play**.
- Self-updating (Tauri updater) — download the latest launcher from the
  [releases page](https://github.com/Eclipse1982/quetoo-launcher-releases/releases/latest).

## Develop

```bash
npm install
npm run tauri dev
```

## Release

```bash
node scripts/bump-version.mjs 0.2.0
git commit -am "release: v0.2.0"
git tag v0.2.0 && git push && git push origin v0.2.0
```

CI builds signed artifacts for Windows, macOS, and Linux and mirrors them to the
public releases repo. See `docs/superpowers/specs/` for the design.
