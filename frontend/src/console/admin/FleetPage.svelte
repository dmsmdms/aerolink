<script lang="ts">
  // ADM-010/011: every robot of the admin's fleets, live, with the map of last known positions.
  import * as api from '../../lib/api';
  import { type SortKey, ago, apiError, filterRobots, label, sortRobots } from '../../lib/admin';
  import { t } from '../../lib/i18n';
  import type { Signaling } from '../../lib/signaling.svelte';
  import type { RobotAdminView, User } from '../../lib/types';
  import OsmMap, { type Marker } from './OsmMap.svelte';
  import RobotDetails from './RobotDetails.svelte';

  let {
    signaling,
    user,
    onDrive,
    onNotice,
  }: {
    signaling: Signaling;
    user: User;
    /** Open the robot in the drive view (the admin holds it). */
    onDrive: (robot: number) => void;
    onNotice: (text: string) => void;
  } = $props();

  let text = $state('');
  let fleetFilter = $state(0);
  let sortKey = $state<SortKey>('state');
  let descending = $state(false);
  let selected = $state<number | null>(null);
  let adding = $state<'robot' | 'fleet' | null>(null);
  let newName = $state('');
  let newFleet = $state(1);
  let token = $state<{ robot: string; token: string } | null>(null);

  const fleets = $derived(signaling.fleets);
  const shown = $derived(
    sortRobots(
      filterRobots(
        signaling.fleetRobots.filter((r) => !fleetFilter || r.fleet === fleetFilter),
        text,
        fleets,
      ),
      sortKey,
      fleets,
      descending,
    ),
  );
  const current = $derived(signaling.fleetRobots.find((r) => r.id === selected));
  const markers = $derived<Marker[]>(
    shown
      .filter((r) => r.location)
      .map((r) => ({ id: r.id, lat: r.location!.lat, lon: r.location!.lon, label: r.name, kind: kind(r) })),
  );

  function kind(r: RobotAdminView): Marker['kind'] {
    if (r.status.mode === 'NEEDS_HELP') return 'help';
    if (r.unread_errors) return 'error';
    if (!r.online) return 'offline';
    return r.operator ? 'held' : 'online';
  }

  function sort(key: SortKey): void {
    if (sortKey === key) descending = !descending;
    else [sortKey, descending] = [key, false];
  }

  function fleetName(id: number): string {
    return fleets.find((f) => f.id === id)?.name ?? `#${id}`;
  }

  async function create(e: SubmitEvent): Promise<void> {
    e.preventDefault();
    try {
      if (adding === 'robot') {
        const r = await api.addRobot(newName.trim(), newFleet);
        token = { robot: newName.trim(), token: r.token };
        selected = r.id;
      } else if (adding === 'fleet') {
        await api.addFleet(newName.trim());
      }
      adding = null;
      newName = '';
    } catch (err) {
      onNotice(apiError(err));
    }
  }

  function startAdding(what: 'robot' | 'fleet'): void {
    adding = adding === what ? null : what;
    newName = '';
    newFleet = fleetFilter || fleets[0]?.id || 1;
  }
</script>

