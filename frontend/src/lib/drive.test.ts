import { describe, expect, it } from 'vitest';
import { combine, deadzone, gamepadToAxes, keysToAxes, KEY_SPEED, KEY_SPEED_BOOST, KEY_STEER } from './drive';

describe('keyboard', () => {
  it('maps WASD and arrows', () => {
    expect(keysToAxes(new Set(['KeyW']), false)).toEqual({ speed: KEY_SPEED, steer: 0 });
    expect(keysToAxes(new Set(['ArrowDown', 'ArrowLeft']), false)).toEqual({ speed: -KEY_SPEED, steer: -KEY_STEER });
    expect(keysToAxes(new Set(['KeyW']), true).speed).toBe(KEY_SPEED_BOOST);
  });

  it('opposite keys cancel out', () => {
    expect(keysToAxes(new Set(['KeyW', 'KeyS', 'KeyA', 'KeyD']), false)).toEqual({ speed: 0, steer: 0 });
  });
});

describe('gamepad', () => {
  it('applies a rescaled dead zone', () => {
    expect(deadzone(0.1)).toBe(0);
    expect(deadzone(1)).toBe(1);
    expect(deadzone(-1)).toBe(-1);
    expect(deadzone(0.56)).toBeCloseTo(0.5);
  });

  it('left stick up is forward, right stick steers', () => {
    expect(gamepadToAxes([0, -1, 1, 0])).toEqual({ speed: 1000, steer: 1000 });
    expect(gamepadToAxes([0.05, 0.05, -0.05, 0])).toEqual({ speed: 0, steer: 0 });
  });

  it('falls back to the left stick for steering', () => {
    expect(gamepadToAxes([-1, 0])).toEqual({ speed: 0, steer: -1000 });
  });
});

it('the stronger input wins per axis', () => {
  expect(combine({ speed: 500, steer: 0 }, { speed: -800, steer: 100 })).toEqual({ speed: -800, steer: 100 });
});
