<script lang="ts">
  // ADM-012: one robot: everything it reported, its event log, and the admin actions.
  import { onMount } from 'svelte';
  import * as api from '../../lib/api';
  import { ago, apiError, dateTime, label } from '../../lib/admin';
  import { t } from '../../lib/i18n';
  import type { Signaling } from '../../lib/signaling.svelte';
  import type { Fleet, RobotAdminView, RobotEvent } from '../../lib/types';

  let {
    robot,
    fleets,
    signaling,
    me,
    onDrive,
    onNotice,
    onToken,
    onClose,
  }: {
    robot: RobotAdminView;
    fleets: Fleet[];
    signaling: Signaling;
    me: number;
    onDrive: (robot: number) => void;
    onNotice: (text: string) => void;
    onToken: (robot: string, token: string) => void;
    onClose: () => void;
  } = $props();

  let history = $state<RobotEvent[]>([]);
  let more = $state(false);
  let name = $state('');
  let fleet = $state(0);

  const s = $derived(robot.status);
  const mine = $derived(robot.operator?.id === me);
  // History from the server plus whatever arrived live since, newest first, each event once.
  const events = $derived.by(() => {
    const live = signaling.recentEvents.filter((e) => e.robot === robot.id).map((e) => e.event);
    const seen = new Set<number>();
    return [...live, ...history].filter((e) => !seen.has(e.id) && seen.add(e.id)).sort((a, b) => b.id - a.id);
  });
  const yesNo = (v: boolean | undefined) => t(v === undefined ? 'robotpage.unknown' : v ? 'robotpage.yes' : 'robotpage.no');

  onMount(() => {
    name = robot.name;
    fleet = robot.fleet;
    void load();
  });

  async function load(before?: number): Promise<void> {
    try {
      const page = (await api.robotEvents(robot.id, before)).events;
      history = [...history, ...page];
      more = page.length === 50;
    } catch (e) {
      onNotice(apiError(e));
    }
  }

  async function act(action: api.RobotAction): Promise<void> {
    try {
      const r = await api.robotAction(robot.id, action);
      if (r.token) onToken(robot.name, r.token);
    } catch (e) {
      onNotice(apiError(e));
    }
  }

  async function save(e: SubmitEvent): Promise<void> {
    e.preventDefault();
    if (name.trim() !== robot.name) await act({ action: 'rename', name: name.trim() });
    if (fleet !== robot.fleet) await act({ action: 'fleet', fleet });
  }

  function endSession(): void {
    if (robot.session === null) return;
    if (!confirm(t('robotpage.confirm_end', { operator: robot.operator?.name ?? '' }))) return;
    signaling.send({ type: 'admin_end', session: robot.session });
  }

  function take(): void {
    if (robot.operator && !mine && !confirm(t('robotpage.confirm_take', { robot: robot.name, operator: robot.operator.name }))) {
      return;
    }
    onDrive(robot.id);
  }
</script>

