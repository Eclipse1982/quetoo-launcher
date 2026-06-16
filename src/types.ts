export interface PlayerInfo {
  name: string;
  score: number;
  ping: number;
  bot: boolean;
}

export interface ServerInfo {
  addr: string;
  hostname: string;
  map: string;
  gameplay: string;
  clients: number;
  bots: number;
  maxClients: number;
  ping: number;
  protocol: number;
  favorite: boolean;
  players: PlayerInfo[];
}

export interface ServerList {
  servers: ServerInfo[];
  masterOk: boolean;
}

export type Channel = 'stable' | 'preRelease';

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
  channel: Channel;
  preReleaseAvailable: boolean;
}

export type InstallPhase = 'download' | 'snapshot' | 'extract' | 'verify' | 'data';

export interface InstallProgress {
  phase: InstallPhase;
  percent: number;
  detail: string;
}

export interface SyncSummary {
  checked: number;
  downloaded: number;
  deleted: number;
  bytesDownloaded: number;
  warnings: number;
  skipped: boolean;
}

export interface Settings {
  cvars: Record<string, string>;
  bindings: Record<string, string>; // command -> key
}

export type FieldKind =
  | { kind: 'text' }
  | { kind: 'number'; min?: number; max?: number; step?: number }
  | { kind: 'slider'; min: number; max: number; step: number }
  | { kind: 'checkbox' }
  | { kind: 'select'; options: { value: string; label: string }[] };

export interface CvarField {
  cvar: string;
  label: string;
  section: string;
  field: FieldKind;
  hint?: string;
}

export interface BindField {
  command: string;
  label: string;
  section: string;
}

