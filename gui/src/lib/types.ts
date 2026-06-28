// Represents a count grouped by a named bucket.
export interface StatBreakdown {
  name: string;
  count: number;
}

// Represents task coordination stats returned by Chiasm.
export interface ChiasmStats {
  total: number;
  by_status: Record<string, number>;
}

// Represents one Chiasm task.
export interface Task {
  id: number;
  agent: string;
  project: string;
  title: string;
  status: string;
  summary?: string;
  expected_output?: string;
  output_format?: string;
  output?: string;
  condition?: string;
  guardrail_url?: string;
  guardrail_retries: number;
  plan?: string;
  feedback?: string;
  last_heartbeat?: string;
  heartbeat_interval: number;
  assigned: boolean;
  created_at: string;
  updated_at: string;
  user_id: number;
}

// Represents Broca action-log stats.
export interface BrocaStats {
  total_actions: number;
  agents: number;
  services: number;
  by_service: StatBreakdown[];
  by_agent: StatBreakdown[];
  by_action: StatBreakdown[];
}

// Represents one Broca action entry.
export interface ActionEntry {
  id: number;
  agent: string;
  service: string;
  action: string;
  payload: unknown;
  narrative?: string;
  axon_event_id?: number;
  user_id: number;
  created_at: string;
}

// Represents Soma agent registry stats.
export interface SomaStats {
  total_agents: number;
  online_agents: number;
  types: number;
  by_type: StatBreakdown[];
  by_status: StatBreakdown[];
}

// Represents one Soma agent.
export interface Agent {
  id: number;
  name: string;
  type: string;
  description?: string;
  capabilities: unknown;
  status: string;
  config: unknown;
  heartbeat_at?: string;
  created_at: string;
  updated_at: string;
  quality_score?: number;
  drift_flags: unknown;
  user_id: number;
}

// Represents one Soma group.
export interface Group {
  id: number;
  name: string;
  description?: string;
  user_id: number;
  created_at: string;
}

// Represents Loom workflow-run stats.
export interface LoomStats {
  workflows: number;
  runs: number;
  active_runs: number;
  steps: number;
  runs_by_status: StatBreakdown[];
}

// Represents one Loom workflow definition.
export interface Workflow {
  id: number;
  name: string;
  description?: string;
  steps: unknown;
  user_id: number;
  created_at: string;
  updated_at: string;
}

// Represents one Loom workflow run.
export interface Run {
  id: number;
  workflow_id: number;
  status: string;
  input: unknown;
  output: unknown;
  error?: string;
  user_id: number;
  started_at?: string;
  completed_at?: string;
  created_at: string;
  updated_at: string;
}

// Represents one Loom run step.
export interface Step {
  id: number;
  run_id: number;
  name: string;
  type: string;
  config: unknown;
  status: string;
  input: unknown;
  output: unknown;
  error?: string;
  depends_on: unknown;
  retry_count: number;
  max_retries: number;
  timeout_ms: number;
  started_at?: string;
  completed_at?: string;
  created_at: string;
}

// Represents Axon event-bus stats.
export interface AxonStats {
  total_events: number;
  channels: number;
  sources: number;
  by_channel: { channel: string; count: number; latest?: string }[];
}

// Represents one Axon event.
export interface AxonEvent {
  id: number;
  channel: string;
  action: string;
  payload: unknown;
  source?: string;
  agent?: string;
  user_id: number;
  created_at: string;
}

// Represents one Axon channel.
export interface Channel {
  id: number;
  name: string;
  description?: string;
  retain_hours: number;
  created_at: string;
  event_count: number;
  subscriber_count: number;
}

// Represents Thymus quality and drift stats.
export interface ThymusStats {
  rubrics: number;
  evaluations: number;
  metrics: number;
  agent_count: number;
  by_rubric: { name: string; evaluation_count: number; avg_score: number }[];
}

// Represents one Thymus rubric.
export interface Rubric {
  id: number;
  name: string;
  description?: string;
  criteria: unknown;
  user_id: number;
  created_at: string;
  updated_at: string;
}

