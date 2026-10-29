<script lang="ts" module>
  export interface Marker {
    id: number;
    lat: number;
    lon: number;
    label: string;
    kind: 'help' | 'held' | 'online' | 'offline' | 'error';
  }
</script>

<script lang="ts">
  // ADM-011: last known robot positions on OpenStreetMap tiles. A few dozen lines of Web Mercator instead of a
  // map library: pan with the pointer, zoom with the wheel or the buttons, nothing loads unless shown.
  import {
    MAX_ZOOM,
    MIN_ZOOM,
    TILE_ATTRIBUTION,
    TILE_ATTRIBUTION_URL,
    TILE_SIZE,
    fitBounds,
    project,
    tileUrl,
    unproject,
  } from '../../lib/admin';
  import { t } from '../../lib/i18n';

  let {
    markers,
    selected,
    onSelect,
  }: { markers: Marker[]; selected: number | null; onSelect: (id: number) => void } = $props();

  let width = $state(0);
  let height = $state(0);
  let center = $state({ lat: 50.45, lon: 30.52 });
  let zoom = $state(12);
  let fitted = false;

  const origin = $derived.by(() => {
    const c = project(center.lat, center.lon, zoom);
    return { x: c.x - width / 2, y: c.y - height / 2 };
  });

  const tiles = $derived.by(() => {
    if (!width || !height) return [];
    const n = 2 ** zoom;
    const list: { key: string; url: string; left: number; top: number }[] = [];
    const x0 = Math.floor(origin.x / TILE_SIZE);
    const y0 = Math.max(0, Math.floor(origin.y / TILE_SIZE));
    const x1 = Math.floor((origin.x + width) / TILE_SIZE);
    const y1 = Math.min(n - 1, Math.floor((origin.y + height) / TILE_SIZE));
    for (let y = y0; y <= y1; y++) {
      for (let x = x0; x <= x1; x++) {
        const wrapped = ((x % n) + n) % n;
        list.push({
          key: `${zoom}/${x}/${y}`,
          url: tileUrl(zoom, wrapped, y),
          left: x * TILE_SIZE - origin.x,
          top: y * TILE_SIZE - origin.y,
        });
      }
    }
    return list;
  });

  const placed = $derived(
    markers.map((m) => {
      const p = project(m.lat, m.lon, zoom);
      return { ...m, left: p.x - origin.x, top: p.y - origin.y };
    }),
  );

  /** Fit once the size and the first positions are known; afterwards only on request. */
  $effect(() => {
    if (!fitted && width && height && markers.length) {
      fitted = true;
      fit();
    }
  });

  export function fit(): void {
    const view = fitBounds(markers, width, height);
    if (!view) return;
    center = { lat: view.lat, lon: view.lon };
    zoom = view.zoom;
  }

  function zoomAt(delta: number, px = width / 2, py = height / 2): void {
    const next = Math.max(MIN_ZOOM, Math.min(MAX_ZOOM, zoom + delta));
    if (next === zoom) return;
    // Keep the point under the pointer where it is.
    const at = unproject(origin.x + px, origin.y + py, zoom);
    const p = project(at.lat, at.lon, next);
    const c = unproject(p.x - px + width / 2, p.y - py + height / 2, next);
    zoom = next;
    center = c;
  }

  let drag: { x: number; y: number; cx: number; cy: number } | null = null;

  function down(e: PointerEvent): void {
    if ((e.target as HTMLElement).closest('button, a')) return;
    const c = project(center.lat, center.lon, zoom);
    drag = { x: e.clientX, y: e.clientY, cx: c.x, cy: c.y };
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
  }

  function move(e: PointerEvent): void {
    if (!drag) return;
    const size = TILE_SIZE * 2 ** zoom;
    const y = Math.max(0, Math.min(size, drag.cy - (e.clientY - drag.y)));
    center = unproject(drag.cx - (e.clientX - drag.x), y, zoom);
  }

  function wheel(e: WheelEvent): void {
    e.preventDefault();
    const box = (e.currentTarget as HTMLElement).getBoundingClientRect();
    zoomAt(e.deltaY < 0 ? 1 : -1, e.clientX - box.left, e.clientY - box.top);
  }
</script>

<div
  class="map"
  bind:clientWidth={width}
  bind:clientHeight={height}
  onpointerdown={down}
  onpointermove={move}
  onpointerup={() => (drag = null)}
  onpointercancel={() => (drag = null)}
  onwheel={wheel}
  role="application"
>
  {#each tiles as tile (tile.key)}
    <!-- OSM's tile policy wants a Referer; the site's policy (same-origin) would send none. -->
    <img
      class="tile"
      src={tile.url}
      alt=""
      draggable="false"
      referrerpolicy="strict-origin-when-cross-origin"
      onerror={(e) => ((e.currentTarget as HTMLImageElement).style.visibility = 'hidden')}
      style="left: {tile.left}px; top: {tile.top}px"
    />
  {/each}
  {#each placed as m (m.id)}
    <button
      type="button"
      class="marker {m.kind}"
      class:selected={m.id === selected}
      style="left: {m.left}px; top: {m.top}px"
      title={m.label}
      onclick={() => onSelect(m.id)}
    >
      <span>{m.label}</span>
    </button>
  {/each}
  <div class="controls">
    <button type="button" title={t('fleet.map_zoom_in')} onclick={() => zoomAt(1)}>+</button>
    <button type="button" title={t('fleet.map_zoom_out')} onclick={() => zoomAt(-1)}>−</button>
    <button type="button" onclick={fit}>{t('fleet.map_fit')}</button>
  </div>
  <a class="attribution" href={TILE_ATTRIBUTION_URL} target="_blank" rel="noopener noreferrer">{TILE_ATTRIBUTION}</a>
</div>

<style>
  .map {
    position: relative;
    overflow: hidden;
    height: 100%;
    min-height: 18rem;
    background: #1b1f27;
    touch-action: none;
    user-select: none;
    cursor: grab;
  }

  .tile {
    position: absolute;
    width: 256px;
    height: 256px;
    pointer-events: none;
    /* Dark console: dim the light map a little. */
    filter: brightness(0.8);
  }

  .marker {
    position: absolute;
    transform: translate(-50%, -100%);
    padding: 0.1rem 0.45rem;
    border-radius: 0.6rem;
    border: 2px solid #fff;
    font-size: 0.8rem;
    white-space: nowrap;
    color: #fff;
    background: var(--muted);
  }

  .marker::after {
    content: '';
    position: absolute;
    left: 50%;
    bottom: -7px;
    margin-left: -5px;
    border: 5px solid transparent;
    border-top-color: #fff;
    border-bottom: 0;
  }

  .marker.online {
    background: var(--ok);
  }

  .marker.held {
    background: var(--accent);
  }

  .marker.help,
  .marker.error {
    background: var(--warn);
    color: #111;
  }

  .marker.offline {
    background: #4b5563;
  }

  .marker.selected {
    z-index: 2;
    outline: 3px solid var(--accent);
  }

  .controls {
    position: absolute;
    top: 0.5rem;
    right: 0.5rem;
    display: flex;
    gap: 0.3rem;
  }

  .controls button {
    padding: 0.2em 0.7em;
  }

  .attribution {
    position: absolute;
    right: 0;
    bottom: 0;
    padding: 0.1rem 0.4rem;
    font-size: 0.75rem;
    background: rgba(15, 17, 21, 0.75);
  }
</style>
