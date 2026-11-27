export interface AgentRunSummary {
  id: string;
  conversationId: number | null;
  source: string;
  requestMode: string;
  status: string;
  modelName: string | null;
  inputPreview: string;
  answerPreview: string | null;
  taintFlags: string;
  compactionCount: number;
  toolCallCount: number;
  errorMessage: string | null;
  startedAt: number;
  finishedAt: number | null;
}

export interface AgentRunEvent {
  id: number;
  runId: string;
  seq: number;
  eventKind: string;
  toolName: string | null;
  payloadJson: string;
  createdAt: number;
}

export interface AgentToolAuditEntry {
  id: number;
  runId: string;
  toolName: string;
  riskTier: string;
  decision: string;
  taintFlags: string;
  ok: boolean;
  durationMs: number;
  reason: string | null;
  createdAt: number;
}

export interface AgentRunTrace {
  run: AgentRunSummary;
  events: AgentRunEvent[];
  audits: AgentToolAuditEntry[];
}

export interface SkillMetadata {
  name: string;
  description: string;
  path: string;
  allowedTools: string[];
}

export interface SkillDetail extends SkillMetadata {
  body: string;
}

export type RoutineSchedule =
  | { kind: 'daily_at'; hour: number; minute: number }
  | { kind: 'interval_minutes'; every_minutes: number }
  | { kind: 'manual_only' };

export interface AgentRoutine {
  id: string;
  name: string;
  prompt: string;
  enabled: boolean;
  allowCloudModel: boolean;
  schedule: RoutineSchedule;
  skillName: string | null;
  lastRunAt: number | null;
  lastRunId: string | null;
  lastStatus: string | null;
  lastSummary: string | null;
}

export interface RoutineRunOutcome {
  routineId: string;
  status: string;
  runId: string | null;
  summary: string;
}

export interface McpClientServerView {
  id: string;
  name: string;
  endpoint: string;
  enabled: boolean;
  allowLocalhost: boolean;
  hasAuthToken: boolean;
}

export interface McpServerTestResult {
  serverId: string;
  ok: boolean;
  toolCount: number;
  toolNames: string[];
  message: string;
}

export function isValidMcpServerIdInput(raw: string): boolean {
  const trimmed = raw.trim();
  return (
    isValidSkillNameInput(trimmed)
    && !trimmed.includes('__')
    && !trimmed.startsWith('_')
    && !trimmed.endsWith('_')
  );
}

export function isLoopbackHostname(hostname: string): boolean {
  const clean = hostname.replace(/^\[|\]$/g, '').replace(/\.$/, '').toLowerCase();
  return (
    clean === 'localhost'
    || clean.endsWith('.localhost')
    || clean === '::1'
    || /^127(?:\.\d{1,3}){3}$/.test(clean)
  );
}

export function isValidMcpEndpointInput(raw: string, allowLocalhost = false): boolean {
  const trimmed = raw.trim();
  if (!trimmed || trimmed.length > 512 || /\s/.test(trimmed)) return false;
  try {
    const parsed = new URL(trimmed);
    if (parsed.protocol !== 'http:' && parsed.protocol !== 'https:') return false;
    if (parsed.username || parsed.password) return false;
    if (!parsed.hostname) return false;
    const isLoopback = isLoopbackHostname(parsed.hostname);
    if (isLoopback) return allowLocalhost;
    return parsed.protocol === 'https:';
  } catch {
    return false;
  }
}

export function formatRoutineSchedule(schedule: RoutineSchedule, localeCode: string): string {
  if (schedule.kind === 'daily_at') {
    const hh = String(schedule.hour).padStart(2, '0');
    const mm = String(schedule.minute).padStart(2, '0');
    return localeCode === 'en' ? `Daily ${hh}:${mm}` : `每天 ${hh}:${mm}`;
  }
  if (schedule.kind === 'interval_minutes') {
    return localeCode === 'en'
      ? `Every ${schedule.every_minutes}m`
      : `每 ${schedule.every_minutes} 分钟`;
  }
  return localeCode === 'en' ? 'Manual only' : '仅手动';
}

export function parseTaintFlags(raw: string | null | undefined): string[] {
  if (!raw) return [];
  return raw
    .split(',')
    .map((item) => item.trim())
    .filter(Boolean);
}

export function parseAllowedToolsInput(raw: string): string[] {
  return raw
    .split(/[,\n，]+/)
    .map((item) => item.trim())
    .filter((item) => /^[a-zA-Z0-9_-]{1,64}$/.test(item))
    .slice(0, 32);
}

export function isValidSkillNameInput(raw: string): boolean {
  const trimmed = raw.trim();
  return trimmed.length >= 1 && trimmed.length <= 64 && /^[a-zA-Z0-9_-]+$/.test(trimmed);
}

export function runStatusI18nKey(status: string): string {
  switch (status) {
    case 'running':
      return 'ask.harness.statusRunning';
    case 'waiting_approval':
      return 'ask.harness.statusWaiting';
    case 'completed':
      return 'ask.harness.statusCompleted';
    case 'cancelled':
      return 'ask.harness.statusCancelled';
    case 'failed':
      return 'ask.harness.statusFailed';
    case 'timed_out':
      return 'ask.harness.statusTimedOut';
    default:
      return 'ask.harness.statusCompleted';
  }
}

export function auditDecisionI18nKey(decision: string): string {
  switch (decision) {
    case 'allow':
      return 'ask.harness.decisionAllow';
    case 'approved':
      return 'ask.harness.decisionApproved';
    case 'denied':
      return 'ask.harness.decisionDenied';
    case 'timed_out':
      return 'ask.harness.decisionTimedOut';
    case 'deny':
    default:
      return 'ask.harness.decisionDeny';
  }
}

export function formatEventPayloadSummary(payloadJson: string): string {
  const trimmed = (payloadJson || '').trim();
  if (!trimmed || trimmed === '{}') return '';
  try {
    const parsed: unknown = JSON.parse(trimmed);
    if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) {
      return trimmed.slice(0, 120);
    }
    const pairs = Object.entries(parsed as Record<string, unknown>)
      .filter(([, value]) => value !== null && value !== undefined && value !== '')
      .slice(0, 4)
      .map(([key, value]) => `${key}=${typeof value === 'string' ? value : JSON.stringify(value)}`);
    return pairs.join(' · ').slice(0, 160);
  } catch {
    return trimmed.slice(0, 120);
  }
}

export function formatUnixTimestamp(epochSeconds: number | null | undefined): string {
  if (!epochSeconds || !Number.isFinite(epochSeconds)) return '';
  const date = new Date(epochSeconds * 1000);
  const hh = String(date.getHours()).padStart(2, '0');
  const mm = String(date.getMinutes()).padStart(2, '0');
  const ss = String(date.getSeconds()).padStart(2, '0');
  const mon = String(date.getMonth() + 1).padStart(2, '0');
  const day = String(date.getDate()).padStart(2, '0');
  return `${mon}-${day} ${hh}:${mm}:${ss}`;
}
