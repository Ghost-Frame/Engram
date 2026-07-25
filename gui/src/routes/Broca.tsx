import { getFeed } from '$lib/api/broca';
import { displayServiceName } from '$lib/display';
import { useLive } from '$lib/realtime';
import { Badge } from '../ui/Badge';
import { EmptyState } from '../ui/EmptyState';
import { Panel } from '../ui/Panel';
import { Spinner } from '../ui/Spinner';

// Render the Broca action feed.
export function Broca() {
  const feed = useLive(['broca', 'feed'], () => getFeed(100), 'broca');

  return (
    <div data-accent="broca">
      <header className="route-header">
        <div>
          <h1>Broca</h1>
          <p>Action narration feed</p>
        </div>
      </header>
      <Panel title="Action feed">
        {feed.isLoading ? (
          <Spinner />
        ) : !feed.data?.length ? (
          <EmptyState />
        ) : (
          <div className="feed-list">
            {feed.data.map((entry) => (
              <article className="feed-row" key={entry.id}>
                <time>{entry.created_at.slice(11, 19)}</time>
                <Badge label={displayServiceName(entry.service)} />
                <span>{entry.agent}</span>
                <strong>{entry.action}</strong>
                {entry.narrative ? <p>{entry.narrative}</p> : null}
              </article>
            ))}
          </div>
        )}
      </Panel>
    </div>
  );
}
