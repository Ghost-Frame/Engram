// Represents an unsuccessful HTTP response from the Kleos API.
export class HttpError extends Error {
  // Create an error that preserves the response status and body.
  constructor(
    public status: number,
    public body: string
  ) {
    super(`${status}: ${body}`);
    this.name = 'HttpError';
  }
}

let unauthorized: (() => void) | null = null;

// Register the callback fired when an API request receives 401.
export function onUnauthorized(cb: () => void) {
  unauthorized = cb;
  return () => {
    if (unauthorized === cb) {
      unauthorized = null;
    }
  };
}

// Build the API URL for production same-origin or local Vite proxy mode.
export function buildUrl(path: string, port?: string): string {
  const resolvedPort = port ?? (typeof window !== 'undefined' ? window.location.port : '');
  return `${resolvedPort === '4200' ? '' : '/api'}${path}`;
}

// Describes options accepted by the shared request helper.
export interface RequestOpts {
  method?: string;
  body?: unknown;
  token?: string;
  port?: string;
  signal?: AbortSignal;
}

const MUTATING_METHODS = new Set(['POST', 'PUT', 'PATCH', 'DELETE']);

// Execute a same-origin Kleos API request and parse its JSON response.
//
// Authentication is via the HttpOnly session cookie established by
// `loginWithApiKey` (sent automatically with `credentials: 'same-origin'`).
// Mutating requests additionally echo the session-bound CSRF token from the
// readable `kleos_csrf` cookie in the `X-CSRF-Token` header so the server
// accepts cookie-authenticated writes. An explicit `opts.token` still sends a
// bearer header for direct/dev API use, but no API key is persisted in storage.
export async function request<T>(path: string, opts: RequestOpts = {}): Promise<T> {
  const method = (opts.method ?? 'GET').toUpperCase();
  const headers: Record<string, string> = { 'Content-Type': 'application/json' };
  if (opts.token) {
    headers.Authorization = `Bearer ${opts.token}`;
  }
  if (MUTATING_METHODS.has(method)) {
    const csrf = readCookie('kleos_csrf');
    if (csrf) {
      headers['X-CSRF-Token'] = csrf;
    }
  }

  const res = await fetch(buildUrl(path, opts.port), {
    body: opts.body !== undefined ? JSON.stringify(opts.body) : undefined,
    credentials: 'same-origin',
    headers,
    method,
    signal: opts.signal
  });

  if (res.status === 401) {
    unauthorized?.();
  }
  if (!res.ok) {
    throw new HttpError(res.status, await res.text());
  }

  return res.json() as Promise<T>;
}

// Read a browser cookie value by name (empty string when absent or non-browser).
function readCookie(name: string): string {
  if (typeof document === 'undefined') {
    return '';
  }
  const match = document.cookie.match(new RegExp('(?:^|; )' + name + '=([^;]*)'));
  return match ? decodeURIComponent(match[1]) : '';
}

// Whether a GUI session is established (the readable CSRF cookie is present).
// The session cookie itself is HttpOnly and cannot be read from JS.
export function isAuthenticated(): boolean {
  return readCookie('kleos_csrf') !== '';
}

// Establish a GUI session by exchanging the API key for HttpOnly session +
// readable CSRF cookies via the server's cookie-login endpoint. The raw key is
// never persisted in localStorage. Returns true on success.
export async function loginWithApiKey(apiKey: string): Promise<boolean> {
  const res = await fetch('/gui/auth', {
    method: 'POST',
    credentials: 'same-origin',
    headers: { 'Content-Type': 'application/x-www-form-urlencoded' },
    body: new URLSearchParams({ api_key: apiKey }).toString()
  });
  return res.ok;
}

// Clear the GUI session cookies.
export async function logout(): Promise<void> {
  await fetch('/gui/logout', { method: 'POST', credentials: 'same-origin' });
}
