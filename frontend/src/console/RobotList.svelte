<script lang="ts">
  import { t } from '../lib/i18n';
  import type { RobotView } from '../lib/types';

  let {
    robots,
    me,
    active,
    onTake,
  }: { robots: RobotView[]; me: number; active: number | null; onTake: (id: number) => void } = $props();

  /** ARCH-003: robots asking for help first, then online ones, then by name. */
  const sorted = $derived(
    [...robots].sort(
      (a, b) =>
        Number(b.status.mode === 'NEEDS_HELP') - Number(a.status.mode === 'NEEDS_HELP') ||
        Number(b.online) - Number(a.online) ||
        a.name.localeCompare(b.name),
    ),
  );
</script>

<h2>{t('robots.title')}</h2>
{#if sorted.length === 0}
  <p class="muted pad">{t('robots.empty')}</p>
{/if}
<ul>
  {#each sorted as robot (robot.id)}
    {@const busy = robot.operator !== null && robot.operator !== me}
    <li class:help={robot.status.mode === 'NEEDS_HELP'} class:active={robot.id === active} class:offline={!robot.online}>
      <div class="name">{robot.name}</div>
      <div class="meta muted">
        {#if !robot.online}
          {t('robots.offline')}
        {:else}
          {#if robot.status.mode}<span class="mode">{t(`mode.${robot.status.mode}`)}</span>{/if}
          {#if robot.status.phone_battery !== undefined}<span>📱 {robot.status.phone_battery}%</span>{/if}
          {#if robot.status.robot_battery_mv !== undefined}<span>🔋 {(robot.status.robot_battery_mv / 1000).toFixed(1)} V</span>{/if}
          {#if busy}<span>{t('robots.busy')}</span>{:else if robot.operator === me}<span>{t('robots.mine')}</span>{/if}
        {/if}
      </div>
      <button type="button" class="primary" disabled={!robot.online || busy} onclick={() => onTake(robot.id)}>
        {robot.id === active ? t('robots.open') : t('robots.take')}
      </button>
    </li>
  {/each}
</ul>

<style>
  h2 {
    font-size: 1rem;
    margin: 0;
    padding: 0.8rem 1rem 0.4rem;
  }

  .pad {
    padding: 0 1rem;
  }

  ul {
    list-style: none;
    margin: 0;
    padding: 0;
  }

  li {
    display: grid;
    grid-template-columns: 1fr auto;
    grid-template-areas: 'name button' 'meta button';
    align-items: center;
    gap: 0.1rem 0.6rem;
    padding: 0.6rem 1rem;
    border-bottom: 1px solid var(--border);
  }

  li.active {
    background: #1d2433;
  }

  li.help {
    box-shadow: inset 4px 0 0 var(--warn);
  }

  li.offline .name {
    color: var(--muted);
  }

  .name {
    grid-area: name;
    font-weight: 600;
  }

  .meta {
    grid-area: meta;
    display: flex;
    flex-wrap: wrap;
    gap: 0.6rem;
    font-size: 0.85rem;
  }

  .help .mode {
    color: var(--warn);
    font-weight: 600;
  }

  button {
    grid-area: button;
  }
</style>
