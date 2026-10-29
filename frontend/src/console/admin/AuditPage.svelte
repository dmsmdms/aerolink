<script lang="ts">
  // ADM-004: who did what, newest first (entries about the admin's fleets; everything for admins of all fleets).
  import { onMount } from 'svelte';
  import * as api from '../../lib/api';
  import { apiError, dateTime, label } from '../../lib/admin';
  import { t } from '../../lib/i18n';
  import type { AuditEntry } from '../../lib/types';

  let { onNotice }: { onNotice: (text: string) => void } = $props();

  let entries = $state<AuditEntry[]>([]);
  let more = $state(false);

  onMount(() => void load());

  async function load(before?: number): Promise<void> {
    try {
      const page = (await api.audit(before)).entries;
      entries = before ? [...entries, ...page] : page;
      more = page.length === 50;
    } catch (e) {
      onNotice(apiError(e));
    }
  }
</script>

<div class="page">
  <div class="toolbar">
    <button type="button" onclick={() => load()}>{t('users.refresh')}</button>
  </div>
  {#if entries.length === 0}
    <p class="muted pad">{t('audit.empty')}</p>
  {:else}
    <table>
      <thead>
        <tr>
          <th>{t('audit.col.time')}</th>
          <th>{t('audit.col.who')}</th>
          <th>{t('audit.col.action')}</th>
          <th>{t('audit.col.target')}</th>
        </tr>
      </thead>
      <tbody>
        {#each entries as e (e.id)}
          <tr>
            <td>{dateTime(e.at)}</td>
            <td>{e.user_name ?? '–'}</td>
            <td>{label('audit', e.action)}{#if e.detail}: <code>{e.detail}</code>{/if}</td>
            <td>{e.target}</td>
          </tr>
        {/each}
      </tbody>
    </table>
  {/if}
  {#if more}
    <button type="button" class="link pad" onclick={() => load(entries.at(-1)?.id)}>{t('audit.more')}</button>
  {/if}
</div>

<style>
  .page {
    height: 100%;
    overflow: auto;
  }

  .toolbar {
    padding: 0.6rem 1rem;
    border-bottom: 1px solid var(--border);
  }

  .pad {
    margin: 0.6rem 1rem;
  }

  table {
    width: 100%;
    border-collapse: collapse;
    font-size: 0.9rem;
  }

  th {
    text-align: left;
    background: var(--panel);
  }

  th,
  td {
    padding: 0.45rem 0.6rem;
    border-bottom: 1px solid var(--border);
  }
</style>