export const CVAR_FIELDS: CvarField[] = [
  // Video
  { cvar: 'r_fullscreen', label: 'Display mode', section: 'Video', field: { kind: 'select', options: [{ value: '0', label: 'Windowed' }, { value: '1', label: 'Borderless fullscreen' }, { value: '2', label: 'Exclusive fullscreen' }] } },
  { cvar: 'r_fullscreen_width', label: 'Fullscreen width', section: 'Video', field: { kind: 'number' }, hint: '0 = desktop resolution' },
  { cvar: 'r_fullscreen_height', label: 'Fullscreen height', section: 'Video', field: { kind: 'number' }, hint: '0 = desktop resolution' },
  { cvar: 'r_window_width', label: 'Window width', section: 'Video', field: { kind: 'number' } },
  { cvar: 'r_window_height', label: 'Window height', section: 'Video', field: { kind: 'number' } },
  { cvar: 'r_swap_interval', label: 'VSync', section: 'Video', field: { kind: 'select', options: [{ value: '-1', label: 'Adaptive' }, { value: '0', label: 'Off' }, { value: '1', label: 'On' }] } },
  { cvar: 'cl_max_fps', label: 'FPS cap', section: 'Video', field: { kind: 'number' }, hint: '-1 = uncapped, 0 = match refresh' },
  { cvar: 'r_draw_scale', label: 'HUD scale', section: 'Video', field: { kind: 'slider', min: 0.5, max: 2, step: 0.1 } },
  { cvar: 'r_anisotropy', label: 'Anisotropic filtering', section: 'Video', field: { kind: 'select', options: [{ value: '1', label: 'Off' }, { value: '2', label: '2x' }, { value: '4', label: '4x' }, { value: '8', label: '8x' }, { value: '16', label: '16x' }] } },
  { cvar: 'r_antialias', label: 'Anti-aliasing (MSAA)', section: 'Video', field: { kind: 'select', options: [{ value: '0', label: 'Off' }, { value: '2', label: '2x' }, { value: '4', label: '4x' }, { value: '8', label: '8x' }] } },
  { cvar: 'r_modulate', label: 'Brightness', section: 'Video', field: { kind: 'slider', min: 0, max: 3, step: 0.1 } },
  { cvar: 'r_saturation', label: 'Saturation', section: 'Video', field: { kind: 'slider', min: 0, max: 2, step: 0.1 } },
  { cvar: 'r_bloom', label: 'Bloom', section: 'Video', field: { kind: 'slider', min: 0, max: 8, step: 0.5 } },
  { cvar: 'r_shadows', label: 'Shadows', section: 'Video', field: { kind: 'checkbox' } },
  // Audio
  { cvar: 's_volume', label: 'Master volume', section: 'Audio', field: { kind: 'slider', min: 0, max: 1, step: 0.05 } },
  { cvar: 's_effects_volume', label: 'Effects volume', section: 'Audio', field: { kind: 'slider', min: 0, max: 1, step: 0.05 } },
  { cvar: 's_music_volume', label: 'Music volume', section: 'Audio', field: { kind: 'slider', min: 0, max: 1, step: 0.05 } },
  { cvar: 's_ambient_volume', label: 'Ambient volume', section: 'Audio', field: { kind: 'slider', min: 0, max: 1, step: 0.05 } },
  { cvar: 's_hrtf', label: 'HRTF (headphones)', section: 'Audio', field: { kind: 'checkbox' } },
  { cvar: 'cg_hit_sound', label: 'Hit sound', section: 'Audio', field: { kind: 'checkbox' } },
  // Mouse
  { cvar: 'm_sensitivity', label: 'Sensitivity', section: 'Mouse', field: { kind: 'number', step: 0.1 } },
  { cvar: 'm_sensitivity_zoom', label: 'Zoom sensitivity', section: 'Mouse', field: { kind: 'number', step: 0.1 } },
  { cvar: 'm_invert', label: 'Invert mouse Y', section: 'Mouse', field: { kind: 'checkbox' } },
  { cvar: 'm_interpolate', label: 'Mouse smoothing', section: 'Mouse', field: { kind: 'checkbox' } },
  { cvar: 'cg_run', label: 'Always run', section: 'Mouse', field: { kind: 'checkbox' } },
  // Player
  { cvar: 'name', label: 'Player name', section: 'Player', field: { kind: 'text' } },
  { cvar: 'skin', label: 'Player model/skin', section: 'Player', field: { kind: 'text' } },
  { cvar: 'hand', label: 'Handedness', section: 'Player', field: { kind: 'select', options: [{ value: '0', label: 'Center' }, { value: '1', label: 'Right' }, { value: '2', label: 'Left' }] } },
  { cvar: 'auto_switch', label: 'Weapon auto-switch', section: 'Player', field: { kind: 'select', options: [{ value: '0', label: 'Never' }, { value: '1', label: 'From Blaster only' }, { value: '2', label: 'Always' }, { value: '3', label: 'New weapons only' }] } },
  { cvar: 'hook_style', label: 'Hook style', section: 'Player', field: { kind: 'select', options: [{ value: 'pull', label: 'Pull' }, { value: 'swing_manual', label: 'Swing (manual)' }, { value: 'swing_auto', label: 'Swing (auto)' }] } },
  // View & HUD
  { cvar: 'cg_fov', label: 'Field of view', section: 'View & HUD', field: { kind: 'number', min: 80, max: 130, step: 1 } },
  { cvar: 'cg_fov_zoom', label: 'Zoom field of view', section: 'View & HUD', field: { kind: 'number', min: 20, max: 110, step: 1 } },
  { cvar: 'cg_draw_hud', label: 'Show HUD', section: 'View & HUD', field: { kind: 'checkbox' } },
  { cvar: 'cg_draw_weapon', label: 'Show weapon model', section: 'View & HUD', field: { kind: 'checkbox' } },
  { cvar: 'cg_draw_weapon_bob', label: 'Weapon bob', section: 'View & HUD', field: { kind: 'checkbox' } },
  { cvar: 'cg_bob', label: 'View bob', section: 'View & HUD', field: { kind: 'checkbox' } },
  { cvar: 'cg_draw_blend_damage', label: 'Damage flash', section: 'View & HUD', field: { kind: 'checkbox' } },
  { cvar: 'cl_draw_counters', label: 'FPS/speed counters', section: 'View & HUD', field: { kind: 'checkbox' } },
  { cvar: 'cl_draw_net_graph', label: 'Net graph', section: 'View & HUD', field: { kind: 'checkbox' } },
  { cvar: 'cg_third_person_chasecam', label: 'Third person', section: 'View & HUD', field: { kind: 'checkbox' } },
  // Crosshair
  { cvar: 'cg_draw_crosshair', label: 'Crosshair style', section: 'Crosshair', field: { kind: 'number' }, hint: '0 = off, N = style number' },
  { cvar: 'cg_draw_crosshair_scale', label: 'Crosshair scale', section: 'Crosshair', field: { kind: 'slider', min: 0.1, max: 3, step: 0.1 } },
  { cvar: 'cg_draw_crosshair_color', label: 'Crosshair color', section: 'Crosshair', field: { kind: 'text' }, hint: "hex rrggbb or 'default'" },
  { cvar: 'cg_draw_crosshair_alpha', label: 'Crosshair opacity', section: 'Crosshair', field: { kind: 'slider', min: 0, max: 1, step: 0.05 } },
  { cvar: 'cg_draw_crosshair_health', label: 'Color by health', section: 'Crosshair', field: { kind: 'select', options: [{ value: '0', label: 'Off' }, { value: '1', label: 'Method 1' }, { value: '2', label: 'Method 2' }, { value: '3', label: 'Method 3' }, { value: '4', label: 'Method 4' }, { value: '5', label: 'Method 5' }] } },
  { cvar: 'cg_draw_crosshair_pulse', label: 'Pulse on pickup', section: 'Crosshair', field: { kind: 'checkbox' } },
];

