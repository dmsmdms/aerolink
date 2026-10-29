// One WebRTC session with a robot (docs/08-backend.md "Control channel"). The console makes the offer:
// receive-only video, two-way audio (the operator's microphone only while talking), and two DataChannels
// straight to the robot:
//   "ctl"   reliable, ordered: arm/disarm, ping/pong, telemetry;
//   "drive" unordered, no retransmits: DRIVE at 20 Hz while ARMED (a late DRIVE is worse than a lost one).
// The backend only relays the offer/answer/candidates; media and control never pass through it.

import type { Axes } from './drive';
import type { CtlFromRobot, CtlToRobot, DriveMsg, OperatorMsg, Telemetry } from './types';

/** LINK-021 / SAFE-010: the robot stops when DRIVE is silent for 500 ms. */
export const DRIVE_PERIOD_MS = 50;
const PING_MS = 1000;
const STATS_MS = 2000;
/** `disconnected` this long → ICE restart. */
const STALL_RESTART_MS = 3000;
/** How long an ICE restart is given before another one (doubling up to the maximum while none helps). */
const RESTART_WAIT_MS = 5000;
const MAX_RESTART_WAIT_MS = 30_000;
/** Drop DRIVE instead of queueing behind a congested channel. */
const MAX_BUFFERED = 1024;

export type IcePath = 'direct' | 'relay';

/** Bench/diagnostics: `?relay` in the console URL forces the media path through TURN. */
export const forceRelay = (): boolean => new URLSearchParams(location.search).has('relay');

/**
 * The robot sends the first codec of the receiver's list that it can encode: H.264 first, so phones with a
 * hardware H.264 encoder use it (PWR-013); the others fall back to VP8.
 */
function preferH264(video: RTCRtpTransceiver): void {
  const codecs = RTCRtpReceiver.getCapabilities('video')?.codecs;
  if (!codecs || !('setCodecPreferences' in video)) return;
  const rank = (c: RTCRtpCodec) => (c.mimeType.toLowerCase() === 'video/h264' ? 0 : 1);
  try {
    video.setCodecPreferences([...codecs].sort((a, b) => rank(a) - rank(b)));
  } catch {
    // Keep the browser's order.
  }
}

export class RobotSession {
  connection = $state<RTCPeerConnectionState>('new');
  telemetry = $state<Telemetry | null>(null);
  telemetryAt = $state(0);
  rtt = $state<number | null>(null);
  path = $state<IcePath | null>(null);
  hasVideo = $state(false);
  /** The operator's microphone goes to the robot's loudspeaker. */
  talking = $state(false);
  readonly stream = new MediaStream();

  private readonly pc: RTCPeerConnection;
  private readonly ctl: RTCDataChannel;
  private readonly drive: RTCDataChannel;
  private readonly voice: RTCRtpTransceiver;
  private mic: MediaStreamTrack | null = null;
  private pending: RTCIceCandidateInit[] = [];
  private seq = 0;
  private timers: number[] = [];
  /** Candidates wait while an ICE restart offer is out: they belong to the new answer. */
  private restarting = false;
  private restartAt = -Infinity;
  private stall: number | undefined;
  private restarts = 0;

  constructor(
    readonly id: number,
    readonly robot: number,
    iceServers: RTCIceServer[],
    private readonly signal: (m: OperatorMsg) => void,
  ) {
    this.pc = new RTCPeerConnection({
      iceServers,
      bundlePolicy: 'max-bundle',
      iceTransportPolicy: forceRelay() ? 'relay' : 'all',
    });
    preferH264(this.pc.addTransceiver('video', { direction: 'recvonly' }));
    // Two-way from the start, with no track until the operator talks: switching later needs no renegotiation.
    this.voice = this.pc.addTransceiver('audio', { direction: 'sendrecv' });
    this.ctl = this.pc.createDataChannel('ctl');
    this.drive = this.pc.createDataChannel('drive', { ordered: false, maxRetransmits: 0 });

    this.pc.onicecandidate = (e) => {
      if (e.candidate) signal({ type: 'ice', session: id, candidate: e.candidate.toJSON() });
    };
    this.pc.ontrack = (e) => {
      this.stream.addTrack(e.track);
      if (e.track.kind === 'video') this.hasVideo = true;
    };
    this.pc.onconnectionstatechange = () => {
      this.connection = this.pc.connectionState;
      // A network change on either side: new candidates are needed (the robot also asks via `resumed`).
      // `failed` takes ~30 s in browsers, too long for driving: a few seconds of `disconnected` are enough.
      // Restarts that do not help back off, so a path that cannot work does not flood the signaling.
      clearTimeout(this.stall);
      if (this.connection === 'connected') this.restarts = 0;
      if (this.connection === 'failed' || this.connection === 'disconnected') {
        const backoff = this.restartAt + Math.min(RESTART_WAIT_MS * 2 ** this.restarts, MAX_RESTART_WAIT_MS);
        const wait = Math.max(this.connection === 'failed' ? 0 : STALL_RESTART_MS, backoff - performance.now());
        this.stall = window.setTimeout(() => {
          this.restarts++;
          void this.restartIce().catch(() => {});
        }, wait);
      }
    };
    this.ctl.onmessage = (e: MessageEvent<string>) => this.onCtl(e.data);
    this.ctl.onopen = () => {
      this.timers.push(window.setInterval(() => this.sendCtl({ t: 'ping', ts: performance.now() }), PING_MS));
    };
    this.timers.push(window.setInterval(() => void this.updatePath(), STATS_MS));
  }

