import { Link } from 'react-router';
import { getAxonStats } from '$lib/api/axon';
import { getBrocaStats, getFeed } from '$lib/api/broca';
import { getChiasmStats, listTasks } from '$lib/api/chiasm';
import { getLoomStats } from '$lib/api/loom';
import { listAgents } from '$lib/api/soma';
import { getThymusStats } from '$lib/api/thymus';
import { displayCount, displayServiceName, displayTime } from '$lib/display';
import { useLive, useStreamStatus } from '$lib/realtime';
import type { Task } from '$lib/types';
import { Badge } from '../ui/Badge';
import { EmptyState } from '../ui/EmptyState';

// Render the operator's cross-service summary and immediate work queues.
export function Overview() {
  const streamStatus = useStreamStatus();
  const tasks = useLive(['overview', 'tasks'], listTasks, 'chiasm');
  const actions = useLive(['overview', 'actions'], () => getFeed(8), 'broca');
  const agents = useLive(['overview', 'agents'], listAgents, 'soma');
  const chiasm = useLive(['stats', 'chiasm'], getChiasmStats, 'chiasm');
  const broca = useLive(['stats', 'broca'], getBrocaStats, 'broca');
  const axon = useLive(['stats', 'axon'], getAxonStats, 'axon');
  const loom = useLive(['stats', 'loom'], getLoomStats, 'loom');
  const thymus = useLive(['stats', 'thymus'], getThymusStats, 'thymus');
  const activeTasks = (tasks.data ?? []).filter((task) => task.status === 'active');
  const queuedTasks = (tasks.data ?? []).filter((task) => task.status === 'pending');
  const onlineAgents = (agents.data ?? []).filter((agent) => agent.status === 'online');

  return (
    <div className="overview">
      <header className="overview__header">
        <div>
          <span className="page-heading__eyebrow">System posture / now</span>
          <h1 className="overview__title">Mission Control</h1>
          <p className="overview__subtle">
            Work, signals, and quality in one operational surface. Exceptions rise; healthy machinery stays quiet.
          </p>
        </div>
        <Badge label={streamStatus === 'live' ? 'mesh live' : streamStatus} tone={streamStatus === 'live' ? 'ok' : 'warn'} />
      </header>

      <section aria-label="System metrics" className="overview__metrics">
        <OverviewMetric
          detail={`${displayCount(queuedTasks.length)} queued`}
          label="active work"
          value={displayCount(activeTasks.length)}
        />
        <OverviewMetric
          detail={`${displayCount(agents.data?.length)} registered`}
          label="agents online"
          value={displayCount(onlineAgents.length)}
        />
        <OverviewMetric
          detail={`${displayCount(axon.data?.channels)} channels`}
          label="signals"
          value={displayCount(axon.data?.total_events)}
        />
        <OverviewMetric
          detail={`${displayCount(thymus.data?.rubrics)} rubrics`}
          label="evaluations"
          value={displayCount(thymus.data?.evaluations)}
        />
      </section>

      <section aria-label="Operational queues" className="overview__grid">
        <OverviewPanel className="overview-panel--work" label="Chiasm" title="Work in motion" to="/chiasm">
          {tasks.isLoading ? (
            <OverviewLoading />
          ) : activeTasks.length === 0 ? (
            <EmptyState message="No active work. The queue is clear." />
          ) : (
            <div className="mission-list">
              {activeTasks.slice(0, 6).map((task) => (
                <article className="mission-row" key={task.id}>
                  <span className="mission-row__id">#{task.id}</span>
                  <span>
                    <strong>{task.title}</strong>
                    <small>{task.project} / {task.agent}</small>
                  </span>
                  <TaskState task={task} />
                </article>
              ))}
            </div>
          )}
        </OverviewPanel>

        <OverviewPanel className="overview-panel--stream" label="Broca + Axon" title="Latest signal" to="/stream">
          {actions.isLoading ? (
            <OverviewLoading />
          ) : !actions.data?.length ? (
            <EmptyState message="No recent narrated actions." />
          ) : (
            <div className="signal-list">
              {actions.data.slice(0, 6).map((entry) => (
                <article className="signal-row" key={entry.id}>
                  <time>{displayTime(entry.created_at)}</time>
                  <span>
                    <strong>{entry.action}</strong>
                    <small>{displayServiceName(entry.service)} / {entry.agent}</small>
                  </span>
                </article>
              ))}
            </div>
          )}
        </OverviewPanel>

        <OverviewPanel className="overview-panel--fleet" label="Soma" title="Agent fleet" to="/soma">
          <div className="fleet-summary">
            <strong>{displayCount(onlineAgents.length)}</strong>
            <span>online now</span>
          </div>
          <div className="fleet-list">
            {(agents.data ?? []).slice(0, 8).map((agent) => (
              <span className="fleet-agent" key={agent.id}>
                <i className={`signal-dot is-${agent.status}`} />
                {agent.name}
                <small>{agent.type}</small>
              </span>
            ))}
          </div>
        </OverviewPanel>

        <OverviewPanel className="overview-panel--quality" label="Loom + Thymus" title="Automation health" to="/thymus">
          <div className="quality-grid">
            <QualityMetric label="workflow runs" value={loom.data?.runs} />
            <QualityMetric label="active runs" value={loom.data?.active_runs} />
            <QualityMetric label="evaluations" value={thymus.data?.evaluations} />
            <QualityMetric label="narrated actions" value={broca.data?.total_actions} />
          </div>
          <p className="quality-note">
            {thymus.isError || loom.isError ? 'One or more quality signals are unavailable.' : 'Quality telemetry is responding.'}
          </p>
        </OverviewPanel>
      </section>
    </div>
  );
}

// Render one metric in the top posture strip.
function OverviewMetric({ detail, label, value }: { detail: string; label: string; value: string }) {
  return (
    <article className="overview-metric">
      <span className="overview-metric__label">{label}</span>
      <strong>{value}</strong>
      <small>{detail}</small>
    </article>
  );
}

// Render a dashboard section with a direct route action.
function OverviewPanel({
  children,
  className,
  label,
  title,
  to
}: {
  children: React.ReactNode;
  className: string;
  label: string;
  title: string;
  to: string;
}) {
  return (
    <article className={`overview-panel ${className}`}>
      <header className="panel-heading">
        <div>
          <small>{label}</small>
          <h2>{title}</h2>
        </div>
        <Link className="panel-heading__link" to={to}>Open →</Link>
      </header>
      {children}
    </article>
  );
}

// Render a task status badge with heartbeat freshness.
function TaskState({ task }: { task: Task }) {
  const hasHeartbeat = Boolean(task.last_heartbeat);
  return <Badge label={hasHeartbeat ? 'active' : 'waiting'} tone={hasHeartbeat ? 'ok' : 'warn'} />;
}

// Render a compact dashboard loading treatment.
function OverviewLoading() {
  return (
    <div aria-label="Loading" className="overview-loading">
      <span className="skeleton" />
      <span className="skeleton" />
      <span className="skeleton" />
    </div>
  );
}

// Render one automation or quality count.
function QualityMetric({ label, value }: { label: string; value: number | undefined }) {
  return (
    <div className="quality-metric">
      <strong>{displayCount(value)}</strong>
      <span>{label}</span>
    </div>
  );
}
