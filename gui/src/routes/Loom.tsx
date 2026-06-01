import { useQuery } from '@tanstack/react-query';
import { useState } from 'react';
import { getSteps, listRuns } from '$lib/api/loom';
import { useLive } from '$lib/realtime';
import { Badge } from '../ui/Badge';
import { EmptyState } from '../ui/EmptyState';
import { Panel } from '../ui/Panel';
import { Spinner } from '../ui/Spinner';
import { Table } from '../ui/Table';

// Pick a badge tone for a Loom run or step status.
function statusTone(status: string) {
  if (status === 'completed') return 'ok';
  if (status === 'failed') return 'err';
  if (status === 'running') return 'warn';
  return 'default';
}

// Render Loom workflow runs and selected run steps.
export function Loom() {
  const runs = useLive(['loom', 'runs'], () => listRuns(), 'loom');
  const [selectedRun, setSelectedRun] = useState<number | null>(null);
  const steps = useQuery({
    enabled: selectedRun != null,
    queryFn: () => getSteps(selectedRun!),
    queryKey: ['loom', 'steps', selectedRun]
  });

  return (
    <div data-accent="loom">
      <header className="route-header">
        <div>
          <h1>Loom</h1>
          <p>Workflow runs and steps</p>
        </div>
      </header>
      <div className="two-pane">
        <Panel title="Runs">
          {runs.isLoading ? (
            <Spinner />
          ) : (runs.data ?? []).length === 0 ? (
            <EmptyState title="No workflow runs" message="No Loom workflows have executed yet." />
          ) : (
            <Table
              headers={['Run', 'Workflow', 'Status', 'Started']}
              rows={(runs.data ?? []).map((run) => [
                <button className="link-button" onClick={() => setSelectedRun(run.id)}>
                  #{run.id}
                </button>,
                run.workflow_id,
                <Badge label={run.status} tone={statusTone(run.status)} />,
                run.started_at?.slice(11, 19) ?? '--'
              ])}
            />
          )}
        </Panel>
        <Panel title={selectedRun ? `Steps run #${selectedRun}` : 'Steps'}>
          {selectedRun == null ? (
            <EmptyState message="Select a run." />
          ) : steps.isLoading ? (
            <Spinner />
          ) : (
            <Table
              headers={['Step', 'Type', 'Status']}
              rows={(steps.data ?? []).map((step) => [
                step.name,
                step.type,
                <Badge label={step.status} tone={statusTone(step.status)} />
              ])}
            />
          )}
        </Panel>
      </div>
    </div>
  );
}
