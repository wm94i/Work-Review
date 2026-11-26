import { describe, expect, it } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { mockIPC } from '@tauri-apps/api/mocks';
import { setLocale } from '$lib/i18n/index.ts';
import BotSenderBindingPanel from '../../routes/settings/components/nodeGateway/BotSenderBindingPanel.svelte';

describe('BotSenderBindingPanel (mockIPC behavioral tests)', () => {
  it('loads status, generates bind code, and requires two-step confirm before clearing', async () => {
    setLocale('zh-CN');

    let currentStatus = {
      allowedCount: 1,
      allowedSenders: ['ou_alice'],
      bindCode: null as string | null,
      bindCodeExpiresAt: null as number | null,
      bindCodeExpired: false,
    };
    const ipcCalls: Array<{ cmd: string; args: unknown }> = [];

    mockIPC((cmd, args) => {
      ipcCalls.push({ cmd, args });
      if (cmd === 'get_bot_sender_binding_status') {
        return { ...currentStatus, allowedSenders: [...currentStatus.allowedSenders] };
      }
      if (cmd === 'generate_bot_sender_bind_code') {
        currentStatus = {
          ...currentStatus,
          bindCode: '847291',
          bindCodeExpiresAt: Math.floor(Date.now() / 1000) + 600,
          bindCodeExpired: false,
        };
        return { bindCode: '847291' };
      }
      if (cmd === 'clear_bot_sender_bindings') {
        currentStatus = {
          allowedCount: 0,
          allowedSenders: [],
          bindCode: null,
          bindCodeExpiresAt: null,
          bindCodeExpired: false,
        };
        return null;
      }
      throw new Error(`Unexpected IPC command: ${cmd}`);
    });

    render(BotSenderBindingPanel, {
      props: { platform: 'feishu' },
    });

    // 初始加载并展示已绑定发送者
    await screen.findByText('ou_alice');
    expect(ipcCalls[0]).toEqual({
      cmd: 'get_bot_sender_binding_status',
      args: { platform: 'feishu' },
    });

    // 生成绑定码并显示 /bind 847291
    const generateBtn = screen.getByRole('button', { name: /生成绑定码/ });
    await fireEvent.click(generateBtn);
    await screen.findByText('/bind 847291');
    expect(ipcCalls.some((c) => c.cmd === 'generate_bot_sender_bind_code')).toBe(true);

    // 第一次点击清空仅进入二次确认态，不立即调用 IPC
    const clearBtn = screen.getByRole('button', { name: /清空/ });
    await fireEvent.click(clearBtn);
    expect(ipcCalls.some((c) => c.cmd === 'clear_bot_sender_bindings')).toBe(false);

    // 第二次点击确认清空并刷新为空状态
    await fireEvent.click(clearBtn);
    await waitFor(() => {
      expect(ipcCalls.some((c) => c.cmd === 'clear_bot_sender_bindings')).toBe(true);
      expect(screen.queryByText('ou_alice')).toBeNull();
    });
  });
});
