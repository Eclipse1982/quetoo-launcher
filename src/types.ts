export type InstallState =
  | { state: 'notInstalled' }
  | { state: 'updateAvailable'; from: string; to: string }
  | { state: 'upToDate' };

export interface Status {
  installDir: string | null;
  latestVersion: string;
  state: InstallState;
}

export interface DownloadProgress {
  downloaded: number;
  total: number;
}
