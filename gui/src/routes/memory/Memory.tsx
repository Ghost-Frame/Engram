import { Navigate, NavLink, Route, Routes } from 'react-router-dom';
import { Entities } from './Entities';
import { Graph } from './Graph';
import { Inbox } from './Inbox';
import { Projects } from './Projects';
import { Search } from './Search';
import { Timeline } from './Timeline';

const TABS = [
  ['search', 'Search'],
  ['timeline', 'Timeline'],
  ['inbox', 'Inbox'],
  ['entities', 'Entities'],
  ['projects', 'Projects'],
  ['graph', 'Graph']
] as const;

// Render the Memory hub and its nested memory-route tabs.
export function Memory() {
  return (
    <div data-accent="memory">
      <header className="route-header">
        <div>
          <h1>Memory</h1>
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
  );
}
