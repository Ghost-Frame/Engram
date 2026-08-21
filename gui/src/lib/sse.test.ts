import { beforeEach, describe, expect, it, vi } from 'vitest';
import { AxonStream } from './sse';

// Provides a controllable EventSource implementation for stream tests.
class FakeEventSource {
  static last: FakeEventSource;
  url: string;
  onopen: ((event: Event) => void) | null = null;
  onerror: ((event: Event) => void) | null = null;
  listeners: Record<string, Array<(event: MessageEvent) => void>> = {};

  // Capture the URL from the stream under test.
  constructor(url: string) {
    this.url = url;
    FakeEventSource.last = this;
  }

  // Register a handler for a named SSE event.
  addEventListener(type: string, callback: (event: MessageEvent) => void) {
    (this.listeners[type] ??= []).push(callback);
  }

  // Remove a handler for a named SSE event.
  removeEventListener(type: string, callback: (event: MessageEvent) => void) {
    this.listeners[type] = (this.listeners[type] ?? []).filter((entry) => entry !== callback);
  }

  // Close is a no-op for controllable tests.
  close() {}

  // Emit one test event to registered listeners.
  emit(type: string, data: string) {
    (this.listeners[type] ?? []).forEach((callback) => callback({ data } as MessageEvent));
  }
}

describe('AxonStream', () => {
  beforeEach(() => {
    vi.stubGlobal('EventSource', FakeEventSource as unknown as typeof EventSource);
  });

  it('builds the stream URL with an agent', () => {
    new AxonStream('kleos-gui', false).connect();

    expect(FakeEventSource.last.url).toBe('/axon/stream?agent=kleos-gui');
  });

  it('routes default message events by channel', () => {
    const stream = new AxonStream('kleos-gui', false);
    const seen: string[] = [];
    stream.onChannel('chiasm', (event) => seen.push(event.action));

    stream.connect();
    FakeEventSource.last.emit(
      'message',
      JSON.stringify({ action: 'task.created', channel: 'chiasm', created_at: 'x', id: 1, payload: {}, user_id: 1 })
    );

    expect(seen).toEqual(['task.created']);
  });

  it('routes custom action events by channel', () => {
    const stream = new AxonStream('kleos-gui', false);
    const seen: string[] = [];
    stream.onChannel('broca', (event) => seen.push(event.action));

    stream.connect();
    FakeEventSource.last.emit(
      'task.progress',
      JSON.stringify({ action: 'task.progress', channel: 'broca', created_at: 'x', id: 2, payload: {}, user_id: 1 })
    );

    expect(seen).toEqual(['task.progress']);
  });
});
