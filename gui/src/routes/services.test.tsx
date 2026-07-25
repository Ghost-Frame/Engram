import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { Axon } from './Axon';
import { Broca } from './Broca';
import { Chiasm } from './Chiasm';
import { Loom } from './Loom';
import { Soma } from './Soma';
import { Thymus } from './Thymus';

const routeData = vi.hoisted(() => ({
  agents: [
    {
      capabilities: [],
      config: {},
      created_at: '',
      drift_flags: [],
      heartbeat_at: '2026-05-30 12:00:00',
      id: 1,
      name: 'codex:architecture',
      quality_score: 0.91,
      status: 'online',
      type: 'cli',
      updated_at: '',
      user_id: 1
    }
  ],
  channels: [{ created_at: '', description: '', event_count: 3, id: 1, name: 'kleos', retain_hours: 24, subscriber_count: 1 }],
  drift: [{ agent: 'codex', created_at: '', drift_type: 'tone', id: 1, severity: 'medium', signal: 'flat output', user_id: 1 }],
  evaluations: [
    {
      agent: 'codex',
      created_at: '2026-05-30 12:00:00',
      evaluator: 'thymus',
      id: 1,
      input: {},
      notes: '',
      output: {},
      overall_score: 0.86,
      rubric_id: 1,
      scores: {},
      subject: 'gui rebuild',
      user_id: 1
    }
  ],
  events: [
    { action: 'task.started', agent: 'codex', channel: 'kleos', created_at: '2026-05-30 12:00:00', id: 1, payload: {}, source: 'ui', user_id: 1 }
  ],
  feed: [
    {
      action: 'task.completed',
      agent: 'codex',
      axon_event_id: 1,
      created_at: '2026-05-30 12:00:00',
      id: 1,
      narrative: 'Phase slice committed',
      payload: {},
      service: 'engram',
      user_id: 1
    }
  ],
  rubrics: [{ created_at: '', criteria: {}, description: 'Quality gates', id: 1, name: 'Release readiness', updated_at: '', user_id: 1 }],
  runs: [
    {
      completed_at: '',
      created_at: '',
      error: '',
      id: 7,
      input: {},
      output: {},
      started_at: '2026-05-30 12:00:00',
      status: 'running',
      updated_at: '',
      user_id: 1,
      workflow_id: 3
    }
  ],
  steps: [
    {
      completed_at: '',
      config: {},
      created_at: '',
      depends_on: [],
      error: '',
      id: 1,
      input: {},
      max_retries: 1,
      name: 'Fetch context',
      output: {},
      retry_count: 0,
      run_id: 7,
      started_at: '',
      status: 'running',
      timeout_ms: 1000,
      type: 'tool'
    }
  ],
  tasks: [
    {
      agent: 'codex',
      assigned: true,
      created_at: '',
      guardrail_retries: 0,
      heartbeat_interval: 300,
      id: 1,
      project: 'Kleos',
      status: 'active',
      summary: 'Build the board',
      title: 'Wire Chiasm',
      updated_at: '',
      user_id: 1
    }
  ]
}));

vi.mock('$lib/realtime', () => ({
  useLive: (key: readonly string[]) => {
    const joined = key.join(':');
    if (joined.startsWith('chiasm:tasks')) return { data: routeData.tasks, isError: false, isLoading: false };
    if (joined.startsWith('broca:feed')) return { data: routeData.feed, isError: false, isLoading: false };
    if (joined.startsWith('soma:agents')) return { data: routeData.agents, isError: false, isLoading: false };
    if (joined.startsWith('loom:runs')) return { data: routeData.runs, isError: false, isLoading: false };
    if (joined.startsWith('axon:events')) return { data: routeData.events, isError: false, isLoading: false };
    if (joined.startsWith('thymus:evals')) return { data: routeData.evaluations, isError: false, isLoading: false };
    return { data: undefined, isError: false, isLoading: false };
  }
}));

vi.mock('@tanstack/react-query', async (importOriginal) => {
  const actual = await importOriginal<typeof import('@tanstack/react-query')>();
  return {
    ...actual,
    useQuery: ({ queryKey }: { queryKey: readonly unknown[] }) => {
      const joined = queryKey.join(':');
      if (joined.startsWith('loom:steps')) return { data: routeData.steps, isError: false, isLoading: false };
      if (joined.startsWith('axon:channels')) return { data: routeData.channels, isError: false, isLoading: false };
      if (joined.startsWith('thymus:drift')) return { data: routeData.drift, isError: false, isLoading: false };
      if (joined.startsWith('thymus:rubrics')) return { data: routeData.rubrics, isError: false, isLoading: false };
      return { data: undefined, isError: false, isLoading: false };
    },
    useQueryClient: () => ({ invalidateQueries: vi.fn() })
  };
});

describe('service routes', () => {
  it('renders the Chiasm task board', () => {
    render(<Chiasm />);

    expect(screen.getByRole('heading', { name: 'Chiasm' })).toBeInTheDocument();
    expect(screen.getByText('Wire Chiasm')).toBeInTheDocument();
  });

  it('renders the Broca action feed', () => {
    render(<Broca />);

    expect(screen.getByRole('heading', { name: 'Broca' })).toBeInTheDocument();
    expect(screen.getByText('Phase slice committed')).toBeInTheDocument();
    expect(screen.getByText('Kleos')).toBeInTheDocument();
    expect(screen.queryByText('engram')).not.toBeInTheDocument();
  });

  it('renders the Soma agent table', () => {
    render(<Soma />);

    expect(screen.getByRole('heading', { name: 'Soma' })).toBeInTheDocument();
    expect(screen.getByText('codex:architecture')).toBeInTheDocument();
  });

  it('renders Loom runs and selected run steps', () => {
    render(<Loom />);

    fireEvent.click(screen.getByRole('button', { name: '#7' }));

    expect(screen.getByRole('heading', { name: 'Loom' })).toBeInTheDocument();
    expect(screen.getByText('Fetch context')).toBeInTheDocument();
  });

  it('renders the Axon event stream', () => {
    render(<Axon />);

    expect(screen.getByRole('heading', { name: 'Axon' })).toBeInTheDocument();
    expect(screen.getByText('task.started')).toBeInTheDocument();
  });

  it('renders Thymus evaluations, drift, and rubrics', () => {
    render(<Thymus />);

    expect(screen.getByRole('heading', { name: 'Thymus' })).toBeInTheDocument();
    expect(screen.getByText('gui rebuild')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Drift' }));
    expect(screen.getByText('flat output')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Rubrics' }));
    expect(screen.getByText('Release readiness')).toBeInTheDocument();
  });
});
