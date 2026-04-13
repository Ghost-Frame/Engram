// ============================================================================
// Loom workflow engine - multi-step pipelines, dependency execution,
// step executors for webhooks, LLM calls, and data transforms.
// ============================================================================

import { db } from "../../db/index.ts";
import { log } from "../../config/logger.ts";
import { parseJsonFields, parseJsonFieldsAll } from "../helpers.ts";
import { publish } from "../axon/bus.ts";
import { callLocalModel, isLocalModelAvailable } from "../../llm/local.ts";
import { validatePublicUrlWithDNS } from "../../helpers/index.ts";
import {
  insertWorkflow, getWorkflowById, getWorkflowByNameStmt, listWorkflowsStmt, deleteWorkflowStmt,
  insertRun, getRunById,
  insertStep, getStepById,
  insertRunLog,
  workflowCount, runCount, activeRunCount, stepCount,
} from "./db.ts";

// ── Step definition (as stored in workflow.steps JSON) ───────────────

interface StepDef {
  name: string;
  type: string;
  config?: Record<string, unknown>;
  depends_on?: string[];
  max_retries?: number;
  timeout_ms?: number;
}

// ── Workflow CRUD ────────────────────────────────────────────────────

export function createWorkflow(userId: number, name: string, description: string | null | undefined, steps: StepDef[]) {
  const info = insertWorkflow.run(name, description ?? null, JSON.stringify(steps), userId);
  return getWorkflow(Number(info.lastInsertRowid), userId)!;
}

export function getWorkflow(id: number, userId: number) {
  const row = getWorkflowById.get(id, userId) as Record<string, unknown> | undefined;
  return parseJsonFields(row, "steps");
}

export function getWorkflowByName(name: string, userId: number) {
  const row = getWorkflowByNameStmt.get(name, userId) as Record<string, unknown> | undefined;
  return parseJsonFields(row, "steps");
}

export function listWorkflows(userId: number) {
  const rows = listWorkflowsStmt.all(userId) as Record<string, unknown>[];
  return parseJsonFieldsAll(rows, "steps");
}

export function updateWorkflow(id: number, userId: number, updates: { name?: string; description?: string | null; steps?: StepDef[] }) {
  const fields: string[] = [];
  const params: unknown[] = [];

  if (updates.name !== undefined) { fields.push("name = ?"); params.push(updates.name); }
  if (updates.description !== undefined) { fields.push("description = ?"); params.push(updates.description); }
  if (updates.steps !== undefined) { fields.push("steps = ?"); params.push(JSON.stringify(updates.steps)); }

  if (fields.length === 0) return getWorkflow(id, userId);

  fields.push("updated_at = datetime('now')");
  params.push(id, userId);

  db.prepare(`UPDATE loom_workflows SET ${fields.join(", ")} WHERE id = ? AND user_id = ?`).run(...params);
  return getWorkflow(id, userId);
}

export function deleteWorkflow(id: number, userId: number): boolean {
  const info = deleteWorkflowStmt.run(id, userId);
  return info.changes > 0;
}

// ── Run management ───────────────────────────────────────────────────

export function createRun(userId: number, workflowId: number, input: Record<string, unknown>) {
  const workflow = getWorkflow(workflowId, userId);
  if (!workflow) throw new Error("Workflow not found");

  const steps = workflow.steps as StepDef[];
  if (!Array.isArray(steps) || steps.length === 0) throw new Error("Workflow has no steps");

  const runInfo = insertRun.run(workflowId, "pending", JSON.stringify(input), userId);
  const runId = Number(runInfo.lastInsertRowid);

  // Create step records from workflow definition
  for (const step of steps) {
    insertStep.run(
      runId,
      step.name,
      step.type,
      JSON.stringify(step.config ?? {}),
      JSON.stringify(step.depends_on ?? []),
      step.max_retries ?? 3,
      step.timeout_ms ?? 30000,
    );
  }

  const run = getRun(runId, userId)!;

  // Publish event AFTER insert (outside any transaction)
  publish(1, "system", "loom", "workflow.run.created", {
    run_id: runId,
    workflow_id: workflowId,
    workflow_name: (workflow as any).name,
  });

  addLog(runId, null, "info", "Run created", { workflow_id: workflowId, input });

  // Start advancing
  setImmediate(() => advanceRun(runId));

  return run;
}

export function getRun(id: number, userId: number) {
  const row = getRunById.get(id, userId) as Record<string, unknown> | undefined;
  return parseJsonFields(row, "input", "output");
}

