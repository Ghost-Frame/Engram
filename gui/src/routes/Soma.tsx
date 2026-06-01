import { listAgents } from '$lib/api/soma';
import { useLive } from '$lib/realtime';
import { Badge } from '../ui/Badge';
import { Panel } from '../ui/Panel';
import { Spinner } from '../ui/Spinner';
import { Table } from '../ui/Table';

// Render the Soma live-agent table.
export function Soma() {
  const agents = useLive(['soma', 'agents'], () => listAgents(), 'soma');

  // Derive online count and sort online agents first, then by most-recent heartbeat.
  const all = agents.data ?? [];
  const onlineCount = all.filter((a) => a.status === 'online').length;
  const sorted = [...all].sort((a, b) => {
    if ((a.status === 'online') !== (b.status === 'online')) return a.status === 'online' ? -1 : 1;
    return (b.heartbeat_at ?? '').localeCompare(a.heartbeat_at ?? '');
  });

  return (
    <div data-accent="soma">
      <header className="route-header">
        <div>
          <h1>Soma</h1>
          <p>{onlineCount} online of {all.length} registered</p>
        </div>
      </header>
      <Panel title="Live agents">
        {agents.isLoading ? (
          <Spinner />
        ) : (
          <Table
            headers={['Name', 'Type', 'Status', 'Quality', 'Heartbeat']}
            rows={sorted.map((agent) => [
              agent.name,
              agent.type,
              <Badge label={agent.status} tone={agent.status === 'online' ? 'ok' : 'default'} />,
              agent.quality_score != null ? agent.quality_score.toFixed(2) : '--',
              agent.heartbeat_at ? agent.heartbeat_at.slice(11, 19) : '--'
            ])}
          />
        )}
      </Panel>
    </div>
  );
}
