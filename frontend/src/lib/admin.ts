// Fleet administration helpers (docs/08-backend.md "Fleet administration"): table filter and sort, relative
// times, and the Web Mercator math of the map. Pure, unit-tested.

import { HttpError } from './api';
import { has, lang, t } from './i18n';
import type { Fleet, RobotAdminView } from './types';

/**
 * ADM-011: map tiles. Public OpenStreetMap tiles for now (their usage policy: attribution shown, a Referer sent,
 * moderate use); a paid or self-hosted source replaces this URL and the CSP `img-src` in
 * backend/package/etc/nginx/snippets/aerolink-headers.conf.
 */
export const TILE_URL = 'https://tile.openstreetmap.org/{z}/{x}/{y}.png';
export const TILE_ATTRIBUTION = '© OpenStreetMap contributors';
export const TILE_ATTRIBUTION_URL = 'https://www.openstreetmap.org/copyright';
export const TILE_SIZE = 256;
export const MIN_ZOOM = 2;
export const MAX_ZOOM = 19;

export function tileUrl(z: number, x: number, y: number): string {
  return TILE_URL.replace('{z}', String(z)).replace('{x}', String(x)).replace('{y}', String(y));
}

/** World pixel coordinates of a position at a zoom level (Web Mercator, 256-px tiles). */
export function project(lat: number, lon: number, zoom: number): { x: number; y: number } {
  const size = TILE_SIZE * 2 ** zoom;
  const clamped = Math.max(-85.05112878, Math.min(85.05112878, lat));
  const sin = Math.sin((clamped * Math.PI) / 180);
  return {
    x: ((lon + 180) / 360) * size,
    y: (0.5 - Math.log((1 + sin) / (1 - sin)) / (4 * Math.PI)) * size,
  };
}

export function unproject(x: number, y: number, zoom: number): { lat: number; lon: number } {
  const size = TILE_SIZE * 2 ** zoom;
  const lon = (x / size) * 360 - 180;
  const n = Math.PI - (2 * Math.PI * y) / size;
  const lat = (180 / Math.PI) * Math.atan(Math.sinh(n));
  return { lat, lon };
}

/** Center and the highest zoom (≤ 17) that shows every point in a `width` × `height` view. */
export function fitBounds(
  points: { lat: number; lon: number }[],
  width: number,
  height: number,
): { lat: number; lon: number; zoom: number } | null {
  if (points.length === 0) return null;
  const lats = points.map((p) => p.lat);
  const lons = points.map((p) => p.lon);
  const north = Math.max(...lats);
  const south = Math.min(...lats);
  const east = Math.max(...lons);
  const west = Math.min(...lons);
  const margin = 48;
  let zoom = 17;
  for (; zoom > MIN_ZOOM; zoom--) {
    const a = project(north, west, zoom);
    const b = project(south, east, zoom);
    if (b.x - a.x <= width - margin && b.y - a.y <= height - margin) break;
  }
  const a = project(north, west, zoom);
  const b = project(south, east, zoom);
  return { ...unproject((a.x + b.x) / 2, (a.y + b.y) / 2, zoom), zoom };
}

/** "5 min ago" from Unix seconds. */
export function ago(at: number, now: number = Date.now() / 1000): string {
  const s = Math.max(0, now - at);
  if (s < 60) return t('ago.now');
  if (s < 3600) return t('ago.min', { n: Math.floor(s / 60) });
  if (s < 86400) return t('ago.h', { n: Math.floor(s / 3600) });
  return t('ago.d', { n: Math.floor(s / 86400) });
}

export function dateTime(at: number): string {
  return new Date(at * 1000).toLocaleString(lang);
}

/** A translated value when there is a string for it, else the raw value (states from newer robots). */
export function label(prefix: string, value: string): string {
  const key = `${prefix}.${value}`;
  return has(key) ? t(key) : value;
}

export type SortKey = 'name' | 'fleet' | 'state' | 'battery' | 'errors' | 'versions';

/** Help first, then online, then by name: the order the operator list uses too (ARCH-003). */
function stateRank(r: RobotAdminView): number {
  if (r.disabled) return 4;
  if (!r.online) return 3;
  if (r.status.mode === 'NEEDS_HELP') return 0;
  return r.operator ? 1 : 2;
}

export function sortRobots(robots: RobotAdminView[], key: SortKey, fleets: Fleet[], descending = false): RobotAdminView[] {
  const fleetName = (id: number) => fleets.find((f) => f.id === id)?.name ?? '';
  const by: Record<SortKey, (a: RobotAdminView, b: RobotAdminView) => number> = {
    name: (a, b) => a.name.localeCompare(b.name),
    fleet: (a, b) => fleetName(a.fleet).localeCompare(fleetName(b.fleet)),
    state: (a, b) => stateRank(a) - stateRank(b) || (b.last_seen ?? 0) - (a.last_seen ?? 0),
    // Unknown batteries last.
    battery: (a, b) => (a.status.phone_battery ?? 101) - (b.status.phone_battery ?? 101),
    errors: (a, b) => b.unread_errors - a.unread_errors,
    versions: (a, b) => (a.status.app ?? '').localeCompare(b.status.app ?? ''),
  };
  const sign = descending ? -1 : 1;
  return [...robots].sort((a, b) => sign * by[key](a, b) || a.name.localeCompare(b.name));
}

/** Case-insensitive match on name, id, fleet, operator, phone model and versions. */
export function filterRobots(robots: RobotAdminView[], text: string, fleets: Fleet[]): RobotAdminView[] {
  const words = text.toLowerCase().split(/\s+/).filter(Boolean);
  if (words.length === 0) return robots;
  return robots.filter((r) => {
    const hay = [
      r.name,
      `#${r.id}`,
      fleets.find((f) => f.id === r.fleet)?.name,
      r.operator?.name,
      r.status.phone,
      r.status.app,
      r.status.firmware,
      r.status.network?.operator,
    ]
      .filter(Boolean)
      .join(' ')
      .toLowerCase();
    return words.every((w) => hay.includes(w));
  });
}

/** A failed admin API call, for the notice line. */
export function apiError(e: unknown): string {
  if (e instanceof HttpError) {
    if (e.status === 409) return t('admin.conflict');
    if (e.status === 400 || e.status === 422) return t('admin.bad_request');
    if (e.status === 403) return t('error.forbidden');
    if (e.status === 404) return t('error.not_found');
  }
  return t('admin.failed', { error: String(e) });
}
