import { describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { push } from 'svelte-spa-router';
import { setLocale, t } from '$lib/i18n/index.ts';
import Sidebar from '$lib/components/Sidebar.svelte';
import Summary from '../../routes/timeline/Summary.svelte';

describe('Sidebar & Summary (Svelte 5 + svelte-spa-router v5 behavioral tests)', () => {
  it('updates Sidebar active nav highlight reactively when router.location changes and invokes onThemeChange', async () => {
    setLocale('zh-CN');
    window.location.hash = '#/';
    window.dispatchEvent(new Event('hashchange'));

    const onThemeChange = vi.fn();
    render(Sidebar, {
      props: {
        isRecording: true,
        isPaused: false,
        theme: 'system',
        onThemeChange,
      },
    });

    const overviewLink = screen.getByRole('link', { name: '概览' });
    const timelineLink = screen.getByRole('link', { name: '时间线' });
    const reportLink = screen.getByRole('link', { name: '日报' });

    await waitFor(() => {
      expect(overviewLink.getAttribute('aria-current')).toBe('page');
      expect(timelineLink.getAttribute('aria-current')).toBeNull();
    });

    // 导航到 /timeline，验证 runes $derived 正确追踪 svelte-spa-router v5 的 router.location
    await push('/timeline');
    window.dispatchEvent(new Event('hashchange'));
    await waitFor(() => {
      expect(timelineLink.getAttribute('aria-current')).toBe('page');
      expect(overviewLink.getAttribute('aria-current')).toBeNull();
    });

    await push('/report');
    window.dispatchEvent(new Event('hashchange'));
    await waitFor(() => {
      expect(reportLink.getAttribute('aria-current')).toBe('page');
      expect(timelineLink.getAttribute('aria-current')).toBeNull();
    });

    // 验证回调 prop onThemeChange 正常工作（system -> light）
    const themeBtn = screen.getByRole('button', { name: t('sidebar.themeTitle.system') });
    await fireEvent.click(themeBtn);
    expect(onThemeChange).toHaveBeenCalledWith('light');
  });

  it('redirects legacy Summary route with date to /timeline?date=...&summary=1', async () => {
    setLocale('zh-CN');
    window.location.hash = '#/timeline/summary/2026-11-20';
    window.dispatchEvent(new Event('hashchange'));

    render(Summary);

    await waitFor(() => {
      expect(window.location.hash).toBe('#/timeline?date=2026-11-20&summary=1');
    });
  });
});
