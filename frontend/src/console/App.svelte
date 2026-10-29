<script lang="ts">
  import { onMount } from 'svelte';
  import * as api from '../lib/api';
  import { t, toggleLang } from '../lib/i18n';
  import { RobotSession } from '../lib/session.svelte';
  import { Signaling } from '../lib/signaling.svelte';
  import type { ServerMsg, User } from '../lib/types';
  import Login from './Login.svelte';
  import RobotList from './RobotList.svelte';
  import RobotPanel from './RobotPanel.svelte';

  /** undefined = still asking the server. */
  let user = $state<User | null | undefined>(undefined);
  let signaling = $state<Signaling | null>(null);
  let session = $state<RobotSession | null>(null);
  let notice = $state<string | null>(null);

  const sessionRobot = $derived(session ? signaling?.robots.find((r) => r.id === session?.robot) : undefined);

  onMount(async () => {
    try {
      user = await api.me();
    } catch {
      user = null;
    }
  });

  $effect(() => {
    if (!user) return;
    const s = new Signaling({
      message: onMessage,
      closed: endSession,
      unauthorized: () => (user = null),
    });
    signaling = s;
    s.start();
    return () => {
      s.stop();
      endSession();
      signaling = null;
    };
  });

  function onMessage(m: ServerMsg): void {
    switch (m.type) {
      case 'session':
        if (session?.id === m.session) return;
        session?.close();
        session = new RobotSession(m.session, m.robot, m.ice_servers, (msg) => signaling?.send(msg));
        session.start().catch((e: unknown) => (notice = String(e)));
        break;
      case 'answer':
        if (session?.id === m.session) session.onAnswer(m.sdp).catch((e: unknown) => (notice = String(e)));
        break;
      case 'ice':
        if (session?.id === m.session) session.onIce(m.candidate).catch(() => {});
        break;
      case 'resumed':
        if (session?.id === m.session) session.restartIce().catch((e: unknown) => (notice = String(e)));
        break;
      case 'end':
        if (session?.id === m.session) {
          endSession();
          if (m.reason !== 'released') notice = t(`end.${m.reason}`);
        }
        break;
      case 'error':
        notice = t(`error.${m.code}`);
        break;
    }
  }

  function endSession(): void {
    session?.close();
    session = null;
  }

  /** One robot at a time: taking another one releases the current. */
  function take(robot: number): void {
    if (session?.robot === robot) return;
    release();
    signaling?.send({ type: 'take', robot });
  }

  function release(): void {
    if (!session) return;
    signaling?.send({ type: 'release', session: session.id });
    endSession();
  }

  async function logout(): Promise<void> {
    try {
      await api.logout();
    } finally {
      user = null;
    }
  }
</script>

{#if user === null}
  <Login onLogin={(u) => (user = u)} />
{:else if user}
  <div class="layout">
    <header>
      <strong>{t('app.title')}</strong>
      <span class="conn {signaling?.state ?? 'connecting'}">{t(`conn.${signaling?.state ?? 'connecting'}`)}</span>
      <span class="spacer"></span>
      <span class="muted">{user.name}</span>
      <button type="button" class="link" onclick={toggleLang}>{t('lang.switch')}</button>
      <button type="button" onclick={logout}>{t('header.logout')}</button>
    </header>

    {#if notice}
      <div class="notice" role="status">
        {notice}
        <button type="button" class="link" onclick={() => (notice = null)}>{t('notice.dismiss')}</button>
      </div>
    {/if}

    <aside>
      <RobotList robots={signaling?.robots ?? []} me={user.id} active={session?.robot ?? null} onTake={take} />
    </aside>

    <main>
      {#if session}
        {#key session.id}
          <RobotPanel {session} robot={sessionRobot} onRelease={release} />
        {/key}
      {/if}
    </main>
  </div>
{/if}

<style>
  .layout {
    display: grid;
    grid-template-columns: 18rem 1fr;
    grid-template-rows: auto auto 1fr;
    grid-template-areas: 'header header' 'notice notice' 'aside main';
    height: 100vh;
  }

  header {
    grid-area: header;
    display: flex;
    align-items: center;
    gap: 1rem;
    padding: 0.6rem 1rem;
    border-bottom: 1px solid var(--border);
    background: var(--panel);
  }

  .spacer {
    flex: 1;
  }

  .conn::before {
    content: '●';
    margin-right: 0.35em;
  }

  .conn.open::before {
    color: var(--ok);
  }

  .conn.connecting::before {
    color: var(--warn);
  }

  .conn.offline::before {
    color: var(--danger);
  }

  .notice {
    grid-area: notice;
    display: flex;
    gap: 1rem;
    padding: 0.5rem 1rem;
    background: #3b2f12;
    color: #fde68a;
  }

  aside {
    grid-area: aside;
    overflow-y: auto;
    border-right: 1px solid var(--border);
  }

  main {
    grid-area: main;
    overflow: auto;
    min-width: 0;
  }

  @media (max-width: 700px) {
    .layout {
      grid-template-columns: 1fr;
      grid-template-rows: auto auto auto 1fr;
      grid-template-areas: 'header' 'notice' 'aside' 'main';
      height: auto;
    }
  }
</style>
