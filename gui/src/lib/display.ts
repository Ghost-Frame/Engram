// Legacy service identities that still occur in persisted activity rows.
const LEGACY_KLEOS_SERVICE_IDS = new Set(['engram', 'engram-rust', 'engram_rust']);

// Convert a backend service identifier into an operator-facing product label.
export function displayServiceName(value: string | null | undefined): string {
  const normalized = value?.trim().toLowerCase();
  if (!normalized) return 'Kleos';
  if (LEGACY_KLEOS_SERVICE_IDS.has(normalized)) return 'Kleos';
  return value!.trim();
}

// Format a timestamp as a compact local time while tolerating missing values.
export function displayTime(value: string | null | undefined): string {
  if (!value) return '--:--';
  const normalized = value.replace(' ', 'T');
  const parsed = new Date(normalized + (/(?:Z|[+-]\d{2}:\d{2})$/i.test(normalized) ? '' : 'Z'));
  if (Number.isNaN(parsed.getTime())) return value.slice(11, 16) || value;
  return new Intl.DateTimeFormat(undefined, {
    hour: '2-digit',
    minute: '2-digit'
  }).format(parsed);
}

// Format a count for dense telemetry without hiding its loading state.
export function displayCount(value: number | null | undefined): string {
  return typeof value === 'number' ? value.toLocaleString() : '—';
}