export function listRuns(userId: number, opts?: { workflow_id?: number; status?: string; limit?: number }) {
  const clauses: string[] = ["user_id = ?"];
  const params: unknown[] = [userId];

  if (opts?.workflow_id) { clauses.push("workflow_id = ?"); params.push(opts.workflow_id); }
  if (opts?.status) { clauses.push("status = ?"); params.push(opts.status); }

  const where = clauses.length ? `WHERE ${clauses.join(" AND ")}` : "";
  const limit = opts?.limit ?? 100;
  const rows = db.prepare(
    `SELECT * FROM loom_runs ${where} ORDER BY created_at DESC LIMIT ?`
  ).all(...params, limit) as Record<string, unknown>[];
  return parseJsonFieldsAll(rows, "input", "output");
}

export function cancelRun(id: number, userId: number): boolean {
  const run = getRun(id, userId);
  if (!run) return false;
  if (run.status === "completed" || run.status === "failed" || run.status === "cancelled") return false;

  db.prepare(
    "UPDATE loom_runs SET status = 'cancelled', completed_at = datetime('now'), updated_at = datetime('now') WHERE id = ?"
  ).run(id);

  // Skip all pending steps
  db.prepare(
    "UPDATE loom_steps SET status = 'skipped' WHERE run_id = ? AND status IN ('pending','running')"
  ).run(id);

  addLog(id, null, "info", "Run cancelled");

  publish(1, "system", "loom", "workflow.run.cancelled", { run_id: id });

  return true;
}

// ── Step management ──────────────────────────────────────────────────

export function getSteps(runId: number) {
  const rows = db.prepare(
    "SELECT * FROM loom_steps WHERE run_id = ? ORDER BY id"
  ).all(runId) as Record<string, unknown>[];
  return parseJsonFieldsAll(rows, "config", "input", "output", "depends_on");
}

export function getStep(id: number) {
  const row = getStepById.get(id) as Record<string, unknown> | undefined;
  return parseJsonFields(row, "config", "input", "output", "depends_on");
}

export function completeStep(stepId: number, output: Record<string, unknown>) {
  const step = getStep(stepId);
  if (!step) throw new Error("Step not found");
  if (step.status !== "running") throw new Error("Step is not running");

  db.prepare(
    "UPDATE loom_steps SET status = 'completed', output = ?, completed_at = datetime('now') WHERE id = ?"
  ).run(JSON.stringify(output), stepId);

  const runId = step.run_id as number;
  addLog(runId, stepId, "info", `Step "${step.name}" completed`, { output });

  // Break recursive chain with setImmediate
  setImmediate(() => advanceRun(runId));
}

export function failStep(stepId: number, error: string) {
  const step = getStep(stepId);
  if (!step) throw new Error("Step not found");
  if (step.status !== "running") throw new Error("Step is not running");

  const retryCount = (step.retry_count as number) + 1;
  const maxRetries = step.max_retries as number;
  const runId = step.run_id as number;

  if (retryCount < maxRetries) {
    // Retry: reset to pending with incremented retry count
    db.prepare(
      "UPDATE loom_steps SET status = 'pending', retry_count = ?, error = ?, started_at = NULL WHERE id = ?"
    ).run(retryCount, error, stepId);

    addLog(runId, stepId, "warn", `Step "${step.name}" failed, retrying (${retryCount}/${maxRetries})`, { error });

    // Break recursive chain with setImmediate
    setImmediate(() => advanceRun(runId));
  } else {
    // Exhausted retries: mark step and run as failed
    db.prepare(
      "UPDATE loom_steps SET status = 'failed', retry_count = ?, error = ?, completed_at = datetime('now') WHERE id = ?"
    ).run(retryCount, error, stepId);

    db.prepare(
      "UPDATE loom_runs SET status = 'failed', error = ?, completed_at = datetime('now'), updated_at = datetime('now') WHERE id = ?"
    ).run(`Step "${step.name}" failed after ${maxRetries} retries: ${error}`, runId);

    addLog(runId, stepId, "error", `Step "${step.name}" failed permanently`, { error, retries: retryCount });

    publish(1, "system", "loom", "workflow.run.failed", {
      run_id: runId,
      step_name: step.name,
      error,
    });
  }
}

// ── Core advance logic ───────────────────────────────────────────────

