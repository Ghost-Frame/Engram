import { useQuery } from '@tanstack/react-query';
import { useEffect, useState } from 'react';
import { NavLink, Outlet, useLocation } from 'react-router';
import { getMe } from '$lib/api/admin';
import { isAuthenticated, loginWithApiKey, onUnauthorized } from '$lib/http';
import { activeOperatorItem, OPERATOR_NAV } from '$lib/services';
import { useStreamStatus } from '$lib/realtime';
import { AuthModal } from './AuthModal';
import { KleosMusic } from '../ui/music/KleosMusic';
import './app.css';

// Admin-only navigation, shown when the caller holds the admin scope.
// `/sharing` (not `/admin/*` or `/spaces`, both reserved for the API) so the
// server's SPA fallback serves this browser route.
const ADMIN_NAV = [{ description: 'Tenancy and access', label: 'Spaces & Sharing', mark: 'SH', route: '/sharing' }];

// Render the persistent dashboard chrome around route content.
export function AppShell() {
  const [authRequired, setAuthRequired] = useState(() => !isAuthenticated());
  const [authOpen, setAuthOpen] = useState(() => !isAuthenticated());
  const [navOpen, setNavOpen] = useState(false);
  const location = useLocation();
  const streamStatus = useStreamStatus();
  const current = location.pathname === '/sharing' ? ADMIN_NAV[0] : activeOperatorItem(location.pathname);
  // Resolve the caller's scopes so the admin nav only renders for admins.
  const me = useQuery({ queryFn: getMe, queryKey: ['me'], retry: false });
  const isAdmin = me.data?.is_admin === true;

  useEffect(() => onUnauthorized(() => {
    setAuthRequired(true);
    setAuthOpen(true);
  }), []);
  useEffect(() => setNavOpen(false), [location.pathname]);

  // Exchange the API key for a cookie session instead of persisting the raw
  // key in localStorage. Keep the modal open on failure so the user can retry.
  const saveApiKey = async (value: string): Promise<string | null> => {
    const result = await loginWithApiKey(value);
    if (!result.ok) {
      return result.error ?? 'Kleos could not start a session.';
    }
    setAuthRequired(false);
    setAuthOpen(false);
    me.refetch();
    return null;
  };

  return (
    <div className="operator-shell">
      <button
        aria-controls="operator-navigation"
        aria-expanded={navOpen}
        aria-label="Toggle navigation"
        className="operator-shell__menu"
        onClick={() => setNavOpen((open) => !open)}
        type="button"
      >
        <span />
        <span />
      </button>
      <aside className={`operator-rail${navOpen ? ' is-open' : ''}`} id="operator-navigation">
        <div className="operator-brand">
          <span aria-hidden="true" className="operator-brand__mark">
            <i />
            <i />
            <i />
          </span>
          <span className="operator-brand__word">
            KLEOS
            <small>operator deck</small>
          </span>
        </div>
        <div className="operator-rail__status">
          <span className={`signal-dot is-${streamStatus}`} />
          <span>{streamStatus === 'live' ? 'Live mesh' : streamStatus}</span>
          <code>4200</code>
        </div>
        <nav aria-label="Primary" className="operator-nav">
          {OPERATOR_NAV.map((group) => (
            <section className="operator-nav__group" key={group.label}>
              <h2>{group.label}</h2>
              {group.items.map((item) => (
                <NavLink className="operator-nav__link" end={item.route === '/'} key={item.route} to={item.route}>
                  <span className="operator-nav__mark">{item.mark}</span>
                  <span>
                    <strong>{item.label}</strong>
                    <small>{item.description}</small>
                  </span>
                </NavLink>
              ))}
            </section>
          ))}
          {isAdmin ? (
            <section className="operator-nav__group">
              <h2>Manage</h2>
              {ADMIN_NAV.map((item) => (
                <NavLink className="operator-nav__link" key={item.route} to={item.route}>
                  <span className="operator-nav__mark">{item.mark}</span>
                  <span>
                    <strong>{item.label}</strong>
                    <small>{item.description}</small>
                  </span>
                </NavLink>
              ))}
            </section>
          ) : null}
        </nav>
        <button className="operator-session" onClick={() => setAuthOpen(true)} type="button">
          <span>
            <strong>{me.data?.username ?? 'Session'}</strong>
            <small>{isAdmin ? 'administrator' : 'authenticated'}</small>
          </span>
          <span aria-hidden="true">•••</span>
        </button>
      </aside>
      <div className="operator-workspace">
        <header className="operator-topbar">
          <div>
            <span className="operator-topbar__eyebrow">{current.mark} / KLEOS</span>
            <strong>{current.label}</strong>
          </div>
          <div className="operator-topbar__signal" aria-label={`Realtime stream ${streamStatus}`}>
            <span className={`signal-dot is-${streamStatus}`} />
            {streamStatus}
          </div>
        </header>
        <main className="operator-main">
          <Outlet />
        </main>
      </div>
      <KleosMusic />
      <AuthModal
        dismissible={!authRequired}
        onClose={() => {
          if (!authRequired) setAuthOpen(false);
        }}
        onSave={saveApiKey}
        open={authOpen}
      />
    </div>
  );
}
