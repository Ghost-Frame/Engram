import { useState } from 'react';
import type { NewMemoryInput } from '$lib/api/memory';
import type { SearchResult } from '$lib/types';

// Categories suggested in the editor dropdown; the field stays free-text so any
// server-side category remains usable.
const CATEGORIES = [
  'general',
  'fact',
  'preference',
  'decision',
  'task',
  'progress',
  'reference',
  'issue',
  'outcome'
];

// Parse a comma/newline separated tag string into a clean, de-duplicated list.
export function parseTags(input: string): string[] {
  const seen = new Set<string>();
  for (const raw of input.split(/[,\n]/)) {
    const t = raw.trim();
    if (t) seen.add(t);
  }
  return [...seen];
}

// Editable shape backing both the create and edit forms.
export interface MemoryDraft {
  content: string;
  category: string;
  importance: number;
  tags: string;
  isStatic: boolean;
}

// Build an empty draft for the create form.
export function emptyDraft(): MemoryDraft {
  return { content: '', category: 'general', importance: 5, tags: '', isStatic: false };
}

// Project an existing memory into an editable draft.
export function toDraft(m: SearchResult): MemoryDraft {
  return {
    content: m.content,
    category: m.category || 'general',
    importance: m.importance ?? 5,
    tags: (m.tags ?? []).join(', '),
    isStatic: false
  };
}

// Convert a draft into the wire payload accepted by storeMemory/updateMemory.
export function draftToInput(d: MemoryDraft): NewMemoryInput {
  return {
    content: d.content.trim(),
    category: d.category.trim() || 'general',
    importance: d.importance,
    tags: parseTags(d.tags),
    is_static: d.isStatic
  };
}

// Render the shared, controlled editor fields for a memory draft.
export function MemoryFields({
  draft,
  onChange,
  disabled
}: {
  draft: MemoryDraft;
  onChange: (d: MemoryDraft) => void;
  disabled?: boolean;
}) {
  return (
    <div className="kl-memform">
      <textarea
        className="kl-memform__content"
        value={draft.content}
        onChange={(e) => onChange({ ...draft, content: e.target.value })}
        rows={3}
        placeholder="Memory content"
        aria-label="Memory content"
        disabled={disabled}
      />
      <div className="kl-memform__row">
        <label className="kl-memform__field">
          <span>Category</span>
          <input
            list="kl-mem-categories"
            value={draft.category}
            onChange={(e) => onChange({ ...draft, category: e.target.value })}
            aria-label="Category"
            disabled={disabled}
          />
          <datalist id="kl-mem-categories">
            {CATEGORIES.map((c) => (
              <option key={c} value={c} />
            ))}
          </datalist>
        </label>
        <label className="kl-memform__field kl-memform__field--imp">
          <span>Importance</span>
          <input
            type="number"
            min={1}
            max={10}
            value={draft.importance}
            onChange={(e) => onChange({ ...draft, importance: Number(e.target.value) })}
            aria-label="Importance"
            disabled={disabled}
          />
        </label>
        <label className="kl-memform__field kl-memform__field--static">
          <input
            type="checkbox"
            checked={draft.isStatic}
            onChange={(e) => onChange({ ...draft, isStatic: e.target.checked })}
            aria-label="Pinned"
            disabled={disabled}
          />
          <span>Pinned</span>
        </label>
      </div>
      <input
        className="kl-memform__tags"
        value={draft.tags}
        onChange={(e) => onChange({ ...draft, tags: e.target.value })}
        placeholder="tags, comma separated"
        aria-label="Tags"
        disabled={disabled}
      />
    </div>
  );
}