export function advanceRun(runId: number): void {
  const row = db.prepare("SELECT * FROM loom_runs WHERE id = ?").get(runId) as Record<string, unknown> | undefined;
  const run = parseJsonFields(row, "input", "output");
  if (!run) return;
  if (run.status === "completed" || run.status === "failed" || run.status === "cancelled") return;

  // Mark run as running if still pending
  if (run.status === "pending") {
    db.prepare(
      "UPDATE loom_runs SET status = 'running', started_at = datetime('now'), updated_at = datetime('now') WHERE id = ?"
    ).run(runId);
  }

  const steps = getSteps(runId);
  const stepsByName = new Map<string, Record<string, unknown>>();
  for (const s of steps) stepsByName.set(s.name as string, s);

  // Check if all steps are done
  const pendingOrRunning = steps.filter(s => s.status === "pending" || s.status === "running");
  if (pendingOrRunning.length === 0) {
    // Gather output from the last completed step
    const completedSteps = steps.filter(s => s.status === "completed");
    const lastOutput = completedSteps.length > 0
      ? completedSteps[completedSteps.length - 1].output
      : {};

    db.prepare(
      "UPDATE loom_runs SET status = 'completed', output = ?, completed_at = datetime('now'), updated_at = datetime('now') WHERE id = ?"
    ).run(JSON.stringify(lastOutput ?? {}), runId);

    addLog(runId, null, "info", "Run completed");

    publish(1, "system", "loom", "workflow.run.completed", {
      run_id: runId,
      output: lastOutput,
    });
    return;
  }

  // Find steps whose dependencies are all satisfied
  for (const step of steps) {
    if (step.status !== "pending") continue;

    const deps = step.depends_on as string[];
    const allDepsMet = deps.every(depName => {
      const dep = stepsByName.get(depName);
      return dep && dep.status === "completed";
    });

    if (!allDepsMet) continue;

    // Resolve input: run input merged with outputs of dependency steps
    const stepInput: Record<string, unknown> = { ...(run.input as Record<string, unknown>) };
    for (const depName of deps) {
      const dep = stepsByName.get(depName);
      if (dep && dep.output) {
        Object.assign(stepInput, dep.output as Record<string, unknown>);
      }
    }

    // Mark step as running
    db.prepare(
      "UPDATE loom_steps SET status = 'running', input = ?, started_at = datetime('now') WHERE id = ?"
    ).run(JSON.stringify(stepInput), step.id);

    addLog(runId, step.id as number, "info", `Step "${step.name}" started`, { type: step.type });

    // Auto-execute built-in step types
    const stepType = step.type as string;
    const stepId = step.id as number;
    const config = step.config as Record<string, unknown>;

    if (stepType === "webhook") {
      executeWebhookStep(stepId, config, stepInput);
    } else if (stepType === "llm") {
      executeLLMStep(stepId, config, stepInput);
    } else if (stepType === "transform") {
      executeTransformStep(stepId, config, stepInput);
    }
    // Types 'action', 'decision', 'parallel', 'wait' require external completion
    // via POST /loom/steps/:id/complete or /fail
  }
}

// ── Step executors ───────────────────────────────────────────────────

async function executeWebhookStep(
  stepId: number,
  config: Record<string, unknown>,
  input: Record<string, unknown>,
): Promise<void> {
  const url = config.url as string;
  if (!url) {
    failStep(stepId, "Webhook step missing url in config");
    return;
  }

  // SSRF validation
  const urlError = await validatePublicUrlWithDNS(url, "Webhook URL");
  if (urlError) {
    failStep(stepId, `SSRF blocked: ${urlError}`);
    return;
  }

  const method = ((config.method as string) || "POST").toUpperCase();
  const headers: Record<string, string> = {
    "Content-Type": "application/json",
    ...(config.headers as Record<string, string> || {}),
  };
  const timeoutMs = (config.timeout_ms as number) || 30000;

  try {
    const resp = await fetch(url, {
      method,
      headers,
      body: method !== "GET" ? JSON.stringify({ ...input, ...(config.body as Record<string, unknown> || {}) }) : undefined,
      signal: AbortSignal.timeout(timeoutMs),
    });

    const text = await resp.text();
    let output: Record<string, unknown>;
    try {
      output = JSON.parse(text);
    } catch {
      output = { body: text, status: resp.status };
    }

    if (!resp.ok) {
      failStep(stepId, `Webhook returned ${resp.status}: ${text.slice(0, 500)}`);
      return;
    }

    completeStep(stepId, output);
  } catch (e: any) {
    failStep(stepId, `Webhook failed: ${e.message}`);
  }
}

async function executeLLMStep(
  stepId: number,
  config: Record<string, unknown>,
  input: Record<string, unknown>,
): Promise<void> {
  if (!isLocalModelAvailable()) {
    failStep(stepId, "No LLM provider available");
    return;
  }

  const systemPrompt = interpolate((config.system_prompt as string) || "You are a helpful assistant.", input);
  const userPrompt = interpolate((config.user_prompt as string) || JSON.stringify(input), input);
  const model = config.model as string | undefined;

  try {
    const result = await callLocalModel(systemPrompt, userPrompt, { priority: "background", model });

    // Try to parse as JSON, fall back to raw text
    let output: Record<string, unknown>;
    try {
      output = JSON.parse(result);
    } catch {
      output = { response: result };
    }

    completeStep(stepId, output);
  } catch (e: any) {
    failStep(stepId, `LLM call failed: ${e.message}`);
  }
}

