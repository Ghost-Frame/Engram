import { useQuery } from '@tanstack/react-query';
import { useMemo, useState } from 'react';
import { listChannels, listEvents } from '$lib/api/axon';
import { useLive } from '$lib/realtime';
import { Badge } from '../ui/Badge';
import { EmptyState } from '../ui/EmptyState';
import { Panel } from '../ui/Panel';
import { Spinner } from '../ui/Spinner';

// Render the Axon event stream with a channel filter.
export function Axon() {
  const [channel, setChannel] = useState('');
  const channels = useQuery({ queryFn: () => listChannels(), queryKey: ['axon', 'channels'] });
  const eventKey = useMemo(() => ['axon', 'events', channel] as const, [channel]);
  const events = useLive(eventKey, () => listEvents({ channel: channel || undefined, limit: 150 }), 'axon');

  return (
    <div data-accent="axon">
      <header className="route-header">
        <div>
          <h1>Axon</h1>
          <p>Event bus stream</p>
        </div>
        <select aria-label="Channel filter" onChange={(event) => setChannel(event.target.value)} value={channel}>
          <option value="">all channels</option>
          {(channels.data ?? []).map((item) => (
            <option key={item.id} value={item.name}>
              {item.name} ({item.event_count})
            </option>
          ))}
        </select>
      </header>
      <Panel title="Events">
        {(channels.data ?? []).length > 0 ? (
          <div className="feed-row" style={{ flexWrap: 'wrap', gap: 'var(--sp-2)', marginBottom: 'var(--sp-3)' }}>
            {(channels.data ?? []).map((c) => (
              <button
                key={c.id}
                className="link-button"
                onClick={() => setChannel(c.name)}
                style={{ display: 'inline-flex', gap: 6 }}
              >
                <Badge label={c.name} />
                <span>{c.event_count}</span>
              </button>
            ))}
          </div>
        ) : null}
        {events.isLoading ? (
          <Spinner />
        ) : !events.data?.length ? (
          <EmptyState />
        ) : (
          <div className="feed-list">
            {events.data.map((event) => (
              <article className="feed-row" key={event.id}>
                <time>{event.created_at.slice(11, 19)}</time>
                <Badge label={event.channel} />
                <strong>{event.action}</strong>
                {event.source ? <span>{event.source}</span> : null}
              </article>
            ))}
          </div>
        )}
      </Panel>
    </div>
  );
}
