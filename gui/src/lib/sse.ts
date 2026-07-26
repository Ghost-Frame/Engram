import { buildUrl } from './http';
import type { AxonEvent } from './types';

// Handles one parsed Axon stream event.
type Handler = (event: AxonEvent) => void;

// Represents the observable connection state for the Axon stream.
export type StreamStatus = 'connecting' | 'live' | 'down';

const ACTION_EVENT_NAMES = [
  'message',
  'task.started',
  'task.progress',
  'task.completed',
  'task.blocked',
  'error.raised',
  'tool.completed',
  'memory.created',
  'memory.updated',
  'memory.deleted',
  'agent.heartbeat',
  'workflow.started',
  'workflow.completed',
  'evaluation.recorded'
] as const;

// Maintains a single EventSource connection to Axon and dispatches by channel.
export class AxonStream {
  private es: EventSource | null = null;
  private handlers = new Map<string, Set<Handler>>();
  private statusCbs = new Set<(status: StreamStatus) => void>();

  // Create a stream for the given agent identity and optional development proxy override.
  constructor(
    private agent: string,
    private useDevProxy?: boolean
  ) {}

  // Register a callback for events on one Axon channel or source key.
  onChannel(channel: string, handler: Handler): () => void {
    const set = this.handlers.get(channel) ?? new Set<Handler>();
    set.add(handler);
    this.handlers.set(channel, set);
    return () => set.delete(handler);
  }

  // Register a callback for stream status changes.
  onStatus(callback: (status: StreamStatus) => void): () => void {
    this.statusCbs.add(callback);
    return () => this.statusCbs.delete(callback);
  }

  // Connect the EventSource and subscribe to default plus known action events.
  connect() {
    this.emitStatus('connecting');
    const url = buildUrl(`/axon/stream?agent=${encodeURIComponent(this.agent)}`, this.useDevProxy);
    const es = new EventSource(url, { withCredentials: true });
    this.es = es;
    es.onopen = () => this.emitStatus('live');
    es.onerror = () => this.emitStatus('down');

    const listener = (event: MessageEvent) => this.dispatch(event.data);
    for (const name of ACTION_EVENT_NAMES) {
      es.addEventListener(name, listener);
    }
  }

  // Close the EventSource connection.
  close() {
    this.es?.close();
    this.es = null;
  }

  // Notify all status subscribers.
  private emitStatus(status: StreamStatus) {
    this.statusCbs.forEach((callback) => callback(status));
  }

  // Parse and dispatch one raw event payload to channel/source subscribers.
  private dispatch(raw: string) {
    let event: AxonEvent;
    try {
      event = JSON.parse(raw) as AxonEvent;
    } catch {
      return;
    }

    const key = event.source || event.channel;
    this.handlers.get(key)?.forEach((handler) => handler(event));
    if (key !== event.channel) {
      this.handlers.get(event.channel)?.forEach((handler) => handler(event));
    }
  }
}
