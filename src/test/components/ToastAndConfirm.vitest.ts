import { describe, expect, it } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import Toast from '$lib/components/Toast.svelte';
import ConfirmDialog from '$lib/components/ConfirmDialog.svelte';
import { clearToast, showToast } from '$lib/stores/toast.ts';
import { confirm, resolveConfirm } from '$lib/stores/confirm.ts';

describe('Toast & ConfirmDialog (Svelte 5 runes behavioral tests)', () => {
  it('renders toast notification and clears when clicked', async () => {
    clearToast();
    render(Toast);
    expect(screen.queryByRole('status')).toBeNull();

    showToast('项目配置已保存', 'success', 60_000);
    const status = await screen.findByRole('status');
    expect(status.textContent).toContain('项目配置已保存');

    const button = screen.getByRole('button');
    await fireEvent.click(button);
    await waitFor(() => {
      expect(screen.queryByRole('status')).toBeNull();
    });
  });

  it('resolves confirm dialog true on confirm click and false on Escape', async () => {
    resolveConfirm(false);
    render(ConfirmDialog);
    expect(screen.queryByRole('dialog')).toBeNull();

    const firstPromise = confirm({
      tone: 'error',
      title: '删除记录',
      message: '删除后无法恢复',
      confirmText: '确认删除',
      cancelText: '取消',
    });

    const dialog = await screen.findByRole('dialog');
    expect(dialog.textContent).toContain('删除记录');
    expect(dialog.textContent).toContain('删除后无法恢复');

    await fireEvent.click(screen.getByRole('button', { name: '确认删除' }));
    await expect(firstPromise).resolves.toBe(true);
    await waitFor(() => {
      expect(screen.queryByRole('dialog')).toBeNull();
    });

    const secondPromise = confirm({
      tone: 'warning',
      title: '放弃编辑',
      message: '当前内容尚未保存',
      confirmText: '放弃',
      cancelText: '保留',
    });

    await screen.findByRole('dialog');
    await fireEvent.keyDown(window, { key: 'Escape' });
    await expect(secondPromise).resolves.toBe(false);
    await waitFor(() => {
      expect(screen.queryByRole('dialog')).toBeNull();
    });
  });
});
