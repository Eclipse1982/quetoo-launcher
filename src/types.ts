export type InstallState =
  | { state: 'notInstalled' }
  | { state: 'updateAvailable'; from: string; to: string }
  | { state: 'upToDate' };

export interface Status {
  installDir: string | null;
  defaultInstallDir: string | null;
  latestVersion: string;
  state: InstallState;
  canRollback: boolean;
}

export type InstallPhase = 'download' | 'snapshot' | 'extract' | 'verify';

export interface InstallProgress {
  phase: InstallPhase;
  percent: number;
  detail: string;
}

export interface Settings {
  cvars: Record<string, string>;
  bindings: Record<string, string>; // command -> key
}

// Display metadata mirrors the Rust curated tables (label/identifier order).
export const CVAR_FIELDS: { cvar: string; label: string }[] = [
  { cvar: 'name', label: 'Player name' },
  { cvar: 'cg_fov', label: 'Field of view' },
  { cvar: 'm_sensitivity', label: 'Mouse sensitivity' },
  { cvar: 'cg_draw_crosshair', label: 'Crosshair' },
  { cvar: 'cg_draw_crosshair_scale', label: 'Crosshair scale' },
  { cvar: 'cg_draw_crosshair_color', label: 'Crosshair color' },
  { cvar: 'cg_draw_weapon', label: 'Draw weapon (0/1)' },
  { cvar: 's_volume', label: 'Master volume' },
];

export const BIND_FIELDS: { command: string; label: string }[] = [
  { command: '+forward', label: 'Move forward' },
  { command: '+back', label: 'Move back' },
  { command: '+move_left', label: 'Move left' },
  { command: '+move_right', label: 'Move right' },
  { command: '+move_up', label: 'Jump' },
  { command: '+attack', label: 'Attack' },
  { command: '+speed', label: 'Run/Walk' },
  { command: '+hook', label: 'Hook' },
];
