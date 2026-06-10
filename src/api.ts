import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { ask, open } from '@tauri-apps/plugin-dialog';
import { check } from '@tauri-apps/plugin-updater';
import { relaunch } from '@tauri-apps/plugin-process';
import type { InstallProgress, Settings, Status } from './types';

export const getStatus = () => invoke<Status>('get_status');
export const setInstallDir = (dir: string) => invoke<void>('set_install_dir', { dir });
export const installOrUpdate = () => invoke<void>('install_or_update');
export const play = () => invoke<void>('play');

export async function chooseInstallDir(): Promise<string | null> {
  const selected = await open({ directory: true, multiple: false });
  return typeof selected === 'string' ? selected : null;
}

export const rollbackUpdate = () => invoke<void>('rollback_update');
export const reinstall = () => invoke<void>('reinstall');
export const uninstall = (deleteUserData: boolean) =>
  invoke<void>('uninstall', { deleteUserData });

export function onInstallProgress(
  cb: (p: InstallProgress) => void,
): Promise<UnlistenFn> {
  return listen<InstallProgress>('install-progress', (e) => cb(e.payload));
}

/** Native confirmation dialog; returns true if the user accepted. */
export const confirmDialog = (message: string, title: string) =>
  ask(message, { title, kind: 'warning' });

export const getQuetooSettings = () => invoke<Settings>('get_quetoo_settings');
export const saveQuetooSettings = (settings: Settings) =>
  invoke<void>('save_quetoo_settings', { settings });
export const defaultQuetooSettings = () => invoke<Settings>('default_quetoo_settings');

export interface LauncherUpdate {
  version: string;
  apply: () => Promise<void>;
}

/** Returns an available launcher update, or null. Never throws (offline-safe). */
export async function checkLauncherUpdate(): Promise<LauncherUpdate | null> {
  try {
    const update = await check();
    if (!update) return null;
    return {
      version: update.version,
      apply: async () => {
        await update.downloadAndInstall();
        await relaunch();
      },
    };
  } catch {
    return null; // offline / no endpoint / not signed: ignore silently
  }
}
