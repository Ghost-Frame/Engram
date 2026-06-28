import { Navigate, NavLink, Route, Routes } from 'react-router-dom';
import { Entities } from './Entities';
import { Graph } from './Graph';
import { Inbox } from './Inbox';
import { Projects } from './Projects';
import { Search } from './Search';
import { Timeline } from './Timeline';
import { MemoryBackground } from '../../ui/effects/MemoryBackground';
import '../../ui/cards3d/cards3d.css';

const TABS = [
  ['search', 'Search'],
  ['timeline', 'Timeline'],
  ['inbox', 'Inbox'],
  ['entities', 'Entities'],
  ['projects', 'Projects'],
  ['graph', 'Graph']
] as const;

// Render the Memory hub, its ambient backdrop, and nested memory-route tabs.
export function Memory() {
  return (
    <div data-accent="memory" style={{ position: 'relative' }}>
      <MemoryBackground />
      <div className="kl-scanlines" />
      <div style={{ position: 'relative', zIndex: 1 }}>
        <header className="route-header">
          <div>
            <h1 className="kl-glitch is-glitching" data-text="Memory">Memory</h1>
            <p>Search, review, and organize stored context</p>
          </div>
        </header>
        <nav aria-label="Memory sections" className="memory-tabs">
          {TABS.map(([to, label]) => (
            // Absolute paths -- relative `to` resolved against the current URL,
            // producing broken links like /memory/graph/search.
            <NavLink className="memory-tabs__link" key={to} to={`/memory/${to}`} end>
              {label}
            </NavLink>
          ))}
        </nav>
        <Routes>
          <Route index element={<Navigate replace to="search" />} />
          <Route path="search" element={<Search />} />
          <Route path="timeline" element={<Timeline />} />
          <Route path="inbox" element={<Inbox />} />
          <Route path="entities" element={<Entities />} />
          <Route path="projects" element={<Projects />} />
          <Route path="graph" element={<Graph />} />
        </Routes>
      </div>
    </div>
  );
}
