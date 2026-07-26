import { useEffect, useRef, useState } from 'react';
import type { FormEvent, KeyboardEvent } from 'react';

// Render the modal used to save a bearer API key for same-origin API calls.
export function AuthModal({
  dismissible,
  onClose,
  onSave,
  open
}: {
  dismissible: boolean;
  onClose: () => void;
  onSave: (value: string) => Promise<string | null>;
  open: boolean;
}) {
  const [error, setError] = useState('');
  const [pending, setPending] = useState(false);
  const [value, setValue] = useState('');
  const panelRef = useRef<HTMLFormElement>(null);
  const restoreFocusRef = useRef<HTMLElement | null>(null);

  // Restore focus to the control that opened the modal after it closes.
  useEffect(() => {
    if (!open) return undefined;
    restoreFocusRef.current = document.activeElement instanceof HTMLElement
      ? document.activeElement
      : null;
    return () => restoreFocusRef.current?.focus();
  }, [open]);

  if (!open) {
    return null;
  }

  // Await cookie login and preserve the entered key whenever authentication fails.
  const submit = async (event: FormEvent) => {
    event.preventDefault();
    const trimmed = value.trim();
    if (!trimmed || pending) return;
    setError('');
    setPending(true);
    try {
      const failure = await onSave(trimmed);
      if (failure) {
        setError(failure);
      } else {
        setValue('');
      }
    } catch {
      setError('Kleos is unreachable. Check the connection and try again.');
    } finally {
      setPending(false);
    }
  };

  // Clear transient modal state when an authenticated user dismisses it.
  const close = () => {
    if (!dismissible || pending) return;
    setError('');
    setValue('');
    onClose();
  };

  // Trap focus inside the modal and support Escape when dismissal is allowed.
  const handleKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (event.key === 'Escape') {
      event.preventDefault();
      close();
      return;
    }
    if (event.key !== 'Tab') return;
    const focusable = [...(panelRef.current?.querySelectorAll<HTMLElement>(
      'input:not(:disabled), button:not(:disabled)'
    ) ?? [])];
    if (focusable.length === 0) return;
    const first = focusable[0];
    const last = focusable[focusable.length - 1];
    if (event.shiftKey && document.activeElement === first) {
      event.preventDefault();
      last?.focus();
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault();
      first?.focus();
    }
  };

  return (
    <div
      aria-describedby={error ? 'auth-modal-error' : undefined}
      aria-labelledby="auth-modal-title"
      aria-modal="true"
      className="auth-modal"
      onKeyDown={handleKeyDown}
      role="dialog"
    >
      <form aria-busy={pending} className="auth-modal__panel" onSubmit={submit} ref={panelRef}>
        <h2 id="auth-modal-title">API Key</h2>
        <label className="auth-modal__label" htmlFor="kleos-api-key">
          API key
        </label>
        <input
          aria-invalid={error ? true : undefined}
          autoFocus
          disabled={pending}
          id="kleos-api-key"
          onChange={(event) => {
            setValue(event.target.value);
            if (error) setError('');
          }}
          type="password"
          value={value}
        />
        {error ? <p className="auth-modal__error" id="auth-modal-error" role="alert">{error}</p> : null}
        <div className="auth-modal__actions">
          <button className="auth-modal__save" disabled={pending || !value.trim()} type="submit">
            {pending ? 'Connecting…' : 'Save'}
          </button>
          {dismissible ? (
            <button className="auth-modal__cancel" disabled={pending} onClick={close} type="button">
              Cancel
            </button>
          ) : null}
        </div>
      </form>
    </div>
  );
}