// Render the create-memory panel: shared fields plus save/cancel controls.
export function CreateMemoryForm({
  onSubmit,
  onCancel,
  busy,
  error
}: {
  onSubmit: (input: NewMemoryInput) => void;
  onCancel: () => void;
  busy: boolean;
  error?: string;
}) {
  // Local working copy of the new-memory fields.
  const [draft, setDraft] = useState<MemoryDraft>(emptyDraft);
  // Disable submission until there is non-whitespace content.
  const canSave = draft.content.trim().length > 0 && !busy;

  return (
    <section className="kl-memcard kl-create-panel" aria-label="New memory">
      <MemoryFields draft={draft} onChange={setDraft} disabled={busy} />
      {error ? <p className="kl-memcard__err">{error}</p> : null}
      <p className="kl-memcard__hint">New memories are timestamped now and appear under today.</p>
      <div className="kl-memcard__actions">
        <button disabled={!canSave} onClick={() => onSubmit(draftToInput(draft))}>
          Save
        </button>
        <button disabled={busy} onClick={onCancel}>
          Cancel
        </button>
      </div>
    </section>
  );
}

// Render one stored memory as a card with inline edit and confirm-delete flows.
export function MemoryCard({
  memory,
  index,
  onSave,
  onDelete,
  busy,
  error
}: {
  memory: SearchResult;
  index: number;
  onSave: (input: NewMemoryInput) => void;
  onDelete: () => void;
  busy: boolean;
  error?: string;
}) {
  // view -> normal display; edit -> inline form; confirm -> delete confirmation.
  const [mode, setMode] = useState<'view' | 'edit' | 'confirm'>('view');
  // Working copy used only while the inline editor is open.
  const [draft, setDraft] = useState<MemoryDraft>(() => toDraft(memory));
  // Stagger each card's entrance so the day view animates in like the rest of
  // the timeline rather than appearing as a flat block.
  const delay = (index % 8) * 0.05;
  // Persisted memories carry created_at; trim to minute precision for the meta row.
  const stamp = memory.created_at ? memory.created_at.slice(0, 16) : '';

  // Enter edit mode with a fresh draft snapshot of the current memory.
  const beginEdit = () => {
    setDraft(toDraft(memory));
    setMode('edit');
  };

  return (
    <article
      className={mode === 'view' ? 'kl-memcard' : 'kl-memcard kl-memcard--editing'}
      style={{ ['--kl-delay' as string]: `${delay}s` }}
      data-accent="memory"
    >
      <div className="kl-memcard__meta">
        <span className="kl-memcard__cat">{memory.category}</span>
        <span className="kl-memcard__imp">
          {stamp}
          {memory.importance ? ` · i${memory.importance}` : ''}
        </span>
      </div>

      {mode === 'edit' ? (
        <MemoryFields draft={draft} onChange={setDraft} disabled={busy} />
      ) : (
        <p className="kl-memcard__content">{memory.content}</p>
      )}

      {mode === 'view' && memory.tags && memory.tags.length > 0 ? (
        <div className="kl-memcard__tags">
          {memory.tags.map((t) => (
            <span className="kl-memcard__tag" key={t}>
              {t}
            </span>
          ))}
        </div>
      ) : null}

      {error ? <p className="kl-memcard__err">{error}</p> : null}

      <div className="kl-memcard__actions">
        {mode === 'view' && (
          <>
            <button disabled={busy} onClick={beginEdit}>
              Edit
            </button>
            <button className="danger" disabled={busy} onClick={() => setMode('confirm')}>
              Delete
            </button>
          </>
        )}
        {mode === 'edit' && (
          <>
            <button
              disabled={busy || draft.content.trim().length === 0}
              onClick={() => {
                onSave(draftToInput(draft));
                setMode('view');
              }}
            >
              Save
            </button>
            <button disabled={busy} onClick={() => setMode('view')}>
              Cancel
            </button>
          </>
        )}
        {mode === 'confirm' && (
          <>
            <span className="kl-memcard__confirm">Move to trash?</span>
            <button
              className="danger"
              disabled={busy}
              onClick={() => {
                onDelete();
                setMode('view');
              }}
            >
              Delete
            </button>
            <button disabled={busy} onClick={() => setMode('view')}>
              Cancel
            </button>
          </>
        )}
      </div>
    </article>
  );
}