<div class="page">
  <div class="toolbar">
    <input class="filter" type="search" placeholder={t('fleet.filter')} bind:value={text} />
    {#if fleets.length > 1}
      <select bind:value={fleetFilter}>
        <option value={0}>{t('fleet.all')}</option>
        {#each fleets as f (f.id)}<option value={f.id}>{f.name}</option>{/each}
      </select>
    {/if}
    <span class="muted">{t('fleet.count', { shown: shown.length, total: signaling.fleetRobots.length })}</span>
    <span class="spacer"></span>
    <button type="button" onclick={() => startAdding('robot')}>{t('fleet.add_robot')}</button>
    {#if user.all_fleets}
      <button type="button" onclick={() => startAdding('fleet')}>{t('fleet.add_fleet')}</button>
    {/if}
  </div>

  {#if adding}
    <form class="toolbar" onsubmit={create}>
      <input
        required
        maxlength="64"
        placeholder={t(adding === 'robot' ? 'fleet.robot_name' : 'fleet.fleet_name')}
        bind:value={newName}
      />
      {#if adding === 'robot'}
        <select bind:value={newFleet}>
          {#each fleets as f (f.id)}<option value={f.id}>{f.name}</option>{/each}
        </select>
      {/if}
      <button type="submit" class="primary">{t('fleet.create')}</button>
      <button type="button" onclick={() => (adding = null)}>{t('fleet.cancel')}</button>
    </form>
  {/if}

  {#if token}
    <div class="token" role="status">
      <div>{t('robotpage.token_shown')} <strong>{token.robot}</strong></div>
      <code>{token.token}</code>
      <button type="button" class="link" onclick={() => (token = null)}>{t('notice.dismiss')}</button>
    </div>
  {/if}

  <div class="body">
    <div class="table">
      <table>
        <thead>
          <tr>
            <th><button type="button" class="link" onclick={() => sort('state')}>{t('fleet.col.state')}</button></th>
            <th><button type="button" class="link" onclick={() => sort('name')}>{t('fleet.col.name')}</button></th>
            {#if fleets.length > 1}
              <th><button type="button" class="link" onclick={() => sort('fleet')}>{t('fleet.col.fleet')}</button></th>
            {/if}
            <th><button type="button" class="link" onclick={() => sort('battery')}>{t('fleet.col.battery')}</button></th>
            <th>{t('fleet.col.network')}</th>
            <th><button type="button" class="link" onclick={() => sort('versions')}>{t('fleet.col.versions')}</button></th>
            <th><button type="button" class="link" onclick={() => sort('errors')}>{t('fleet.col.errors')}</button></th>
          </tr>
        </thead>
        <tbody>
          {#each shown as r (r.id)}
            <tr
              class:selected={r.id === selected}
              class:help={r.status.mode === 'NEEDS_HELP'}
              class:off={!r.online || r.disabled}
              onclick={() => (selected = r.id)}
            >
              <td>
                {#if r.disabled}
                  {t('fleet.disabled')}
                {:else if r.online}
                  <span class="dot ok"></span>{r.status.mode ? t(`mode.${r.status.mode}`) : t('fleet.online')}
                  {#if r.operator}<div class="muted small">{t('fleet.held', { name: r.operator.name })}</div>{/if}
                {:else if r.last_seen}
                  {t('fleet.seen', { ago: ago(r.last_seen) })}
                {:else}
                  {t('fleet.never')}
                {/if}
              </td>
              <td><strong>{r.name}</strong> <span class="muted">#{r.id}</span></td>
              {#if fleets.length > 1}<td>{fleetName(r.fleet)}</td>{/if}
              <td>
                {#if r.status.phone_battery !== undefined}📱 {r.status.phone_battery}%{/if}
                {#if r.status.robot_battery_mv !== undefined}
                  🔋 {(r.status.robot_battery_mv / 1000).toFixed(1)} V
                {/if}
                {#if r.status.charge_control}<span class="muted" title="charge control">⚡</span>{/if}
              </td>
              <td>
                {#if r.status.network}
                  {label('net', r.status.network.type)}
                  {#if r.status.network.operator}<span class="muted">{r.status.network.operator}</span>{/if}
                  {#if r.status.network.signal !== undefined}
                    <span class="muted">{t('net.signal', { level: r.status.network.signal })}</span>
                  {/if}
                {/if}
              </td>
              <td>
                {r.status.app ?? '–'} / {r.status.firmware ?? '–'}
                {#each [r.status.app_update, r.status.firmware_update] as state, i (i)}
                  {#if state && state !== 'current'}<div class="muted small">{label('upd', state)}</div>{/if}
                {/each}
              </td>
              <td>{#if r.unread_errors}<span class="errors">{r.unread_errors}</span>{/if}</td>
            </tr>
          {/each}
        </tbody>
      </table>
      {#if shown.length === 0}<p class="muted pad">{t('fleet.empty')}</p>{/if}
    </div>

    <div class="side">
      <div class="map">
        <OsmMap {markers} {selected} onSelect={(id) => (selected = id)} />
        {#if markers.length === 0}<p class="muted overlay">{t('fleet.no_positions')}</p>{/if}
      </div>
      {#if current}
        {#key current.id}
          <RobotDetails
            robot={current}
            {fleets}
            {signaling}
            me={user.id}
            {onDrive}
            {onNotice}
            onToken={(robot, value) => (token = { robot, token: value })}
            onClose={() => (selected = null)}
          />
        {/key}
      {/if}
    </div>
  </div>
</div>

<style>
  .page {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
  }

  .toolbar {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.6rem;
    padding: 0.6rem 1rem;
    border-bottom: 1px solid var(--border);
  }

  .toolbar input {
    width: auto;
  }

  .filter {
    flex: 0 1 24rem;
  }

  .spacer {
    flex: 1;
  }

  select {
    font: inherit;
    padding: 0.45em 0.6em;
    border-radius: 6px;
    border: 1px solid var(--border);
    background: var(--bg);
    color: var(--text);
  }

  .token {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.4rem 1rem;
    padding: 0.6rem 1rem;
    background: #12301f;
  }

  .token code {
    user-select: all;
    word-break: break-all;
    font-size: 0.95rem;
  }

  .body {
    flex: 1;
    min-height: 0;
    display: grid;
    grid-template-columns: minmax(0, 3fr) minmax(20rem, 2fr);
  }

  .table {
    overflow: auto;
    border-right: 1px solid var(--border);
  }

  table {
    width: 100%;
    border-collapse: collapse;
    font-size: 0.9rem;
  }

  th {
    position: sticky;
    top: 0;
    background: var(--panel);
    text-align: left;
    white-space: nowrap;
  }

  th,
  td {
    padding: 0.45rem 0.6rem;
    border-bottom: 1px solid var(--border);
    vertical-align: top;
  }

  tbody tr {
    cursor: pointer;
  }

  tbody tr:hover,
  tr.selected {
    background: #1d2433;
  }

  tr.help {
    box-shadow: inset 4px 0 0 var(--warn);
  }

  tr.off td {
    color: var(--muted);
  }

  .dot {
    display: inline-block;
    width: 0.55rem;
    height: 0.55rem;
    border-radius: 50%;
    margin-right: 0.35rem;
    background: var(--ok);
  }

  .small {
    font-size: 0.8rem;
  }

  .errors {
    display: inline-block;
    min-width: 1.5rem;
    padding: 0 0.4rem;
    border-radius: 0.75rem;
    text-align: center;
    background: var(--danger);
    color: #fff;
  }

  .pad {
    padding: 0 1rem;
  }

  .side {
    display: flex;
    flex-direction: column;
    min-height: 0;
    overflow-y: auto;
  }

  .map {
    position: relative;
    flex: 0 0 45%;
    min-height: 18rem;
  }

  .overlay {
    position: absolute;
    left: 0;
    right: 0;
    top: 40%;
    text-align: center;
    pointer-events: none;
  }

  @media (max-width: 900px) {
    .body {
      grid-template-columns: 1fr;
    }
  }
</style>
