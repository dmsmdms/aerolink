<script lang="ts">
  // ADM-013: who is online and driving what; adding users, blocking, passwords, roles and admins' fleets
  // (admins of every fleet only; the server checks it as well).
  import { onMount } from 'svelte';
  import * as api from '../../lib/api';
  import { apiError, label } from '../../lib/admin';
  import { t } from '../../lib/i18n';
  import type { AdminUser, Fleet, Role, User } from '../../lib/types';

  let {
    user,
    fleets,
    robotName,
    onNotice,
  }: { user: User; fleets: Fleet[]; robotName: (id: number) => string; onNotice: (text: string) => void } = $props();

  let users = $state<AdminUser[]>([]);
  let adding = $state(false);
  let form = $state({ email: '', name: '', password: '', role: 'operator' as Role });
  /** Inline editors: one open at a time. */
  let editing = $state<{ id: number; what: 'password' | 'fleets' } | null>(null);
  let password = $state('');
  let fleetAll = $state(false);
  let fleetIds = $state<number[]>([]);

  const manage = $derived(user.all_fleets);

  onMount(() => void refresh());

  async function refresh(): Promise<void> {
    try {
      users = (await api.users()).users;
    } catch (e) {
      onNotice(apiError(e));
    }
  }

  async function run(call: () => Promise<unknown>): Promise<boolean> {
    try {
      await call();
      await refresh();
      return true;
    } catch (e) {
      onNotice(apiError(e));
      return false;
    }
  }

  async function add(e: SubmitEvent): Promise<void> {
    e.preventDefault();
    const f = form;
    if (await run(() => api.addUser(f.email.trim(), f.name.trim(), f.password, f.role))) {
      adding = false;
      form = { email: '', name: '', password: '', role: 'operator' };
    }
  }

  function edit(u: AdminUser, what: 'password' | 'fleets'): void {
    if (editing?.id === u.id && editing.what === what) {
      editing = null;
      return;
    }
    editing = { id: u.id, what };
    password = '';
    fleetAll = u.all_fleets;
    fleetIds = [...u.fleets];
  }

  async function saveEdit(e: SubmitEvent, u: AdminUser): Promise<void> {
    e.preventDefault();
    const action: api.UserAction =
      editing?.what === 'password'
        ? { action: 'password', password }
        : { action: 'fleets', all: fleetAll, fleets: fleetIds };
    if (await run(() => api.userAction(u.id, action))) {
      if (action.action === 'password') onNotice(t('users.password_set', { name: u.name }));
      editing = null;
    }
  }

  function fleetsText(u: AdminUser): string {
    if (u.role !== 'admin') return '–';
    if (u.all_fleets) return t('users.all_fleets');
    return u.fleets.map((id) => fleets.find((f) => f.id === id)?.name ?? `#${id}`).join(', ') || '–';
  }
</script>

