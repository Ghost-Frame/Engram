import '@testing-library/jest-dom/vitest';
import { vi } from 'vitest';

// jsdom has no canvas renderer; returning null exercises the production
// fallback without emitting a not-implemented exception for every app test.
HTMLCanvasElement.prototype.getContext = vi.fn(() => null);

// jsdom has no media transport; player cleanup only needs a harmless pause.
HTMLMediaElement.prototype.pause = vi.fn();

// jsdom does not implement window.matchMedia -- provide a minimal stub so
// components that read media queries do not throw in the test environment.
if (typeof window !== 'undefined' && !window.matchMedia) {
  window.matchMedia = (query: string): MediaQueryList =>
    ({
      matches: false,
      media: query,
      onchange: null,
      addListener: () => {},
      removeListener: () => {},
      addEventListener: () => {},
      removeEventListener: () => {},
      dispatchEvent: () => false,
    }) as MediaQueryList;
}

// Provide a controllable default EventSource shape for tests that replace it.
if (!('EventSource' in globalThis)) {
  // @ts-expect-error jsdom does not ship EventSource.
  globalThis.EventSource = class {
    // Close is intentionally empty because tests install their own behavior.
    close() {}

    // Listener registration is intentionally empty for the default stub.
    addEventListener() {}
  } as unknown as typeof EventSource;
}
