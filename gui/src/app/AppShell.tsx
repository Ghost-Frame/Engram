import { useQuery } from '@tanstack/react-query';
import { useEffect, useState } from 'react';
import { NavLink, Outlet } from 'react-router-dom';
import { getMe } from '$lib/api/admin';
import { isAuthenticated, loginWithApiKey, onUnauthorized } from '$lib/http';
import { SERVICES } from '$lib/services';
import { AuthModal } from './AuthModal';
import { ConnectionDot } from '../ui/ConnectionDot';
import './app.css';

const EXTRA_NAV = [
  { label: 'Memory', route: '/memory' },
  // The real similarity graph lives under Memory; /graph (top level) collides
  // with the server's API-reserved /graph path and cannot be served as a page.
  { label: 'Graph', route: '/memory/graph' }
];

// Admin-only navigation, shown when the caller holds the admin scope.
// `/sharing` (not `/admin/*` or `/spaces`, both reserved for the API) so the
// server's SPA fallback serves this browser route.
const ADMIN_NAV = [{ label: 'Spaces & Sharing', route: '/sharing' }];

// Render the persistent dashboard chrome around route content.
export function AppShell() {
  const [authOpen, setAuthOpen] = useState(() => !isAuthenticated());
  // Resolve the caller's scopes so the admin nav only renders for admins.
  const me = useQuery({ queryFn: getMe, queryKey: ['me'], retry: false });
  const isAdmin = me.data?.is_admin === true;

  useEffect(() => onUnauthorized(() => setAuthOpen(true)), []);

  // Exchange the API key for a cookie session instead of persisting the raw
  // key in localStorage. Keep the modal open on failure so the user can retry.
  const saveApiKey = async (value: string) => {
    if (await loginWithApiKey(value)) {
      setAuthOpen(false);
      me.refetch();
    }
  };

  return (
    <div className="app-shell">
      <aside className="app-shell__rail">
        <div className="app-shell__brand">
          <span>Kleos</span>
          <ConnectionDot />
        </div>
        <button className="app-shell__auth" onClick={() => setAuthOpen(true)} type="button">
          API Key
        </button>
        <nav aria-label="Primary">
          <NavLink className="app-shell__link" end to="/">
            Mission Control
          </NavLink>
          {SERVICES.map((service) => (
            <NavLink className="app-shell__link" key={service.id} to={service.route}>
              {service.label}
            </NavLink>
          ))}
          {EXTRA_NAV.map((item) => (
            <NavLink className="app-shell__link" key={item.route} to={item.route}>
              {item.label}
            </NavLink>
          ))}
          {isAdmin
            ? ADMIN_NAV.map((item) => (
                <NavLink className="app-shell__link" key={item.route} to={item.route}>
                  {item.label}
                </NavLink>
              ))
            : null}
        </nav>
      </aside>
      <main className="app-shell__main">
        <Outlet />
      </main>
      <AuthModal onClose={() => setAuthOpen(false)} onSave={saveApiKey} open={authOpen} />
    </div>
  );
}
