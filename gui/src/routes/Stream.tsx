import { useQuery } from '@tanstack/react-query';
import { useMemo, useState } from 'react';
import { listChannels, listEvents } from '$lib/api/axon';
import { getFeed } from '$lib/api/broca';
import { displayServiceName, displayTime } from '$lib/display';
import { useLive } from '$lib/realtime';
import type { ActionEntry, AxonEvent } from '$lib/types';
import { EmptyState } from '../ui/EmptyState';
import { Spinner } from '../ui/Spinner';

// Identifies which telemetry sources should be included in the combined stream.
type StreamSource = 'all' | 'actions' | 'events';

// Represents one normalized row in the combined activity stream.
interface StreamItem {
  action: string;
  detail: string;
  id: string;
  source: string;
  timestamp: string;
  type: 'action' | 'event';
}

// Render the combined narrated-action and event-bus timeline.
export function Stream() {
  const [channel, setChannel] = useState('');
  const [source, setSource] = useState<StreamSource>('all');
  const channels = useQuery({ queryFn: listChannels, queryKey: ['stream', 'channels'] });
  const actions = useLive(['stream', 'actions'], () => getFeed(100), 'broca');
  const events = useLive(
    ['stream', 'events', channel],
    () => listEvents({ channel: channel || undefined, limit: 150 }),
    'axon'
  );
  const items = useMemo(
    () => mergeStreamItems(actions.data ?? [], events.data ?? [], source),
    [actions.data, events.data, source]
  );

  return (
    <div className="operator-page">
      <header className="page-heading">
        <div>
          <span className="page-heading__eyebrow">Broca + Axon / unified telemetry</span>
          <h1>Signal Stream</h1>
          <p>Narrated work and raw system events, ordered together without hiding their origin.</p>
        </div>
        <div className="stream-controls">
          <select
            aria-label="Source filter"
            onChange={(event) => setSource(event.target.value as StreamSource)}
            value={source}
          >
            <option value="all">all sources</option>
            <option value="actions">narrated actions</option>
            <option value="events">system events</option>
          </select>
          <select aria-label="Channel filter" onChange={(event) => setChannel(event.target.value)} value={channel}>
            <option value="">all channels</option>
            {(channels.data ?? []).map((item) => (
              <option key={item.id} value={item.name}>
                {item.name} ({item.event_count})
              </option>
            ))}
          </select>
        </div>
      </header>

      <section aria-label="Combined signal stream" className="stream-surface">
        <header className="stream-surface__head">
          <span>time</span>
          <span>source</span>
          <span>signal</span>
          <strong>{items.length} shown</strong>
        </header>
        {actions.isLoading || events.isLoading ? (
          <Spinner />
        ) : items.length === 0 ? (
          <EmptyState message="No signals match the current filters." />
        ) : (
          <div className="stream-list">
            {items.map((item) => (
              <article className="stream-row" data-accent={item.type === 'action' ? 'broca' : 'axon'} key={item.id}>
                <time dateTime={item.timestamp}>{displayTime(item.timestamp)}</time>
                <span className="stream-row__source">{item.source}</span>
                <span>
                  <strong>{item.action}</strong>
                  <small>{item.detail}</small>
                </span>
              </article>
            ))}
          </div>
        )}
      </section>
    </div>
  );
}

// Normalize and sort action and event records into one bounded timeline.
function mergeStreamItems(actions: ActionEntry[], events: AxonEvent[], source: StreamSource): StreamItem[] {
  const actionItems: StreamItem[] = source === 'events'
    ? []
    : actions.map((entry) => ({
        action: entry.action,
        detail: entry.narrative || entry.agent,
        id: `action-${entry.id}`,
        source: displayServiceName(entry.service),
        timestamp: entry.created_at,
        type: 'action'
      }));
  const eventItems: StreamItem[] = source === 'actions'
    ? []
    : events.map((event) => ({
        action: event.action,
        detail: event.agent || event.source || 'system',
        id: `event-${event.id}`,
        source: event.channel,
        timestamp: event.created_at,
        type: 'event'
      }));

  return [...actionItems, ...eventItems]
    .sort((left, right) => right.timestamp.localeCompare(left.timestamp))
    .slice(0, 200);
}
