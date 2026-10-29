// Operator input → DRIVE wish in ‰ (speed: + forward, steer: + right), like the phone joystick.

export interface Axes {
  speed: number;
  steer: number;
}

export const ZERO: Axes = { speed: 0, steer: 0 };

/** Keyboard: normal speed is half, Shift gives full speed (fewer accidents with a sticky key). */
export const KEY_SPEED = 500;
export const KEY_SPEED_BOOST = 1000;
export const KEY_STEER = 600;

const FORWARD = ['KeyW', 'ArrowUp'];
const BACK = ['KeyS', 'ArrowDown'];
const LEFT = ['KeyA', 'ArrowLeft'];
const RIGHT = ['KeyD', 'ArrowRight'];
export const DRIVE_KEYS = new Set([...FORWARD, ...BACK, ...LEFT, ...RIGHT]);

const any = (keys: ReadonlySet<string>, codes: string[]) => codes.some((c) => keys.has(c));

export function keysToAxes(keys: ReadonlySet<string>, boost: boolean): Axes {
  const dir = (pos: string[], neg: string[]) => Number(any(keys, pos)) - Number(any(keys, neg));
  return {
    speed: dir(FORWARD, BACK) * (boost ? KEY_SPEED_BOOST : KEY_SPEED),
    steer: dir(RIGHT, LEFT) * KEY_STEER,
  };
}

/** Dead zone with rescaling, so the output still starts at 0 and reaches ±1. */
export function deadzone(v: number, zone = 0.12): number {
  const a = Math.abs(v);
  if (a <= zone) return 0;
  return (Math.sign(v) * (Math.min(a, 1) - zone)) / (1 - zone);
}

/** Standard-mapping gamepad: left stick Y = speed, right stick X = steer (left stick X if absent). */
export function gamepadToAxes(axes: readonly number[]): Axes {
  const y = deadzone(axes[1] ?? 0);
  const x = deadzone(axes.length > 2 ? (axes[2] ?? 0) : (axes[0] ?? 0));
  return { speed: permille(-y), steer: permille(x) };
}

/** Per axis, the stronger of the two inputs wins (keyboard and gamepad can be used together). */
export function combine(a: Axes, b: Axes): Axes {
  const pick = (x: number, y: number) => (Math.abs(x) >= Math.abs(y) ? x : y);
  return { speed: pick(a.speed, b.speed), steer: pick(a.steer, b.steer) };
}

function permille(v: number): number {
  // `|| 0` folds -0 into 0.
  return Math.round(Math.max(-1, Math.min(1, v)) * 1000) || 0;
}
