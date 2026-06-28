import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useState } from 'react';
import { approveInbox, editInbox, getInbox, rejectInbox } from '$lib/api/memory';
import type { InboxItem } from '$lib/types';
import { EmptyState } from '../../ui/EmptyState';
import { Spinner } from '../../ui/Spinner';

// Render the pending-memory inbox with approve / reject / edit actions.
export function Inbox() {
  // React Query client used to invalidate stale caches after mutations.
  const queryClient = useQueryClient();
  // Fetch up to 50 pending inbox items.
  const inbox = useQuery({ queryFn: () => getInbox(50), queryKey: ['mem', 'inbox'] });

  // Invalidate the inbox and the timeline list after any action.
  const invalidate = () => {
    queryClient.invalidateQueries({ queryKey: ['mem', 'inbox'] });
    queryClient.invalidateQueries({ queryKey: ['mem', 'cal'] });
  };

  // Wrap in a lambda so only the id is forwarded -- useMutation injects a
  // second context argument that the spy in tests would otherwise see.
  // Mutation to approve a pending inbox item by id.
  const approve = useMutation({ mutationFn: (id: number) => approveInbox(id), onSuccess: invalidate });
  // Mutation to reject a pending inbox item with an optional reason.
  const reject = useMutation({
    mutationFn: (vars: { id: number; reason?: string }) => rejectInbox(vars.id, vars.reason),
    onSuccess: invalidate
  });
  // Mutation to update the content of a pending inbox item.
  const edit = useMutation({
    mutationFn: (vars: { id: number; content: string }) => editInbox(vars.id, vars.content),
    onSuccess: invalidate
  });

  if (inbox.isLoading) {
    return <Spinner />;
  }
  if (inbox.isError) {
    return <EmptyState message="Failed to load inbox. Try refreshing." />;
  }
  if (!inbox.data?.length) {
    return <EmptyState message="Inbox empty." hint="Pending memories awaiting review appear here." />;
  }

  return (
    <div className="memory-list">
      {inbox.data.map((item) => (
        <InboxCard
          key={item.id}
          item={item}
          onApprove={() => approve.mutate(item.id)}
          onReject={(reason) => reject.mutate({ id: item.id, reason })}
          onEdit={(content) => edit.mutate({ id: item.id, content })}
          // Global lock -- disables all actions while any mutation is in flight.
          busy={approve.isPending || reject.isPending || edit.isPending}
        />
      ))}
    </div>
  );
}

// Render one pending memory with its action controls and inline edit field.
function InboxCard({
  item,
  onApprove,
  onReject,
  onEdit,
  busy
}: {
  item: InboxItem;
  onApprove: () => void;
  onReject: (reason?: string) => void;
  onEdit: (content: string) => void;
  busy: boolean;
}) {
  // Whether the inline editor is currently open.
  const [editing, setEditing] = useState(false);
  // Editable copy of the memory content while the editor is open.
  const [draft, setDraft] = useState(item.content);

  return (
    <article className="glass memory-card" data-accent="memory">
      <div className="memory-card__meta">
        <span>{item.category}</span>
        <span>{item.created_at.slice(0, 16)}</span>
      </div>
      {editing ? (
        <textarea
          className="kl-inbox-edit"
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          rows={3}
          aria-label="Edit memory content"
        />
      ) : (
        <p>{item.content}</p>
      )}
      <div className="kl-inbox-actions">
        {editing ? (
          <>
            <button disabled={busy} onClick={() => { onEdit(draft); setEditing(false); }}>Save</button>
            <button disabled={busy} onClick={() => { setDraft(item.content); setEditing(false); }}>Cancel</button>
          </>
        ) : (
          <>
            <button disabled={busy} onClick={onApprove}>Approve</button>
            <button disabled={busy} onClick={() => onReject()}>Reject</button>
            <button disabled={busy} onClick={() => setEditing(true)}>Edit</button>
          </>
        )}
      </div>
    </article>
  );
}