<section>
  <header>
    <h2>{robot.name} <span class="muted">#{robot.id}</span></h2>
    <button type="button" class="link" onclick={onClose}>{t('robotpage.close')}</button>
  </header>

  <ul class="facts">
    <li>
      {#if robot.disabled}
        {t('fleet.disabled')}
      {:else if robot.online}
        {t('fleet.online')}{#if s.mode}, {t('robotpage.mode', { mode: t(`mode.${s.mode}`) })}{/if}
      {:else}
        {robot.last_seen ? t('fleet.seen', { ago: ago(robot.last_seen) }) : t('fleet.never')}
      {/if}
    </li>
    <li>{robot.operator ? t('robotpage.session', { name: robot.operator.name }) : t('robotpage.free')}</li>
    {#if s.phone_battery !== undefined}<li>{t('robotpage.phone_battery', { percent: s.phone_battery })}</li>{/if}
    {#if s.robot_battery_mv !== undefined}
      <li>{t('robotpage.robot_battery', { volts: (s.robot_battery_mv / 1000).toFixed(1) })}</li>
    {/if}
    {#if s.phone}<li>{t('robotpage.phone', { model: s.phone, android: s.android ?? '?' })}</li>{/if}
    {#if s.network}
      <li>
        {label('net', s.network.type)}{#if s.network.operator}, {s.network.operator}{/if}{#if s.network.signal !== undefined},
          {t('net.signal', { level: s.network.signal })}{/if}
      </li>
    {/if}
    {#if s.app}
      <li>{t('robotpage.app', { version: s.app, state: s.app_update ? label('upd', s.app_update) : '–' })}</li>
    {/if}
    {#if s.firmware || s.firmware_update}
      <li>
        {t('robotpage.firmware', {
          version: s.firmware ?? '?',
          state: s.firmware_update ? label('upd', s.firmware_update) : '–',
        })}
      </li>
    {/if}
    <li>{t('robotpage.device_owner', { value: yesNo(s.device_owner) })}</li>
    <li>{t('robotpage.charge_control', { value: yesNo(s.charge_control) })}</li>
    {#if s.started}<li>{t('robotpage.started', { time: dateTime(s.started) })}</li>{/if}
    {#if robot.location}
      <li>
        {t('robotpage.location', {
          lat: robot.location.lat.toFixed(5),
          lon: robot.location.lon.toFixed(5),
          accuracy: Math.round(robot.location.accuracy_m ?? 0),
          ago: ago(robot.location.at),
        })}
      </li>
    {/if}
  </ul>

  <div class="actions">
    {#if robot.online && !robot.disabled}
      <button type="button" class="primary" onclick={take}>
        {robot.operator && !mine ? t('robotpage.take') : t('robotpage.drive')}
      </button>
    {/if}
    {#if robot.session !== null && !mine}
      <button type="button" class="danger" onclick={endSession}>{t('robotpage.end')}</button>
    {/if}
    {#if robot.unread_errors}
      <button type="button" onclick={() => act({ action: 'read_errors' })}>{t('robotpage.read')}</button>
    {/if}
  </div>

  <form class="edit" onsubmit={save}>
    <label>{t('robotpage.name')} <input required maxlength="64" bind:value={name} /></label>
    {#if fleets.length > 1}
      <label>
        {t('robotpage.fleet')}
        <select bind:value={fleet}>
          {#each fleets as f (f.id)}<option value={f.id}>{f.name}</option>{/each}
        </select>
      </label>
    {/if}
    <button type="submit" disabled={name.trim() === robot.name && fleet === robot.fleet}>{t('robotpage.save')}</button>
  </form>

  <div class="actions">
    {#if robot.disabled}
      <button type="button" onclick={() => act({ action: 'enable' })}>{t('robotpage.enable')}</button>
    {:else}
      <button
        type="button"
        onclick={() => confirm(t('robotpage.confirm_disable', { robot: robot.name })) && act({ action: 'disable' })}
      >
        {t('robotpage.disable')}
      </button>
    {/if}
    <button
      type="button"
      onclick={() => confirm(t('robotpage.confirm_token', { robot: robot.name })) && act({ action: 'token' })}
    >
      {t('robotpage.token')}
    </button>
  </div>

  <h3>{t('robotpage.events')}</h3>
  {#if events.length === 0}
    <p class="muted">{t('robotpage.no_events')}</p>
  {/if}
  <ol class="events">
    {#each events as e (e.id)}
      <li class={e.level}>
        <span class="muted" title={dateTime(e.received)}>{dateTime(e.at)}</span>
        <span class="level">{label('level', e.level)}</span>
        <code>{e.code}</code>
        {#if e.message}<div class="message">{e.message}</div>{/if}
      </li>
    {/each}
  </ol>
  {#if more}
    <button type="button" class="link" onclick={() => load(history.at(-1)?.id)}>{t('robotpage.more')}</button>
  {/if}
</section>

<style>
  section {
    padding: 0.6rem 1rem 1rem;
    border-top: 1px solid var(--border);
  }

  header {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
  }

  h2 {
    font-size: 1.1rem;
    margin: 0.2rem 0;
  }

  h3 {
    font-size: 1rem;
    margin: 1rem 0 0.4rem;
  }

  .facts {
    margin: 0.4rem 0;
    padding-left: 1.1rem;
    font-size: 0.9rem;
  }

  .actions,
  .edit {
    display: flex;
    flex-wrap: wrap;
    align-items: end;
    gap: 0.5rem;
    margin: 0.6rem 0;
  }

  .edit label {
    display: flex;
    flex-direction: column;
    font-size: 0.85rem;
    color: var(--muted);
  }

  .edit input {
    width: 12rem;
  }

  select {
    font: inherit;
    padding: 0.45em 0.6em;
    border-radius: 6px;
    border: 1px solid var(--border);
    background: var(--bg);
    color: var(--text);
  }

  .events {
    list-style: none;
    margin: 0;
    padding: 0;
    font-size: 0.85rem;
  }

  .events li {
    padding: 0.3rem 0;
    border-bottom: 1px solid var(--border);
  }

  .level {
    margin: 0 0.4rem;
  }

  .warning .level {
    color: var(--warn);
  }

  .error .level {
    color: var(--danger);
    font-weight: 600;
  }

  .message {
    white-space: pre-wrap;
    word-break: break-word;
  }
</style>
