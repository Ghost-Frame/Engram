import { useQuery } from '@tanstack/react-query';
import { listDrift, listEvaluations, listRubrics } from '$lib/api/thymus';
import { useLive } from '$lib/realtime';
import { Badge } from '../ui/Badge';
import { EmptyState } from '../ui/EmptyState';
import { Panel } from '../ui/Panel';
import { Table } from '../ui/Table';
import { Tabs } from '../ui/Tabs';

// Pick a badge tone for a Thymus drift severity.
function severityTone(severity: string) {
  if (severity === 'high') return 'err';
  if (severity === 'medium') return 'warn';
  return 'default';
}

// Render Thymus quality, drift, and rubric tabs.
export function Thymus() {
  const evaluations = useLive(['thymus', 'evals'], () => listEvaluations({ limit: 50 }), 'thymus');
  const drift = useQuery({ queryFn: () => listDrift(), queryKey: ['thymus', 'drift'] });
  const rubrics = useQuery({ queryFn: () => listRubrics(), queryKey: ['thymus', 'rubrics'] });

  return (
    <div data-accent="thymus">
      <header className="route-header">
        <div>
          <h1>Thymus</h1>
          <p>Quality and drift signals</p>
        </div>
      </header>
      <Panel>
        <Tabs
          tabs={[
            {
              content: (
                (evaluations.data ?? []).length === 0 ? (
                  <EmptyState title="No evaluations yet" message="Agent sessions are scored automatically when they end." hint="Quality scores will appear here as sessions complete." />
                ) : (
                  <Table
                    headers={['Agent', 'Subject', 'Score', 'When']}
                    rows={(evaluations.data ?? []).map((item) => [
                      item.agent,
                      item.subject,
                      item.overall_score.toFixed(2),
                      item.created_at.slice(0, 16)
                    ])}
                  />
                )
              ),
              id: 'evals',
              label: 'Evaluations'
            },
            {
              content: (
                (drift.data ?? []).length === 0 ? (
                  <EmptyState title="No drift signals" message="No agent drift has been recorded." />
                ) : (
                  <Table
                    headers={['Agent', 'Type', 'Severity', 'Signal']}
                    rows={(drift.data ?? []).map((item) => [
                      item.agent,
                      item.drift_type,
                      <Badge label={item.severity} tone={severityTone(item.severity)} />,
                      item.signal
                    ])}
                  />
                )
              ),
              id: 'drift',
              label: 'Drift'
            },
            {
              content: (
                <Table
                  headers={['Name', 'Description']}
                  rows={(rubrics.data ?? []).map((item) => [item.name, item.description ?? '--'])}
                />
              ),
              id: 'rubrics',
              label: 'Rubrics'
            }
          ]}
        />
      </Panel>
    </div>
  );
}