export const BIND_FIELDS: BindField[] = [
  // Movement
  { command: '+forward', label: 'Move forward', section: 'Movement' },
  { command: '+back', label: 'Move back', section: 'Movement' },
  { command: '+move_left', label: 'Move left', section: 'Movement' },
  { command: '+move_right', label: 'Move right', section: 'Movement' },
  { command: '+move_up', label: 'Jump', section: 'Movement' },
  { command: '+move_down', label: 'Crouch', section: 'Movement' },
  { command: '+speed', label: 'Run/Walk', section: 'Movement' },
  { command: 'center_view', label: 'Center view', section: 'Movement' },
  // Combat
  { command: '+attack', label: 'Attack', section: 'Combat' },
  { command: '+hook', label: 'Hook', section: 'Combat' },
  { command: '+ZOOM', label: 'Zoom', section: 'Combat' },
  { command: 'cg_weapon_next', label: 'Next weapon', section: 'Combat' },
  { command: 'cg_weapon_previous', label: 'Previous weapon', section: 'Combat' },
  { command: 'weapon_last', label: 'Last weapon', section: 'Combat' },
  { command: '+score', label: 'Show score', section: 'Combat' },
  { command: 'kill', label: 'Kill/respawn', section: 'Combat' },
  // Weapons
  { command: 'use blaster', label: 'Blaster', section: 'Weapons' },
  { command: 'use shotgun', label: 'Shotgun', section: 'Weapons' },
  { command: 'use super shotgun', label: 'Super shotgun', section: 'Weapons' },
  { command: 'use machinegun', label: 'Machinegun', section: 'Weapons' },
  { command: 'use hand grenades', label: 'Hand grenades', section: 'Weapons' },
  { command: 'use grenade launcher', label: 'Grenade launcher', section: 'Weapons' },
  { command: 'use rocket launcher', label: 'Rocket launcher', section: 'Weapons' },
  { command: 'use hyperblaster', label: 'Hyperblaster', section: 'Weapons' },
  { command: 'use lightning gun', label: 'Lightning gun', section: 'Weapons' },
  { command: 'use railgun', label: 'Railgun', section: 'Weapons' },
  { command: 'use bfg10k', label: 'BFG10K', section: 'Weapons' },
  // Communication
  { command: 'cl_message_mode', label: 'Chat', section: 'Communication' },
  { command: 'cl_message_mode_2', label: 'Team chat', section: 'Communication' },
  // Misc
  { command: 'r_screenshot', label: 'Screenshot', section: 'Misc' },
  { command: 'cl_toggle_console', label: 'Toggle console', section: 'Misc' },
];
