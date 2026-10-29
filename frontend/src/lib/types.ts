// Signaling messages (docs/08-backend.md "Signaling"), mirrored from backend/src/hub.rs.

export type RobotMode = 'IDLE' | 'TELEOP' | 'AUTO' | 'NEEDS_HELP';

export interface Network {
  type: 'wifi' | 'cellular' | 'other';
  operator?: string;
  /** 0..4 */
  signal?: number;
}

export interface RobotStatus {
  mode?: RobotMode;
  phone_battery?: number;
  robot_battery_mv?: number;
  charge_control?: boolean;
  app?: string;
  firmware?: string;
  /** ADM-001 */
  phone?: string;
  android?: string;
  network?: Network;
  app_update?: string;
  firmware_update?: string;
  device_owner?: boolean;
  /** Robot service start, Unix seconds. */
  started?: number;
}

export interface RobotView {
  id: number;
  name: string;
  online: boolean;
  status: RobotStatus;
  operator: number | null;
}

export interface Peer {
  id: number;
  name: string;
}

// Fleet administration (docs/08-backend.md "Fleet administration").

export interface Fleet {
  id: number;
  name: string;
}

export interface Location {
  lat: number;
  lon: number;
  accuracy_m?: number;
  /** Unix seconds. */
  at: number;
}

export interface RobotAdminView {
  id: number;
  name: string;
  fleet: number;
  disabled: boolean;
  online: boolean;
  /** Last known, also while offline. */
  status: RobotStatus;
  location: Location | null;
  /** Unix seconds; null while online. */
  last_seen: number | null;
  unread_errors: number;
  session: number | null;
  operator: Peer | null;
}

export type EventLevel = 'info' | 'warning' | 'error';

export interface RobotEvent {
  id: number;
  level: EventLevel;
  code: string;
  message: string;
  at: number;
  received: number;
}

export interface AdminUser extends User {
  disabled: boolean;
  online: boolean;
  /** Robots held right now. */
  robots: number[];
}

export interface AuditEntry {
  id: number;
  at: number;
  user: number | null;
  user_name: string | null;
  fleet: number | null;
  action: string;
  target: string;
  detail: string;
}

export type ErrorCode = 'not_found' | 'offline' | 'busy' | 'no_session' | 'forbidden';
export type EndReason = 'released' | 'robot_ended' | 'robot_offline' | 'operator_offline' | 'admin';

export type ServerMsg =
  | { type: 'welcome'; you: Peer }
  | { type: 'robots'; robots: RobotView[] }
  | { type: 'robot'; robot: RobotView }
  | { type: 'robot_gone'; robot: number }
  | { type: 'fleet'; fleets: Fleet[]; robots: RobotAdminView[] }
  | { type: 'fleet_robot'; robot: RobotAdminView }
  | { type: 'robot_event'; robot: number; event: RobotEvent }
  | { type: 'session'; session: number; robot: number; peer: Peer; ice_servers: RTCIceServer[] }
  | { type: 'answer'; session: number; sdp: RTCSessionDescriptionInit }
  | { type: 'ice'; session: number; candidate: RTCIceCandidateInit }
  | { type: 'end'; session: number; reason: EndReason }
  | { type: 'resumed'; session: number }
  | { type: 'error'; code: ErrorCode; robot?: number }
  | { type: 'pong' };

export type OperatorMsg =
  | { type: 'take'; robot: number }
  | { type: 'offer'; session: number; sdp: RTCSessionDescriptionInit }
  | { type: 'ice'; session: number; candidate: RTCIceCandidateInit }
  | { type: 'release'; session: number }
  | { type: 'ping' }
  | { type: 'admin_end'; session: number }
  | { type: 'admin_take'; robot: number };

export type Role = 'operator' | 'admin';

export interface User {
  id: number;
  email: string;
  name: string;
  role: Role;
  /** Admins: every fleet (and user management)… */
  all_fleets: boolean;
  /** …or these. */
  fleets: number[];
}

// Peer-to-peer DataChannel messages, console ↔ robot (docs/08-backend.md "Control channel").

/** Robot state as in the USB protocol (docs/03-usb-link.md). */
export type RobotState = 'DISARMED' | 'ARMED' | 'STOPPING' | 'FAULT';

export interface Telemetry {
  /** false: the phone has no usable robot base (RP2040), so there is nothing to arm. */
  base?: boolean;
  state: RobotState;
  faults: number;
  speed_l?: number;
  speed_r?: number;
  battery_mv?: number;
  phone_battery?: number;
  /** Flashlight state; absent when the robot's camera has none. */
  light?: boolean;
}

export type CtlFromRobot = ({ t: 'telemetry' } & Telemetry) | { t: 'pong'; ts: number };
export type CtlToRobot = { t: 'arm' } | { t: 'disarm' } | { t: 'ping'; ts: number } | { t: 'light'; on: boolean };

/** "drive" channel: unordered, no retransmits; the robot drops seq ≤ last seen (SAFE-012). */
export interface DriveMsg {
  seq: number;
  ts: number;
  speed: number;
  steer: number;
}
