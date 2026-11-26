import { describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { mockIPC } from '@tauri-apps/api/mocks';
import { setLocale } from '$lib/i18n/index.ts';

vi.mock('@tauri-apps/plugin-dialog', () => ({
  ask: vi.fn(async () => true),
}));

import SettingsProjects from '../../routes/settings/components/SettingsProjects.svelte';

type MockRuleField = 'app' | 'title' | 'domain' | 'url_path' | 'executable_path';
type MockRuleOp = 'equals' | 'contains' | 'regex';

interface MockProject {
  key: string;
  name: string;
  color: string;
  archived: boolean;
  rules: Array<{ field: MockRuleField; op: MockRuleOp; value: string }>;
}

describe('SettingsProjects (mockIPC CRUD + rule preview behavioral tests)', () => {
  it('loads projects, previews a rule, saves a new project, and deletes a project via Tauri IPC', async () => {
    setLocale('zh-CN');

    let projectStore: MockProject[] = [
      {
        key: 'work-review',
        name: 'Work Review',
        color: '#3B82F6',
        archived: false,
        rules: [{ field: 'title', op: 'contains', value: 'Work_Review' }],
      },
    ];
    const ipcLog: Array<{ cmd: string; args: unknown }> = [];

    mockIPC((cmd, args) => {
      ipcLog.push({ cmd, args });
      if (cmd === 'get_projects') {
        return structuredClone(projectStore);
      }
      if (cmd === 'preview_project_rule') {
        return {
          dateFrom: '2026-11-18',
          dateTo: '2026-11-24',
          matchedCount: 3,
          totalDuration: 3600,
          sampleActivities: [
            {
              date: '2026-11-24',
              appName: 'Code',
              windowTitle: 'client-portal - VS Code',
              browserUrl: null,
              duration: 1800,
            },
          ],
        };
      }
      if (cmd === 'save_project') {
        const payload = (args as { project: MockProject }).project;
        const saved: MockProject = {
          ...payload,
          key: payload.key || 'client-portal',
        };
        projectStore = [
          ...projectStore.filter((p) => p.key !== saved.key),
          saved,
        ];
        return structuredClone(projectStore);
      }
      if (cmd === 'delete_project') {
        const { key } = args as { key: string };
        projectStore = projectStore.filter((p) => p.key !== key);
        return structuredClone(projectStore);
      }
      throw new Error(`Unexpected IPC command: ${cmd}`);
    });

    const config: { projects?: MockProject[] } = {};
    render(SettingsProjects, { props: { config } });

    // 1. 初始加载渲染已有项目
    await screen.findByText('Work Review');
    expect(ipcLog[0]?.cmd).toBe('get_projects');

    // 2. 点击新建项目、填写名称和规则并执行规则命中预览
    await fireEvent.click(screen.getByRole('button', { name: /新建项目|添加项目/ }));
    const nameInput = screen.getByPlaceholderText(/例如|项目名称|Work Review/i);
    await fireEvent.input(nameInput, { target: { value: 'Client Portal' } });

    const ruleValueInputs = screen.getAllByRole('textbox');
    const ruleInput = ruleValueInputs[ruleValueInputs.length - 1];
    await fireEvent.input(ruleInput, { target: { value: 'client-portal' } });

    await fireEvent.click(screen.getByRole('button', { name: /测试|预览/ }));
    await screen.findByText(/client-portal - VS Code/);
    expect(ipcLog.some((entry) => entry.cmd === 'preview_project_rule')).toBe(true);

    // 3. 保存项目并验证新项目出现在列表且同步回 config
    await fireEvent.click(screen.getByRole('button', { name: /保存/ }));
    await screen.findByText('Client Portal');
    expect(config.projects?.map((p) => p.name)).toContain('Client Portal');

    // 4. 删除新创建的项目
    const deleteButtons = screen.getAllByRole('button', { name: /删除/ });
    await fireEvent.click(deleteButtons[deleteButtons.length - 1]);
    await waitFor(() => {
      expect(ipcLog.some((entry) => entry.cmd === 'delete_project')).toBe(true);
      expect(screen.queryByText('Client Portal')).toBeNull();
    });
  });
});
