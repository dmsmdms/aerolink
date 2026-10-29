<script lang="ts">
  import { combine, DRIVE_KEYS, gamepadToAxes, keysToAxes, ZERO, type Axes } from '../lib/drive';
  import { faultsText, t } from '../lib/i18n';
  import { DRIVE_PERIOD_MS, type RobotSession } from '../lib/session.svelte';
  import type { RobotView } from '../lib/types';

  let { session, robot, onRelease }: { session: RobotSession; robot: RobotView | undefined; onRelease: () => void } =
    $props();

  /** Telemetry older than this is shown as missing (the robot sends it several times a second). */
  const TELEMETRY_STALE_MS = 2000;
  const GAMEPAD_STOP_BUTTON = 1; // B on the standard mapping

  let video = $state<HTMLVideoElement>();
  let muted = $state(false);
  let micError = $state<string | null>(null);
  let axes = $state<Axes>(ZERO);
  let now = $state(performance.now());

  const keys = new Set<string>();
  let boost = false;

  const telemetry = $derived(
    session.telemetry && now - session.telemetryAt < TELEMETRY_STALE_MS ? session.telemetry : null,
  );
  const armed = $derived(telemetry?.state === 'ARMED');
  const noBase = $derived(telemetry?.base === false);
  const linked = $derived(session.connection === 'connected');
  const light = $derived(telemetry?.light);

  $effect(() => {
    if (video && session.hasVideo) video.srcObject = session.stream;
  });

  $effect(() => {
    const timer = setInterval(() => (now = performance.now()), 500);
    return () => clearInterval(timer);
  });

  // DRIVE stream: only while the robot reports ARMED (LINK-020), at 20 Hz (LINK-021). Stopping the
  // stream is itself a stop command: the robot halts after 500 ms of silence (SAFE-010).
  $effect(() => {
    if (!armed) {
      axes = ZERO;
      return;
    }
    const timer = setInterval(() => {
      const pad = navigator.getGamepads?.().find((g) => g?.connected && g.mapping === 'standard') ?? null;
      if (pad?.buttons[GAMEPAD_STOP_BUTTON]?.pressed) {
        stop();
        return;
      }
      axes = combine(keysToAxes(keys, boost), pad ? gamepadToAxes(pad.axes) : ZERO);
      session.sendDrive(axes);
    }, DRIVE_PERIOD_MS);
    return () => clearInterval(timer);
  });

  function stop(): void {
    keys.clear();
    session.disarm();
  }

  function toggleTalk(): void {
    micError = null;
    session.talk(!session.talking).catch((e: unknown) => (micError = e instanceof Error ? e.message : String(e)));
  }

  function toggleLight(): void {
    if (light !== undefined) session.light(!light);
  }

  function editing(e: KeyboardEvent): boolean {
    return e.target instanceof HTMLInputElement || e.target instanceof HTMLTextAreaElement;
  }

  function onKeyDown(e: KeyboardEvent): void {
    if (editing(e)) return;
    boost = e.shiftKey;
    if (e.code === 'Space') {
      e.preventDefault();
      stop();
    } else if (e.code === 'KeyT' && !e.repeat && linked) {
      toggleTalk();
    } else if (e.code === 'KeyL' && !e.repeat && linked) {
      toggleLight();
    } else if (DRIVE_KEYS.has(e.code)) {
      e.preventDefault();
      keys.add(e.code);
    }
  }

  function onKeyUp(e: KeyboardEvent): void {
    boost = e.shiftKey;
    keys.delete(e.code);
  }

  /** Focus lost: keyup events will not arrive, so forget held keys. */
  function onBlur(): void {
    keys.clear();
    boost = false;
  }

  /** A hidden tab must not keep driving. */
  function onVisibility(): void {
    if (document.hidden) stop();
  }
</script>

<svelte:window onkeydown={onKeyDown} onkeyup={onKeyUp} onblur={onBlur} />
<svelte:document onvisibilitychange={onVisibility} />

