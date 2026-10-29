// Operator signaling WebSocket (docs/08-backend.md): robot list, sessions, SDP/ICE relay.
// Reconnects with jittered exponential backoff; a refused handshake caused by an expired login is told
// apart from network trouble by asking /api/v1/me.

import { HttpError, me } from './api';
import type { Fleet, OperatorMsg, Peer, RobotAdminView, RobotEvent, RobotView, ServerMsg } from './types';

/** Live robot events kept for the robot pages (older ones come from the API). */
const RECENT_EVENTS = 200;

/** The backend drops consoles silent for 2 min. */
const PING_MS = 30_000;
const MAX_BACKOFF_MS = 30_000;

export type ConnState = 'connecting' | 'open' | 'offline';

export interface SignalingEvents {
  message: (m: ServerMsg) => void;
  /** The socket closed: the backend has ended all our sessions. */
  closed: () => void;
  unauthorized: () => void;
}

export class Signaling {
  state = $state<ConnState>('connecting');
  robots = $state<RobotView[]>([]);
  you = $state<Peer | null>(null);
  /** Admins only (ADM-010): the fleets they may see and every robot in them. */
  fleets = $state<Fleet[]>([]);
  fleetRobots = $state<RobotAdminView[]>([]);
  recentEvents = $state<{ robot: number; event: RobotEvent }[]>([]);

  private ws: WebSocket | null = null;
  private retries = 0;
  private pingTimer = 0;
  private retryTimer = 0;
  private stopped = false;

  constructor(private readonly events: SignalingEvents) {}

  start(): void {
    this.open();
  }

  stop(): void {
    this.stopped = true;
    clearTimeout(this.retryTimer);
    clearInterval(this.pingTimer);
    this.ws?.close();
  }

  send(m: OperatorMsg): void {
    if (this.ws?.readyState === WebSocket.OPEN) this.ws.send(JSON.stringify(m));
  }

  private open(): void {
    this.state = 'connecting';
    const ws = new WebSocket(`${location.protocol === 'https:' ? 'wss' : 'ws'}://${location.host}/ws/operator`);
    this.ws = ws;
    ws.onopen = () => {
      this.retries = 0;
      this.state = 'open';
      this.pingTimer = window.setInterval(() => this.send({ type: 'ping' }), PING_MS);
    };
    ws.onmessage = (e: MessageEvent<string>) => this.handle(JSON.parse(e.data) as ServerMsg);
    ws.onclose = () => {
      clearInterval(this.pingTimer);
      if (this.ws !== ws || this.stopped) return;
      this.state = 'offline';
      this.events.closed();
      void this.retry();
    };
  }

  private async retry(): Promise<void> {
    try {
      await me();
    } catch (e) {
      if (e instanceof HttpError && e.status === 401) {
        this.events.unauthorized();
        return;
      }
    }
    if (this.stopped) return;
    const delay = Math.min(MAX_BACKOFF_MS, 1000 * 2 ** this.retries++) * (0.5 + Math.random() / 2);
    this.retryTimer = window.setTimeout(() => this.open(), delay);
  }

  private handle(m: ServerMsg): void {
    switch (m.type) {
      case 'welcome':
        this.you = m.you;
        break;
      case 'robots':
        this.robots = m.robots;
        break;
      case 'robot':
        upsert(this.robots, m.robot);
        break;
      case 'robot_gone':
        this.robots = this.robots.filter((r) => r.id !== m.robot);
        break;
      case 'fleet':
        this.fleets = m.fleets;
        this.fleetRobots = m.robots;
        break;
      case 'fleet_robot':
        upsert(this.fleetRobots, m.robot);
        break;
      case 'robot_event':
        this.recentEvents = [{ robot: m.robot, event: m.event }, ...this.recentEvents].slice(0, RECENT_EVENTS);
        break;
    }
    this.events.message(m);
  }
}

function upsert<T extends { id: number }>(list: T[], item: T): void {
  const i = list.findIndex((x) => x.id === item.id);
  if (i >= 0) list[i] = item;
  else list.push(item);
}
