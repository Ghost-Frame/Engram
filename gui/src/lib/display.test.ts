import { describe, expect, it } from 'vitest';
import { displayCount, displayServiceName, displayTime } from './display';

describe('displayServiceName', () => {
  it('normalizes persisted legacy service identities without rewriting unknown services', () => {
    expect(displayServiceName('engram')).toBe('Kleos');
    expect(displayServiceName(' ENGRAM-RUST ')).toBe('Kleos');
    expect(displayServiceName('broca')).toBe('broca');
  });

  it('uses the product name for blank service identities', () => {
    expect(displayServiceName(undefined)).toBe('Kleos');
    expect(displayServiceName('')).toBe('Kleos');
  });
});

describe('display telemetry helpers', () => {
  it('formats counts and tolerates absent values', () => {
    expect(displayCount(1200)).toBe('1,200');
    expect(displayCount(undefined)).toBe('—');
  });

  it('formats timestamps carrying either UTC or explicit offsets', () => {
    expect(displayTime('2026-07-25T12:30:00Z')).not.toBe('--:--');
    expect(displayTime('2026-07-25T12:30:00+00:00')).not.toBe('--:--');
  });
});
