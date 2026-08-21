// Identifies a coordination service rendered by the dashboard.
export type ServiceId = 'chiasm' | 'broca' | 'soma' | 'loom' | 'axon' | 'thymus';

// Describes the route, stats endpoint, and realtime channel for a service.
export interface ServiceDef {
  id: ServiceId;
  label: string;
  route: string;
  statsPath: string;
  channel: string;
}

// Lists the six coordination services in dashboard display order.
export const SERVICES: ServiceDef[] = [
  { id: 'chiasm', label: 'Chiasm', route: '/chiasm', statsPath: '/tasks/stats', channel: 'chiasm' },
  { id: 'broca', label: 'Broca', route: '/broca', statsPath: '/broca/stats', channel: 'broca' },
  { id: 'soma', label: 'Soma', route: '/soma', statsPath: '/soma/stats', channel: 'soma' },
  { id: 'loom', label: 'Loom', route: '/loom', statsPath: '/loom/stats', channel: 'loom' },
  { id: 'axon', label: 'Axon', route: '/axon', statsPath: '/axon/stats', channel: 'axon' },
  { id: 'thymus', label: 'Thymus', route: '/thymus', statsPath: '/thymus/stats', channel: 'thymus' }
];

// Describes one operator-facing navigation destination.
export interface OperatorNavItem {
  description: string;
  label: string;
  mark: string;
  route: string;
}

// Describes a related group of operator destinations.
export interface OperatorNavGroup {
  items: OperatorNavItem[];
  label: string;
}

// Primary navigation is organized by operator intent, not backend ownership.
export const OPERATOR_NAV: OperatorNavGroup[] = [
  {
    label: 'Operate',
    items: [
      { description: 'Live system posture', label: 'Mission Control', mark: 'MC', route: '/' },
      { description: 'Actions and events', label: 'Stream', mark: 'ST', route: '/stream' },
      { description: 'Coordinated tasks', label: 'Work', mark: 'WK', route: '/chiasm' },
      { description: 'Agent presence', label: 'Agents', mark: 'AG', route: '/soma' }
    ]
  },
  {
    label: 'Analyze',
    items: [
      { description: 'Runs and steps', label: 'Workflows', mark: 'WF', route: '/loom' },
      { description: 'Evaluation and drift', label: 'Quality', mark: 'QL', route: '/thymus' },
      { description: 'Recall and curation', label: 'Memory', mark: 'MM', route: '/memory' },
      { description: 'Relationship atlas', label: 'Graph', mark: 'GR', route: '/memory/graph' }
    ]
  }
];

// Resolve the closest navigation destination for a browser pathname.
export function activeOperatorItem(pathname: string): OperatorNavItem {
  const items = OPERATOR_NAV.flatMap((group) => group.items);
  return (
    items
      .filter((item) => item.route === '/' ? pathname === '/' : pathname.startsWith(item.route))
      .sort((left, right) => right.route.length - left.route.length)[0] ?? items[0]
  );
}
