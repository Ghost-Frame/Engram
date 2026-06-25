import { beforeEach, describe, expect, it, vi } from 'vitest';
import { HttpError, buildUrl, onUnauthorized, request } from './http';

describe('buildUrl', () => {
  it('uses same-origin paths on the production server port', () => {
    expect(buildUrl('/x', '4200')).toBe('/x');
  });

  it('uses the dev proxy away from the production server port', () => {
    expect(buildUrl('/x', '5173')).toBe('/api/x');
  });
});

describe('request', () => {
  beforeEach(() => {
    localStorage.clear();
    // Clear cookies so CSRF assertions are isolated between tests.
    for (const c of document.cookie.split(';')) {
      const name = c.split('=')[0].trim();
      if (name) {
        document.cookie = `${name}=; expires=Thu, 01 Jan 1970 00:00:00 GMT`;
      }
    }
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });

  it('parses json responses', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn(async () => new Response('{"a":1}', { headers: { 'content-type': 'application/json' }, status: 200 }))
    );

    expect(await request<{ a: number }>('/x', { port: '4200' })).toEqual({ a: 1 });
  });

  it('throws HttpError on server errors', async () => {
    vi.stubGlobal('fetch', vi.fn(async () => new Response('no', { status: 500 })));

    await expect(request('/x', { port: '4200' })).rejects.toBeInstanceOf(HttpError);
  });

  it('calls the unauthorized handler on 401', async () => {
    const cb = vi.fn();
    onUnauthorized(cb);
    vi.stubGlobal('fetch', vi.fn(async () => new Response('', { status: 401 })));

    await expect(request('/x', { port: '4200' })).rejects.toBeInstanceOf(HttpError);
    expect(cb).toHaveBeenCalledOnce();
  });

  it('sends bearer auth when a token is provided', async () => {
    const spy = vi.fn(
      async () => new Response('{}', { headers: { 'content-type': 'application/json' }, status: 200 })
    );
    vi.stubGlobal('fetch', spy);

    await request('/x', { port: '4200', token: 'abc' });

    const calls = spy.mock.calls as unknown as Array<[RequestInfo | URL, RequestInit]>;
    expect(calls[0][1].headers).toMatchObject({ Authorization: 'Bearer abc' });
  });

  it('does not send a bearer header by default (cookie auth)', async () => {
    const spy = vi.fn(
      async () => new Response('{}', { headers: { 'content-type': 'application/json' }, status: 200 })
    );
    // A stale localStorage value must NOT be used: auth is via the cookie now.
    localStorage.setItem('kleos_api_key', 'stored');
    vi.stubGlobal('fetch', spy);

    await request('/x', { port: '4200' });

    const calls = spy.mock.calls as unknown as Array<[RequestInfo | URL, RequestInit]>;
    expect((calls[0][1].headers as Record<string, string>).Authorization).toBeUndefined();
  });

  it('echoes the kleos_csrf cookie as X-CSRF-Token on mutating requests', async () => {
    const spy = vi.fn(
      async () => new Response('{}', { headers: { 'content-type': 'application/json' }, status: 200 })
    );
    document.cookie = 'kleos_csrf=tok123';
    vi.stubGlobal('fetch', spy);

    await request('/x', { method: 'POST', port: '4200' });

    const calls = spy.mock.calls as unknown as Array<[RequestInfo | URL, RequestInit]>;
    expect(calls[0][1].headers).toMatchObject({ 'X-CSRF-Token': 'tok123' });
  });

  it('does not send X-CSRF-Token on safe GET requests', async () => {
    const spy = vi.fn(
      async () => new Response('{}', { headers: { 'content-type': 'application/json' }, status: 200 })
    );
    document.cookie = 'kleos_csrf=tok123';
    vi.stubGlobal('fetch', spy);

    await request('/x', { port: '4200' });

    const calls = spy.mock.calls as unknown as Array<[RequestInfo | URL, RequestInit]>;
    expect((calls[0][1].headers as Record<string, string>)['X-CSRF-Token']).toBeUndefined();
  });
});