function executeTransformStep(
  stepId: number,
  config: Record<string, unknown>,
  input: Record<string, unknown>,
): void {
  try {
    const mapping = config.mapping as Record<string, string> | undefined;
    const template = config.template as Record<string, unknown> | undefined;

    let output: Record<string, unknown> = {};

    if (mapping) {
      // Dot-path mapping: { "result.name": "input.user.name" }
      for (const [targetPath, sourcePath] of Object.entries(mapping)) {
        const value = resolveDotPath(input, sourcePath);
        setDotPath(output, targetPath, value);
      }
    } else if (template) {
      // Template interpolation: { "greeting": "Hello {{name}}" }
      output = interpolateObject(template, input);
    } else {
      // Pass-through
      output = { ...input };
    }

    completeStep(stepId, output);
  } catch (e: any) {
    failStep(stepId, `Transform failed: ${e.message}`);
  }
}

// ── Logging ──────────────────────────────────────────────────────────

export function addLog(
  runId: number,
  stepId: number | null,
  level: string,
  message: string,
  data?: Record<string, unknown>,
) {
  insertRunLog.run(runId, stepId, level, message, JSON.stringify(data ?? {}));
  log.debug({ msg: "loom_log", run_id: runId, step_id: stepId, level, message });
}

export function getLogs(opts: { run_id: number; step_id?: number; level?: string; limit?: number }) {
  const clauses = ["run_id = ?"];
  const params: unknown[] = [opts.run_id];

  if (opts.step_id) { clauses.push("step_id = ?"); params.push(opts.step_id); }
  if (opts.level) { clauses.push("level = ?"); params.push(opts.level); }

  const where = `WHERE ${clauses.join(" AND ")}`;
  const limit = opts.limit ?? 200;
  const rows = db.prepare(
    `SELECT * FROM loom_run_logs ${where} ORDER BY created_at ASC LIMIT ?`
  ).all(...params, limit) as Record<string, unknown>[];
  return parseJsonFieldsAll(rows, "data");
}

// ── Stats ────────────────────────────────────────────────────────────

export function getStats() {
  const workflows = (workflowCount.get() as any).count;
  const runs = (runCount.get() as any).count;
  const active_runs = (activeRunCount.get() as any).count;
  const steps = (stepCount.get() as any).count;
  return { workflows, runs, active_runs, steps };
}

// ── Utility: dot-path resolution and template interpolation ─────────

function resolveDotPath(obj: Record<string, unknown>, path: string): unknown {
  const parts = path.split(".");
  let current: unknown = obj;
  for (const part of parts) {
    if (current == null || typeof current !== "object") return undefined;
    current = (current as Record<string, unknown>)[part];
  }
  return current;
}

function isUnsafePathSegment(segment: string): boolean {
  return segment === "__proto__" || segment === "constructor" || segment === "prototype";
}

function setDotPath(obj: Record<string, unknown>, path: string, value: unknown): void {
  const parts = path.split(".");
  if (parts.length === 0 || parts.some(isUnsafePathSegment)) return;

  let current: Record<string, unknown> = obj;
  for (let i = 0; i < parts.length - 1; i++) {
    const key = parts[i];
    if (!Object.hasOwn(current, key) || typeof current[key] !== "object" || current[key] === null) {
      current[key] = {};
    }
    current = current[key] as Record<string, unknown>;
  }

  const lastKey = parts[parts.length - 1];
  if (isUnsafePathSegment(lastKey)) return;
  current[lastKey] = value;
}

function interpolate(template: string, vars: Record<string, unknown>): string {
  return template.replace(/\{\{(\w+(?:\.\w+)*)\}\}/g, (_match, path) => {
    const val = resolveDotPath(vars, path);
    if (val === undefined || val === null) return "";
    return typeof val === "object" ? JSON.stringify(val) : String(val);
  });
}

function interpolateObject(
  template: Record<string, unknown>,
  vars: Record<string, unknown>,
): Record<string, unknown> {
  const result: Record<string, unknown> = {};
  for (const [key, value] of Object.entries(template)) {
    if (typeof value === "string") {
      result[key] = interpolate(value, vars);
    } else if (value && typeof value === "object" && !Array.isArray(value)) {
      result[key] = interpolateObject(value as Record<string, unknown>, vars);
    } else {
      result[key] = value;
    }
  }
  return result;
}
