import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useState } from 'react';
import {
  deleteMemory,
  getCalendar,
  listMemoriesByDay,
  storeMemory,
  updateMemory,
  // Payload type shared by the create and edit mutations.
  type NewMemoryInput
} from '$lib/api/memory';
import { FloatingCard, FloatingCardField } from '../../ui/cards3d';
import { EmptyState } from '../../ui/EmptyState';
import { Spinner } from '../../ui/Spinner';
import { CreateMemoryForm, MemoryCard } from './MemoryCard';

// Month-number (1-12) to short label for the month cards.
const MONTHS = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];

// Render the year -> month -> day -> memories drill-down.
export function Timeline() {
  // Currently selected year (null = year overview).
  const [year, setYear] = useState<number | null>(null);
  // Currently selected month (null = month overview).
  const [month, setMonth] = useState<number | null>(null);
  // Currently selected day (null = day overview).
  const [day, setDay] = useState<number | null>(null);
  // Whether the inline create-memory panel is open.
  const [creating, setCreating] = useState(false);
  // React Query client used to refresh calendar buckets after a mutation.
  const queryClient = useQueryClient();

  // Refresh every timeline calendar/day-list query after a write. Counts and the
  // visible day list both depend on the corpus, so invalidate the shared prefix.
  const invalidate = () => queryClient.invalidateQueries({ queryKey: ['mem', 'cal'] });

  // Jump the drill-down to today so a freshly created memory is visible (the
  // server stamps new memories now(), regardless of the day being viewed).
  const jumpToToday = () => {
    const now = new Date();
    setYear(now.getFullYear());
    setMonth(now.getMonth() + 1);
    setDay(now.getDate());
  };

  // Create a memory, then close the panel and reveal it under today.
  const create = useMutation({
    mutationFn: (input: NewMemoryInput) => storeMemory(input),
    onSuccess: () => {
      invalidate();
      setCreating(false);
      jumpToToday();
    }
  });
  // Update a memory's editable fields in place.
  const save = useMutation({
    mutationFn: (vars: { id: number; input: NewMemoryInput }) => updateMemory(vars.id, vars.input),
    onSuccess: invalidate
  });
  // Soft-delete a memory (recoverable from the server-side trash).
  const remove = useMutation({
    mutationFn: (id: number) => deleteMemory(id),
    onSuccess: invalidate
  });

  // Fetch the list of years that contain memories.
  const years = useQuery({
    queryFn: () => getCalendar('year'),
    queryKey: ['mem', 'cal', 'year']
  });
  // Fetch month buckets for the selected year.
  const months = useQuery({
    enabled: year !== null,
    queryFn: () => getCalendar('month', year!),
    queryKey: ['mem', 'cal', 'month', year]
  });
  // Fetch day buckets for the selected year and month.
  const days = useQuery({
    enabled: year !== null && month !== null,
    queryFn: () => getCalendar('day', year!, month!),
    queryKey: ['mem', 'cal', 'day', year, month]
  });
  // Fetch individual memories for the selected year, month, and day.
  const memories = useQuery({
    enabled: year !== null && month !== null && day !== null,
    queryFn: () => listMemoriesByDay(year!, month!, day!),
    queryKey: ['mem', 'cal', 'list', year, month, day]
  });

  // Build a count lookup keyed by zero-padded bucket for month/day fields.
  const monthCount = (m: number) =>
    Number(months.data?.find((b) => Number(b.bucket) === m)?.count ?? 0);
  const dayCount = (d: number) =>
    Number(days.data?.find((b) => Number(b.bucket) === d)?.count ?? 0);

  // Total number of days in the currently selected month.
  const daysInMonth = year !== null && month !== null ? new Date(year, month, 0).getDate() : 0;

  // Surface a top-level error instead of an empty/misleading year view.
  if (years.isError) return <EmptyState message="Failed to load timeline. Try refreshing." />;

  return (
    <div className="memory-view">
      <nav className="kl-breadcrumb" aria-label="Timeline position">
        <button onClick={() => { setYear(null); setMonth(null); setDay(null); }}>Timeline</button>
        {year !== null && (
          <>
            <span className="sep">/</span>
            <button onClick={() => { setMonth(null); setDay(null); }}>{year}</button>
          </>
        )}
        {month !== null && (
          <>
            <span className="sep">/</span>
            <button onClick={() => setDay(null)}>{MONTHS[month - 1]}</button>
          </>
        )}
        {day !== null && (
          <>
            <span className="sep">/</span>
            <span>{day}</span>
          </>
        )}
      </nav>

      <div className="kl-timeline-toolbar">
        <button
          className="kl-new-memory"
          onClick={() => setCreating((open) => !open)}
          aria-expanded={creating}
        >
          {creating ? 'Close' : '+ New memory'}
        </button>
      </div>

      {creating && (
        <CreateMemoryForm
          onSubmit={(input) => create.mutate(input)}
          onCancel={() => setCreating(false)}
          busy={create.isPending}
          error={create.isError ? 'Could not create memory. Try again.' : undefined}
        />
      )}

      {/* Year level */}
      {year === null &&
        (years.isLoading ? (
          <Spinner />
        ) : years.data && years.data.length > 0 ? (
          <FloatingCardField>
            {years.data.map((b, i) => (
              <FloatingCard
                key={b.bucket}
                title={b.bucket}
                count={b.count}
                index={i}
                onClick={() => setYear(Number(b.bucket))}
              />
            ))}
          </FloatingCardField>
        ) : (
          <EmptyState message="No memories yet." />
        ))}

      {/* Month level */}
      {year !== null && month === null &&
        (months.isLoading ? (
          <Spinner />
        ) : months.isError ? (
          <EmptyState message="Failed to load. Try refreshing." />
        ) : (
          <FloatingCardField>
            {MONTHS.map((label, idx) => {
              const m = idx + 1;
              const c = monthCount(m);
              return (
                <FloatingCard
                  key={label}
                  title={label}
                  count={c}
                  index={idx}
                  isEmpty={c === 0}
                  onClick={() => setMonth(m)}
                />
              );
            })}
          </FloatingCardField>
        ))}

      {/* Day level */}
      {year !== null && month !== null && day === null &&
        (days.isLoading ? (
          <Spinner />
        ) : days.isError ? (
          <EmptyState message="Failed to load. Try refreshing." />
        ) : (
          <FloatingCardField>
            {Array.from({ length: daysInMonth }, (_, i) => i + 1).map((d) => {
              const c = dayCount(d);
              return (
                <FloatingCard
                  key={d}
                  title={String(d)}
                  count={c}
                  index={d - 1}
                  isEmpty={c === 0}
                  onClick={() => setDay(d)}
                />
              );
            })}
          </FloatingCardField>
        ))}

      {/* Memories level */}
      {day !== null &&
        (memories.isLoading ? (
          <Spinner />
        ) : memories.isError ? (
          <EmptyState message="Failed to load. Try refreshing." />
        ) : memories.data && memories.data.length > 0 ? (
          <div className="kl-memcard-field">
            {memories.data.map((m, i) => (
              <MemoryCard
                key={m.id}
                memory={m}
                index={i}
                onSave={(input) => save.mutate({ id: m.id, input })}
                onDelete={() => remove.mutate(m.id)}
                // Global lock -- one write at a time across the whole day view.
                busy={save.isPending || remove.isPending}
                error={
                  (save.isError && save.variables?.id === m.id) ||
                  (remove.isError && remove.variables === m.id)
                    ? 'Action failed. Try again.'
                    : undefined
                }
              />
            ))}
          </div>
        ) : (
          <EmptyState message="No memories on this day." hint="Use + New memory to add one." />
        ))}
    </div>
  );
}
