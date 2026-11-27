import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import {
  auditDecisionI18nKey,
  formatEventPayloadSummary,
  formatRoutineSchedule,
  isValidMcpEndpointInput,
  isValidMcpServerIdInput,
  isValidSkillNameInput,
  parseAllowedToolsInput,
  parseTaintFlags,
  runStatusI18nKey,
} from './agentHarnessTypes.ts';

test('个人技能名称与允许工具解析应拒绝路径穿越和非法字符', () => {
  assert.equal(isValidSkillNameInput('weekly-review_01'), true);
  assert.equal(isValidSkillNameInput('../etc/passwd'), false);
  assert.equal(isValidSkillNameInput('.hidden'), false);
  assert.equal(isValidSkillNameInput('bad name'), false);
  assert.equal(isValidSkillNameInput(''), false);

  assert.deepEqual(
    parseAllowedToolsInput('weekly_review, ../escape, project_stats，list_todos'),
    ['weekly_review', 'project_stats', 'list_todos'],
  );
});

test('污点标记、状态与事件摘要格式化应安全稳定', () => {
  assert.deepEqual(
    parseTaintFlags('local_work_data, user_memory, '),
    ['local_work_data', 'user_memory'],
  );
  assert.deepEqual(parseTaintFlags(''), []);

  assert.equal(runStatusI18nKey('waiting_approval'), 'ask.harness.statusWaiting');
  assert.equal(runStatusI18nKey('cancelled'), 'ask.harness.statusCancelled');
  assert.equal(auditDecisionI18nKey('approved'), 'ask.harness.decisionApproved');
  assert.equal(auditDecisionI18nKey('deny'), 'ask.harness.decisionDeny');

  assert.equal(
    formatEventPayloadSummary(JSON.stringify({ mode: 'work_review', toolCalls: 2, empty: '' })),
    'mode=work_review · toolCalls=2',
  );
  assert.equal(formatEventPayloadSummary('{}'), '');
});

test('AgentHarnessDrawer 与 Ask 接入应保持安全渲染且使用最新命令名', async () => {
  const [askSource, drawerSource] = await Promise.all([
    readFile(new URL('./Ask.svelte', import.meta.url), 'utf8'),
    readFile(new URL('./AgentHarnessDrawer.svelte', import.meta.url), 'utf8'),
  ]);

  assert.match(askSource, /import AgentHarnessDrawer from '\.\/AgentHarnessDrawer\.svelte';/);
  assert.match(askSource, /ask-header-harness/);
  assert.doesNotMatch(drawerSource, /\{@html\b/, '审计与技能抽屉不得使用 {@html} 渲染不可信内容');
  assert.match(drawerSource, /confirmDeleteSkillName/, '删除个人技能前必须经过二次确认');
  assert.match(drawerSource, /deletingSkillName/, '删除个人技能期间必须有防重入状态');
  assert.match(drawerSource, /aria-pressed=/, '范围切换按钮应提供无障碍 aria-pressed 语义');
  for (const command of [
    'list_agent_runs',
    'get_agent_run_trace',
    'list_personal_skills',
    'get_personal_skill',
    'save_personal_skill',
    'delete_personal_skill',
    'open_personal_skills_dir',
  ]) {
    assert.ok(drawerSource.includes(`'${command}'`), `抽屉应调用命令 ${command}`);
  }

  const [routinesSource, mcpSource] = await Promise.all([
    readFile(new URL('./AgentRoutinesPanel.svelte', import.meta.url), 'utf8'),
    readFile(new URL('./AgentMcpPanel.svelte', import.meta.url), 'utf8'),
  ]);
  assert.doesNotMatch(routinesSource, /\{@html\b/);
  assert.doesNotMatch(mcpSource, /\{@html\b/);
  assert.match(routinesSource, /confirmDeleteId/, '删除自主任务必须经过二次确认');
  assert.match(mcpSource, /confirmDeleteId/, '删除外部 MCP 服务必须经过二次确认');
  assert.equal(isValidMcpServerIdInput('obsidian'), true);
  assert.equal(isValidMcpServerIdInput('foo_'), false);
  assert.equal(isValidMcpServerIdInput('_bar'), false);
  assert.equal(isValidMcpServerIdInput('a__b'), false);
  assert.equal(isValidMcpEndpointInput('https://mcp.example.com/rpc', false), true);
  assert.equal(isValidMcpEndpointInput('http://mcp.example.com/rpc', false), false);
  assert.equal(isValidMcpEndpointInput('http://127.0.0.1:27124/mcp', false), false);
  assert.equal(isValidMcpEndpointInput('http://127.0.0.1:27124/mcp', true), true);
  assert.equal(isValidMcpEndpointInput('file:///etc/passwd', true), false);
  assert.equal(isValidMcpEndpointInput('https://user:pass@example.com/rpc', false), false);
  assert.equal(formatRoutineSchedule({ kind: 'daily_at', hour: 9, minute: 5 }, 'zh-CN'), '每天 09:05');
});
