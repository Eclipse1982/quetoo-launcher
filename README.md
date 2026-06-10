# Quetoo Launcher

A small cross-platform desktop launcher that installs, updates, and launches
[Quetoo](https://github.com/jdolan/quetoo) from its official GitHub releases.

- Auto-checks the latest Quetoo release on open.
- First run downloads the full bundle; later runs apply small updates.
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
public releases repo.