<section class="panel">
  <div class="title">
    <h2>{robot?.name ?? `#${session.robot}`}</h2>
    <span class="muted">
      {t('robot.connection', { state: t(`pc.${session.connection}`) })}
      {#if session.path}· {t(`robot.path.${session.path}`)}{/if}
      {#if session.rtt !== null}· {t('robot.rtt', { ms: session.rtt })}{/if}
    </span>
    <span class="spacer"></span>
    <button type="button" onclick={onRelease}>{t('robot.release')}</button>
  </div>

  <div class="video">
    <!-- svelte-ignore a11y_media_has_caption -->
    <video bind:this={video} autoplay playsinline {muted}></video>
    {#if !session.hasVideo}
      <div class="overlay muted">{t('robot.no_video')}</div>
    {/if}
  </div>

  <div class="controls">
    <button type="button" class="primary" disabled={!linked || !telemetry || noBase || armed} onclick={() => session.arm()}>
      {t('robot.arm')}
    </button>
    <button type="button" class="danger" disabled={!linked} onclick={stop}>{t('robot.disarm')}</button>
    <button type="button" onclick={() => (muted = !muted)}>{muted ? t('robot.unmute') : t('robot.mute')}</button>
    <button type="button" class:on={session.talking} disabled={!linked} onclick={toggleTalk}>
      {session.talking ? t('robot.talk_stop') : t('robot.talk')}
    </button>
    {#if light !== undefined}
      <button type="button" class:on={light} disabled={!linked} onclick={toggleLight}>
        {light ? t('robot.light_off') : t('robot.light_on')}
      </button>
    {/if}
    <span class="drive" class:active={armed}>{t('robot.drive', { speed: axes.speed, steer: axes.steer })}</span>
  </div>

  {#if micError}
    <p class="fault">{t('robot.mic_error', { error: micError })}</p>
  {/if}

  <dl class="telemetry">
    {#if telemetry && noBase}
      <div class="fault">{t('robot.no_base')}</div>
    {/if}
    {#if telemetry}
      <div class:armed class:fault={telemetry.state === 'FAULT'}>
        {t('robot.state', { state: t(`state.${telemetry.state}`) })}
      </div>
      <div class:fault={telemetry.faults !== 0}>{t('robot.faults', { faults: faultsText(telemetry.faults) })}</div>
      {#if telemetry.speed_l !== undefined && telemetry.speed_r !== undefined}
        <div>{t('robot.wheels', { left: telemetry.speed_l, right: telemetry.speed_r })}</div>
      {/if}
      {#if telemetry.battery_mv !== undefined}
        <div>{t('robot.battery', { volts: (telemetry.battery_mv / 1000).toFixed(1) })}</div>
      {/if}
      {#if telemetry.phone_battery !== undefined}
        <div>{t('robot.phone_battery', { percent: telemetry.phone_battery })}</div>
      {/if}
    {:else}
      <div class="muted">{t('robot.telemetry_stale')}</div>
    {/if}
  </dl>

  <p class="muted help">{t('robot.help')}</p>
</section>

<style>
  .panel {
    padding: 1rem;
    display: flex;
    flex-direction: column;
    gap: 0.8rem;
  }

  .title {
    display: flex;
    align-items: baseline;
    flex-wrap: wrap;
    gap: 0.8rem;
  }

  h2 {
    margin: 0;
    font-size: 1.2rem;
  }

  .spacer {
    flex: 1;
  }

  .video {
    position: relative;
    background: #000;
    border-radius: 8px;
    overflow: hidden;
    aspect-ratio: 16 / 9;
    max-height: 70vh;
  }

  video {
    width: 100%;
    height: 100%;
    object-fit: contain;
    display: block;
  }

  .overlay {
    position: absolute;
    inset: 0;
    display: grid;
    place-items: center;
  }

  .controls {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.6rem;
  }

  button.on {
    border-color: var(--ok);
    color: var(--ok);
  }

  .drive {
    color: var(--muted);
    font-variant-numeric: tabular-nums;
  }

  .drive.active {
    color: var(--ok);
  }

  .telemetry {
    display: flex;
    flex-wrap: wrap;
    gap: 0.4rem 1.4rem;
    margin: 0;
    font-variant-numeric: tabular-nums;
  }

  .armed {
    color: var(--ok);
  }

  .fault {
    color: var(--danger);
  }

  .help {
    font-size: 0.85rem;
    margin: 0;
  }
</style>
