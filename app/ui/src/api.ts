// Typed bridge to the Rust backend. Every command the UI calls is listed in
// `Commands`, with the argument and result types the Rust side uses
// (app/src-tauri/src/commands.rs), so a mismatch is a compile error.
//
// In a normal browser there is no Tauri; mock.ts installs a stand-in
// backend on window.__TAURI__ before anything here runs.
import "./mock";

export type Status = "queued" | "downloading" | "paused" | "completed" | "error";
export type Header = [string, string];

export interface Item {
  id: number;
  url: string;
  filename: string;
  path: string;
  size: number | null;
  downloaded: number;
  status: Status;
  error: string | null;
  category: string;
  addedAt: number;
  completedAt: number | null;
  resumable: boolean;
  headers: Header[];
  remaining: [number, number][];
  /** Live (start, pos, end) per connection. */
  segments: [number, number, number][];
  speed: number;
  connections: number;
  format: string | null;
  torrentFiles: number[] | null;
  note: string | null;
  checksum?: string | null;
  verified?: boolean;
  peers?: number | null;
  seeds?: number | null;
}

export interface Settings {
  downloadDir: string;
  connections: number;
  maxConcurrent: number;
  speedLimitKib: number;
  sortIntoCategories: boolean;
  askBeforeDownload: boolean;
  notifyOnComplete: boolean;
  offerExtension: boolean;
  onboardingDismissed: boolean;
  showProgressWindow: boolean;
  closeToTray: boolean;
  theme: "system" | "light" | "dark" | string;
  seedUntil: "stop" | "1" | "2" | "forever" | string;
  /** Once all downloads finish: "nothing", "close", "sleep" or "shutdown". */
  whenDone: string;
  /** The queue is held until this time (ms since epoch); 0 = no schedule. */
  startAt: number;
  /** Proxy for downloads, like http://127.0.0.1:8080; empty follows the system's. */
  proxy: string;
  /** Send downloads out from this local address only (a VPN adapter's). */
  bindAddress: string;
  /** Offer to download a link on the clipboard when switching to SwiftDL. */
  watchClipboard: boolean;
  /** Unpack finished archives beside themselves. */
  extractArchives: boolean;
  /** Real-Debrid API token; empty = not used. */
  debridToken: string;
}

export interface Incoming {
  url: string;
  filename?: string | null;
  referrer?: string | null;
  cookies?: string | null;
  userAgent?: string | null;
  /** A video's qualities, read from the page by the extension. */
  qualities?: Quality[] | null;
  pick?: number | null;
}

export interface AppState { items: Item[]; settings: Settings; pending: Incoming[]; version: string }
export interface UpdateInfo { version: string; page: string; asset: string | null }
export interface ProbeInfo { filename: string; size: number | null; resumable: boolean; category: string; dir: string }
export interface BrowserStatus { id: string; name: string; installed: boolean; autoOffer: boolean; lastSeen?: number | null }
export interface IntegrationStatus { browsers: BrowserStatus[]; lastSeen: number | null; storeUrl: string; storeAvailable: boolean }
export interface Quality { label: string; format: string; ext: string; size: number | null }
export interface MediaInfo { title: string; site: string; duration: number | null; qualities: Quality[] }
export interface TorrentInfo { name: string; size: number; files: { path: string; size: number }[] }
export interface AddRequest {
  url: string; filename: string; dir: string; headers?: Header[]; start: boolean;
  window?: string | null; format?: string | null; size?: number | null; torrentFiles?: number[] | null;
  checksum?: string | null;
}

type Ids = { ids: number[] };
type Browser = { browser: string };
export interface Commands {
  get_state: [void, AppState];
  probe_url: [{ url: string; headers: Header[] }, ProbeInfo];
  probe_media: [{ url: string; headers: Header[] }, MediaInfo];
  probe_note: [void, string | null];
  add_links: [{ text: string }, number];
  set_url: [{ id: number; url: string }, void];
  download_next: [{ id: number }, boolean];
  add_metalink: [{ url: string }, [string, number]];
  grab_page: [{ url: string }, { url: string; name: string }[]];
  open_in_browser: [{ url: string }, void];
  is_media_url: [{ url: string }, boolean];
  is_torrent_url: [{ url: string; filename: string | null }, boolean];
  probe_torrent: [{ url: string; headers: Header[] }, TorrentInfo];
  open_new_download: [{ url: string }, void];
  default_dir: [{ filename: string }, string];
  add_download: [{ req: AddRequest }, number];
  pause: [Ids, void];
  resume: [Ids, void];
  remove: [Ids & { deleteFiles: boolean }, void];
  open_file: [{ id: number }, void];
  open_folder: [{ id: number }, void];
  set_settings: [{ settings: Settings }, void];
  open_url: [{ url: string }, void];
  extension_dir: [void, string | null];
  open_extension_folder: [void, void];
  open_extensions_page: [Browser, void];
  open_log: [void, void];
  pick_folder: [{ start: string | null }, string | null];
  integration_status: [void, IntegrationStatus];
  open_store: [Browser, void];
  get_item: [{ id: number }, Item | null];
  confirm_info: [{ token: number }, Incoming | null];
  forget_confirm: [{ token: number }, void];
  show_main: [void, void];
  check_update: [void, UpdateInfo | null];
  install_update: [{ url: string }, void];
}
export interface Events {
  downloads: Item[];
  settings: Settings;
  incoming: unknown;
}

// ---------- the Tauri globals (withGlobalTauri) ----------
interface TauriWindow {
  label: string;
  minimize(): Promise<void>;
  toggleMaximize(): Promise<void>;
  close(): Promise<void>;
  hide(): Promise<void>;
  show(): Promise<void>;
  setFocus(): Promise<void>;
  setTitle(t: string): Promise<void>;
  setSize(s: unknown): Promise<void>;
  isMaximized(): Promise<boolean>;
  onResized(cb: () => void): Promise<() => void>;
  onDragDropEvent?(cb: (e: { payload: { type: string; paths?: string[] } }) => void): Promise<() => void>;
}
interface TauriGlobal {
  core: { invoke(cmd: string, args?: unknown): Promise<unknown> };
  event: { listen(ev: string, cb: (e: { payload: unknown }) => void): Promise<() => void> };
  window?: { getCurrentWindow?(): TauriWindow };
  dpi?: { LogicalSize: new (w: number, h: number) => unknown };
}
declare global {
  interface Window {
    __TAURI__: TauriGlobal;
    setTheme(theme: string): void;
  }
}

type Args<K extends keyof Commands> = Commands[K][0];
export function invoke<K extends keyof Commands>(
  cmd: K,
  ...args: Args<K> extends void ? [] : [Args<K>]
): Promise<Commands[K][1]> {
  return window.__TAURI__.core.invoke(cmd, args[0]) as Promise<Commands[K][1]>;
}

export function listen<K extends keyof Events>(ev: K, cb: (e: { payload: Events[K] }) => void): Promise<() => void> {
  return window.__TAURI__.event.listen(ev, cb as (e: { payload: unknown }) => void);
}

export const currentWindow = (): TauriWindow | undefined => window.__TAURI__.window?.getCurrentWindow?.();
export const LogicalSize = window.__TAURI__.dpi?.LogicalSize;

/** The page's elements by id. Every id passed is in the page's HTML, so the
 *  result is never null; the type covers the element kinds the pages use. */
export type El = HTMLElement & HTMLInputElement & HTMLDialogElement & HTMLCanvasElement & HTMLSelectElement & HTMLFormElement;
export const $ = (id: string): El => document.getElementById(id) as El;
