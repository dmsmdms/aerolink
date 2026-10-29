import { describe, expect, it } from 'vitest';
import { ago, filterRobots, fitBounds, label, project, sortRobots, tileUrl, unproject } from './admin';
import { t } from './i18n';
import type { Fleet, RobotAdminView } from './types';

const fleets: Fleet[] = [
  { id: 1, name: 'default' },
  { id: 2, name: 'north' },
];

function robot(id: number, name: string, more: Partial<RobotAdminView> = {}): RobotAdminView {
  return {
    id,
    name,
    fleet: 1,
    disabled: false,
    online: true,
    status: {},
    location: null,
    last_seen: null,
    unread_errors: 0,
    session: null,
    operator: null,
    ...more,
  };
}

describe('map math', () => {
  it('projects like the OSM tile scheme', () => {
    // Kyiv at zoom 12: the slippy-map formula x = (lon + 180) / 360 · 2^z, y = (1 − asinh(tan φ) / π) / 2 · 2^z.
    const p = project(50.45, 30.52, 12);
    expect([Math.floor(p.x / 256), Math.floor(p.y / 256)]).toEqual([2395, 1381]);
    const back = unproject(p.x, p.y, 12);
    expect(back.lat).toBeCloseTo(50.45, 9);
    expect(back.lon).toBeCloseTo(30.52, 9);
    expect(tileUrl(3, 4, 5)).toBe('https://tile.openstreetmap.org/3/4/5.png');
  });

  it('fits every point', () => {
    const points = [
      { lat: 50.4, lon: 30.4 },
      { lat: 50.5, lon: 30.6 },
    ];
    const view = fitBounds(points, 800, 600)!;
    expect(view.lat).toBeCloseTo(50.45, 1);
    expect(view.lon).toBeCloseTo(30.5, 5);
    for (const p of points) {
      const a = project(p.lat, p.lon, view.zoom);
      const c = project(view.lat, view.lon, view.zoom);
      expect(Math.abs(a.x - c.x)).toBeLessThan(400);
      expect(Math.abs(a.y - c.y)).toBeLessThan(300);
    }
    // One more level would not fit.
    const a = project(50.5, 30.4, view.zoom + 1);
    const b = project(50.4, 30.6, view.zoom + 1);
    expect(b.x - a.x > 800 - 48 || b.y - a.y > 600 - 48).toBe(true);
    expect(fitBounds([{ lat: 1, lon: 2 }], 800, 600)!.zoom).toBe(17);
    expect(fitBounds([], 800, 600)).toBeNull();
  });
});

describe('fleet table', () => {
  const robots = [
    robot(1, 'bravo', { online: false, last_seen: 100 }),
    robot(2, 'alpha', { status: { mode: 'NEEDS_HELP', phone_battery: 80 } }),
    robot(3, 'charlie', { operator: { id: 7, name: 'Olga' }, status: { phone_battery: 20, app: '1.2' }, fleet: 2 }),
    robot(4, 'delta', { disabled: true, online: false, unread_errors: 2 }),
  ];

  it('sorts help first, then driven, online, offline, disabled', () => {
    expect(sortRobots(robots, 'state', fleets).map((r) => r.id)).toEqual([2, 3, 1, 4]);
    expect(sortRobots(robots, 'name', fleets).map((r) => r.name)).toEqual(['alpha', 'bravo', 'charlie', 'delta']);
    expect(sortRobots(robots, 'name', fleets, true)[0].name).toBe('delta');
    expect(sortRobots(robots, 'battery', fleets).map((r) => r.id)).toEqual([3, 2, 1, 4]);
    expect(sortRobots(robots, 'errors', fleets)[0].id).toBe(4);
    expect(sortRobots(robots, 'fleet', fleets).at(-1)!.id).toBe(3);
  });

  it('filters on several fields, every word must match', () => {
    expect(filterRobots(robots, '', fleets)).toHaveLength(4);
    expect(filterRobots(robots, 'olga', fleets).map((r) => r.id)).toEqual([3]);
    expect(filterRobots(robots, 'NORTH 1.2', fleets).map((r) => r.id)).toEqual([3]);
    expect(filterRobots(robots, '#4', fleets).map((r) => r.id)).toEqual([4]);
    expect(filterRobots(robots, 'alpha olga', fleets)).toHaveLength(0);
  });
});

it('formats relative times and labels', () => {
  expect(ago(1000, 1030)).toBe(t('ago.now'));
  expect(ago(1000, 1000 + 125)).toBe(t('ago.min', { n: 2 }));
  expect(ago(0, 7200)).toBe(t('ago.h', { n: 2 }));
  expect(ago(0, 3 * 86400)).toBe(t('ago.d', { n: 3 }));
  expect(label('upd', 'blocked')).toBe(t('upd.blocked'));
  expect(label('upd', 'something_new')).toBe('something_new');
});