<div class="page">
  <div class="toolbar">
    <button type="button" onclick={refresh}>{t('users.refresh')}</button>
    {#if manage}
      <button type="button" onclick={() => (adding = !adding)}>{t('users.add')}</button>
    {:else}
      <span class="muted">{t('users.readonly')}</span>
    {/if}
  </div>

  {#if adding}
    <form class="toolbar" onsubmit={add}>
      <input type="email" required maxlength="254" placeholder={t('users.email')} bind:value={form.email} />
      <input required maxlength="64" placeholder={t('users.name')} bind:value={form.name} />
      <input
        type="password"
        required
        minlength="10"
        maxlength="1024"
        autocomplete="new-password"
        placeholder={t('users.password')}
        bind:value={form.password}
      />
      <select bind:value={form.role}>
        <option value="operator">{t('role.operator')}</option>
        <option value="admin">{t('role.admin')}</option>
      </select>
      <button type="submit" class="primary">{t('fleet.create')}</button>
      <button type="button" onclick={() => (adding = false)}>{t('fleet.cancel')}</button>
    </form>
  {/if}

  <div class="table">
    <table>
      <thead>
        <tr>
          <th>{t('users.col.name')}</th>
          <th>{t('users.col.email')}</th>
          <th>{t('users.col.role')}</th>
          <th>{t('users.col.fleets')}</th>
          <th>{t('users.col.state')}</th>
          {#if manage}<th></th>{/if}
        </tr>
      </thead>
      <tbody>
        {#each users as u (u.id)}
          <tr class:off={u.disabled}>
            <td>{u.name} {#if u.id === user.id}<span class="muted">({t('users.you')})</span>{/if}</td>
            <td>{u.email}</td>
            <td>{label('role', u.role)}</td>
            <td>{fleetsText(u)}</td>
            <td>
              {#if u.disabled}
                {t('users.disabled')}
              {:else if u.online}
                {t('users.online')}
                {#if u.robots.length}
                  <div class="muted">{t('users.holding', { robots: u.robots.map(robotName).join(', ') })}</div>
                {/if}
              {:else}
                <span class="muted">{t('users.offline')}</span>
              {/if}
            </td>
            {#if manage}
              <td class="actions">
                <button type="button" class="link" onclick={() => edit(u, 'password')}>{t('users.reset_password')}</button>
                {#if u.id !== user.id}
                  {#if u.disabled}
                    <button type="button" class="link" onclick={() => run(() => api.userAction(u.id, { action: 'enable' }))}>
                      {t('users.enable')}
                    </button>
                  {:else}
                    <button
                      type="button"
                      class="link"
                      onclick={() =>
                        confirm(t('users.confirm_disable', { name: u.name })) &&
                        run(() => api.userAction(u.id, { action: 'disable' }))}
                    >
                      {t('users.disable')}
                    </button>
                  {/if}
                  <button
                    type="button"
                    class="link"
                    onclick={() =>
                      run(() => api.userAction(u.id, { action: 'role', role: u.role === 'admin' ? 'operator' : 'admin' }))}
                  >
                    {u.role === 'admin' ? t('users.make_operator') : t('users.make_admin')}
                  </button>
                  {#if u.role === 'admin'}
                    <button type="button" class="link" onclick={() => edit(u, 'fleets')}>{t('users.edit_fleets')}</button>
                  {/if}
                {/if}
              </td>
            {/if}
          </tr>
          {#if editing?.id === u.id}
            <tr>
              <td colspan="6">
                <form class="inline" onsubmit={(e) => saveEdit(e, u)}>
                  {#if editing.what === 'password'}
                    <input
                      type="password"
                      required
                      minlength="10"
                      maxlength="1024"
                      autocomplete="new-password"
                      placeholder={t('users.password')}
                      bind:value={password}
                    />
                  {:else}
                    <label><input type="checkbox" bind:checked={fleetAll} /> {t('users.all_fleets')}</label>
                    {#each fleets as f (f.id)}
                      <label>
                        <input type="checkbox" value={f.id} disabled={fleetAll} bind:group={fleetIds} />
                        {f.name}
                      </label>
                    {/each}
                  {/if}
                  <button type="submit" class="primary">{t('users.save')}</button>
                  <button type="button" onclick={() => (editing = null)}>{t('fleet.cancel')}</button>
                </form>
              </td>
            </tr>
          {/if}
        {/each}
      </tbody>
    </table>
  </div>
</div>

<style>
  .page {
    display: flex;
    flex-direction: column;
    height: 100%;
  }

  .toolbar,
  .inline {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.6rem;
  }

  .toolbar {
    padding: 0.6rem 1rem;
    border-bottom: 1px solid var(--border);
  }

  input:not([type='checkbox']) {
    width: auto;
  }

  input[type='checkbox'] {
    width: auto;
  }

  select {
    font: inherit;
    padding: 0.45em 0.6em;
    border-radius: 6px;
    border: 1px solid var(--border);
    background: var(--bg);
    color: var(--text);
  }

  .table {
    overflow: auto;
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
    vertical-align: top;
  }

  tr.off td {
    color: var(--muted);
  }

  .actions {
    display: flex;
    flex-wrap: wrap;
    gap: 0.3rem 0.9rem;
  }
</style>