  async start(): Promise<void> {
    await this.pc.setLocalDescription(await this.pc.createOffer());
    const sdp = this.pc.localDescription;
    if (sdp) this.signal({ type: 'offer', session: this.id, sdp: sdp.toJSON() });
  }

  async onAnswer(sdp: RTCSessionDescriptionInit): Promise<void> {
    await this.pc.setRemoteDescription(sdp);
    this.restarting = false;
    for (const candidate of this.pending.splice(0)) await this.pc.addIceCandidate(candidate);
  }

  async onIce(candidate: RTCIceCandidateInit): Promise<void> {
    if (this.pc.remoteDescription && !this.restarting) await this.pc.addIceCandidate(candidate);
    else this.pending.push(candidate);
  }

  /**
   * SIG-007: the robot is back on a new network (or the path failed). Same tracks and channels, new ICE
   * credentials; the robot answers the new offer like the first one.
   */
  async restartIce(): Promise<void> {
    if (this.connection === 'closed') return;
    const offered = this.pc.signalingState === 'have-local-offer';
    // `failed` and `resumed` often come together: one restart is enough.
    if (offered && performance.now() - this.restartAt < RESTART_WAIT_MS) return;
    // An offer the robot never answered (it was offline then): start over.
    if (offered) await this.pc.setLocalDescription({ type: 'rollback' });
    this.restarting = true;
    this.restartAt = performance.now();
    await this.pc.setLocalDescription(await this.pc.createOffer({ iceRestart: true }));
    const sdp = this.pc.localDescription;
    if (sdp) this.signal({ type: 'offer', session: this.id, sdp: sdp.toJSON() });
  }

  arm(): void {
    this.sendCtl({ t: 'arm' });
  }

  disarm(): void {
    this.sendCtl({ t: 'disarm' });
  }

  /** CTL-004: the phone's flashlight. */
  light(on: boolean): void {
    this.sendCtl({ t: 'light', on });
  }

  /** The microphone is opened on the first talk and released when talking stops. */
  async talk(on: boolean): Promise<void> {
    if (on === this.talking) return;
    if (on) {
      const media = await navigator.mediaDevices.getUserMedia({
        audio: { echoCancellation: true, noiseSuppression: true, autoGainControl: true },
      });
      this.mic = media.getAudioTracks()[0] ?? null;
      if (!this.mic || this.connection === 'closed') {
        media.getTracks().forEach((t) => t.stop());
        this.mic = null;
        return;
      }
      await this.voice.sender.replaceTrack(this.mic);
      this.talking = true;
    } else {
      this.talking = false;
      await this.voice.sender.replaceTrack(null);
      this.mic?.stop();
      this.mic = null;
    }
  }

  sendDrive(axes: Axes): void {
    if (this.drive.readyState !== 'open' || this.drive.bufferedAmount > MAX_BUFFERED) return;
    const msg: DriveMsg = { seq: ++this.seq, ts: Date.now(), speed: axes.speed, steer: axes.steer };
    this.drive.send(JSON.stringify(msg));
  }

  close(): void {
    this.timers.forEach(clearInterval);
    this.timers = [];
    clearTimeout(this.stall);
    this.mic?.stop();
    this.mic = null;
    this.talking = false;
    this.pc.close();
    this.connection = 'closed';
  }

  private sendCtl(msg: CtlToRobot): void {
    if (this.ctl.readyState === 'open') this.ctl.send(JSON.stringify(msg));
  }

  private onCtl(data: string): void {
    let msg: CtlFromRobot;
    try {
      msg = JSON.parse(data) as CtlFromRobot;
    } catch {
      return;
    }
    if (msg.t === 'telemetry') {
      const { t: _, ...telemetry } = msg;
      this.telemetry = telemetry;
      this.telemetryAt = performance.now();
    } else if (msg.t === 'pong') {
      this.rtt = Math.round(performance.now() - msg.ts);
    }
  }

  /** Direct (host/srflx) or via TURN: shown to the operator, it explains latency. */
  private async updatePath(): Promise<void> {
    if (this.pc.connectionState !== 'connected') return;
    const stats = await this.pc.getStats();
    let pairId: string | undefined;
    stats.forEach((s) => {
      if (s.type === 'transport' && s.selectedCandidatePairId) pairId = s.selectedCandidatePairId;
      // Firefox has no transport stats: it flags the pair itself.
      else if (s.type === 'candidate-pair' && s.selected && !pairId) pairId = s.id;
    });
    const pair = pairId ? stats.get(pairId) : undefined;
    const local = pair ? stats.get(pair.localCandidateId) : undefined;
    const remote = pair ? stats.get(pair.remoteCandidateId) : undefined;
    if (local && remote) {
      this.path = local.candidateType === 'relay' || remote.candidateType === 'relay' ? 'relay' : 'direct';
    }
  }
}
