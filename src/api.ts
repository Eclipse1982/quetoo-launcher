import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { open } from '@tauri-apps/plugin-dialog';
import type { DownloadProgress, Status } from './types';

export const getStatus = () => invoke<Status>('get_status');
export const setInstallDir = (dir: string) => invoke<void>('set_install_dir', { dir });
export const installOrUpdate = () => invoke<void>('install_or_update');
export const play = () => invoke<void>('play');

export async function chooseInstallDir(): Promise<string | null> {
  const selected = await open({ directory: true, multiple: false });
  return typeof selected === 'string' ? selected : null;
}

export function onDownloadProgress(
  cb: (p: DownloadProgress) => void,
): Promise<UnlistenFn> {
  return listen<DownloadProgress>('download-progress', (e) => cb(e.payload));
}
