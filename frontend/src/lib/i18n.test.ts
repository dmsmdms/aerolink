import { expect, it } from 'vitest';
import { dictionaries, faultsText, t } from './i18n';

it('every language has every key, with the same placeholders', () => {
  const placeholders = (s: string) => [...s.matchAll(/\{(\w+)\}/g)].map((m) => m[1]).sort();
  for (const [key, text] of Object.entries(dictionaries.en)) {
    const ru = dictionaries.ru[key as keyof typeof dictionaries.en];
    expect(ru, key).toBeTruthy();
    expect(placeholders(ru), key).toEqual(placeholders(text));
  }
});

it('substitutes parameters', () => {
  expect(t('robot.rtt', { ms: 42 })).toContain('42');
});

it('names fault bits', () => {
  expect(faultsText(0)).toBe(t('faults.none'));
  expect(faultsText(0b0101)).toBe(`${t('fault.motor_link_lost')}, ${t('fault.low_battery')}`);
});
