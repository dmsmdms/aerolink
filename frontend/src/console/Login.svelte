<script lang="ts">
  import * as api from '../lib/api';
  import { t, toggleLang } from '../lib/i18n';
  import type { User } from '../lib/types';

  let { onLogin }: { onLogin: (user: User) => void } = $props();

  let email = $state('');
  let password = $state('');
  let busy = $state(false);
  let error = $state<string | null>(null);

  async function submit(e: SubmitEvent): Promise<void> {
    e.preventDefault();
    busy = true;
    error = null;
    try {
      onLogin(await api.login(email.trim(), password));
    } catch (err) {
      const status = err instanceof api.HttpError ? err.status : 0;
      error = t(status === 401 ? 'login.invalid' : status === 429 ? 'login.rate_limited' : 'login.failed');
    } finally {
      busy = false;
      password = '';
    }
  }
</script>

<form class="login" onsubmit={submit}>
  <h1>{t('login.title')}</h1>
  <label>
    {t('login.email')}
    <input type="email" autocomplete="username" required maxlength="254" bind:value={email} />
  </label>
  <label>
    {t('login.password')}
    <input type="password" autocomplete="current-password" required maxlength="1024" bind:value={password} />
  </label>
  {#if error}
    <p class="error" role="alert">{error}</p>
  {/if}
  <button type="submit" class="primary" disabled={busy}>{t('login.submit')}</button>
  <button type="button" class="link" onclick={toggleLang}>{t('lang.switch')}</button>
</form>

<style>
  .login {
    max-width: 22rem;
    margin: 15vh auto;
    padding: 1.5rem;
    display: flex;
    flex-direction: column;
    gap: 0.9rem;
    border: 1px solid var(--border);
    border-radius: 10px;
    background: var(--panel);
  }

  h1 {
    font-size: 1.3rem;
    margin: 0;
  }

  label {
    display: flex;
    flex-direction: column;
    gap: 0.3rem;
  }

  .error {
    color: var(--danger);
    margin: 0;
  }
</style>
