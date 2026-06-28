import type { ReactNode } from 'react';
import './cards3d.css';

// Lay out floating cards in a shared perspective grid. Wrap with .kl-cards3d so
// the vendored --sa-* variable bridge applies to descendants.
export function FloatingCardField({ children }: { children: ReactNode }) {
  return (
    <div className="kl-cards3d">
      <div className="kl-card-field">{children}</div>
    </div>
  );
}
