import type { ReactNode } from 'react';
import './cards3d.css';

// Props for one floating, breathing 3D-perspective descriptor card.
export interface FloatingCardProps {
  title: string;
  subtitle?: string;
  count?: number;
  accent?: string;
  isEmpty?: boolean;
  badges?: ReactNode;
  // Reserved for a future backface flip; not yet rendered.
  back?: ReactNode;
  onClick?: () => void;
  index?: number;
}

// Render a single floating card. `index` staggers the breathe animation and
// gives a stable per-card perspective lean so a field reads as 3D space.
export function FloatingCard({
  title,
  subtitle,
  count,
  accent,
  isEmpty,
  badges,
  onClick,
  index = 0
}: FloatingCardProps) {
  // Derive a deterministic tilt and animation delay from the card's position.
  const tilt = ((index % 5) - 2) * 3;
  const delay = (index % 6) * 0.4;
  const handleClick = () => {
    if (!isEmpty) {
      onClick?.();
    }
  };
  return (
    <div
      className={['kl-card', isEmpty ? 'is-empty' : ''].filter(Boolean).join(' ')}
      data-accent={accent}
      style={{ ['--kl-tilt' as string]: `${tilt}deg`, ['--kl-delay' as string]: `${delay}s` }}
      onClick={handleClick}
      onKeyDown={(e) => {
        if (!isEmpty && (e.key === 'Enter' || e.key === ' ')) {
          e.preventDefault();
          onClick?.();
        }
      }}
      role="button"
      tabIndex={isEmpty ? -1 : 0}
      aria-label={title}
    >
      <div className="kl-card__body">
        <span className="kl-card__title">{title}</span>
        {subtitle ? <span className="kl-card__subtitle">{subtitle}</span> : null}
        {badges ? <div className="kl-card__badges">{badges}</div> : null}
        {count !== undefined ? <span className="kl-card__count">{count}</span> : null}
      </div>
    </div>
  );
}
