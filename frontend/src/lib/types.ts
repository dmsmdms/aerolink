// Signaling messages (docs/08-backend.md "Signaling"), mirrored from backend/src/hub.rs.

export type RobotMode = 'IDLE' | 'TELEOP' | 'AUTO' | 'NEEDS_HELP';

export interface RobotStatus {
  mode?: RobotMode;
  phone_battery?: number;
  robot_battery_mv?: number;
  charge_control?: boolean;
  app?: string;
  firmware?: string;
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

export type ErrorCode = 'not_found' | 'offline' | 'busy' | 'no_session';
export type EndReason = 'released' | 'robot_ended' | 'robot_offline' | 'operator_offline';

export type ServerMsg =
  | { type: 'welcome'; you: Peer }
  | { type: 'robots'; robots: RobotView[] }
  | { type: 'robot'; robot: RobotView }
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
  | { type: 'ping' };

export interface User {
  id: number;
  email: string;
  name: string;
  role: string;
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