// Represents one Thymus evaluation.
export interface Evaluation {
  id: number;
  rubric_id: number;
  agent: string;
  subject: string;
  input: unknown;
  output: unknown;
  scores: unknown;
  overall_score: number;
  notes?: string;
  evaluator: string;
  user_id: number;
  created_at: string;
}

// Represents one Thymus drift event.
export interface DriftEvent {
  id: number;
  agent: string;
  session_id?: string;
  drift_type: string;
  severity: string;
  signal: string;
  user_id: number;
  created_at: string;
}

// Represents one memory search result.
export interface SearchResult {
  id: number;
  content: string;
  category: string;
  source: string;
  importance: number;
  created_at: string;
  score: number;
  tags: string[];
  search_type: string;
  semantic_score?: number;
  fts_score?: number;
  combined_score?: number;
  decay_score?: number;
}

// Represents one extracted memory entity.
export interface Entity {
  id: number;
  name: string;
  entity_type: string;
  description?: string;
  aliases?: string;
  confidence: number;
  occurrence_count: number;
  first_seen_at: string;
  last_seen_at: string;
  created_at: string;
}

// Represents one memory project.
export interface Project {
  id: number;
  name: string;
  description?: string;
  status: string;
  metadata?: string;
  memory_count?: number;
  created_at: string;
  updated_at?: string;
}

// Represents the server health payload.
export interface Health {
  status: string;
  version: string;
  memories: number;
  entities: number;
  episodes: number;
  pending: number;
  [k: string]: unknown;
}

// Represents the semantic type of a memory graph edge.
export type LinkType =
  | 'cite'
  | 'mentions'
  | 'contradicts'
  | 'refines'
  | 'generalizes'
  | 'has_fact'
  | 'association'
  | 'temporal'
  | 'causal'
  | 'resolves';

// Represents one memory node in the graph payload.
export interface GraphNode {
  id: string;
  label: string;
  importance: number;
  size: number;
  category: string;
  community_id?: number;
  pagerank?: number;
  decay_score?: number;
  content: string;
  source: string;
  created_at: string;
  is_static: boolean;
}

// Represents one memory edge in the graph payload.
export interface GraphEdge {
  source: string;
  target: string;
  type: LinkType;
  weight: number;
}

// Represents the complete memory graph payload.
export interface GraphData {
  nodes: GraphNode[];
  edges: GraphEdge[];
  node_count: number;
  edge_count: number;
}

// Access level an instance grant conveys over an owner's shard.
export type InstanceAccess = 'read' | 'write';

// Represents the authenticated caller's identity and scopes (GET /me).
export interface Me {
  user_id: number;
  username: string | null;
  scopes: string[];
  is_admin: boolean;
}

// Represents a Kleos user as returned by the admin user list.
export interface KleosUser {
  id: number;
  username: string;
  email?: string | null;
  role?: string | null;
  is_active?: boolean;
  created_at?: string;
}

// Represents one instance-level access grant (Spaces and Sharing).
export interface InstanceGrant {
  owner_user_id: number;
  grantee_user_id: number;
  access: InstanceAccess;
  granted_by: number;
  created_at: string;
}

// Represents one grant in the admin overview, with usernames resolved.
export interface AdminGrant {
  owner_user_id: number;
  owner_username: string | null;
  grantee_user_id: number;
  grantee_username: string | null;
  access: InstanceAccess;
  granted_by: number;
  granted_by_username: string | null;
  created_at: string;
}

// Represents one named space in the admin overview, with the owner resolved.
export interface AdminSpace {
  id: number;
  owner_user_id: number;
  owner_username: string | null;
  name: string;
  description: string | null;
  created_at: string;
}

// Represents one date bucket in the timeline calendar aggregation.
export interface CalendarBucket {
  bucket: string;
  count: number;
}

// Represents one music track exposed by the server music manifest.
export interface Track {
  src: string;
  name: string;
}

// Represents one pending memory awaiting inbox review.
export interface InboxItem {
  id: number;
  content: string;
  category: string;
  source: string;
  created_at: string;
}
