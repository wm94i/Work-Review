import { open as openExternal } from '@tauri-apps/plugin-shell';

/** 允许交给系统浏览器 / 邮件客户端打开的协议。 */
const EXTERNAL_LINK_PROTOCOLS = new Set(['http:', 'https:', 'mailto:']);

/**
 * 判断 href 是否可以在外部打开；返回规范化后的 URL，不允许时返回 null。
 * 相对路径、`javascript:`、`file:`、自定义协议（可能唤起本机程序）一律拒绝。
 */
export function resolveExternalHref(href: string | null | undefined): string | null {
  const raw = (href ?? '').trim();
  if (!raw || raw.startsWith('#')) return null;
  let parsed: URL;
  try {
    parsed = new URL(raw);
  } catch {
    return null;
  }
  return EXTERNAL_LINK_PROTOCOLS.has(parsed.protocol) ? parsed.href : null;
}

/**
 * Svelte action：拦截容器内 Markdown 渲染出的链接点击与表单提交。
 *
 * 模型输出可能夹带被注入的链接或表单；不拦截时点击会让主 WebView 直接跳离应用或跳转 SPA 路由。
 * 这里一律阻止默认导航（含中键 auxclick、空 href、`#/` 路由链接、`<area href>` 与表单 submit），
 * 仅把 http(s) / mailto 链接交给系统浏览器打开。
 */
export function interceptExternalLinks(node: HTMLElement): { destroy: () => void } {
  const handleLinkEvent = (event: MouseEvent) => {
    if (event.type === 'auxclick' && event.button !== 1) return;
    const target = event.target;
    const link = target instanceof Element ? target.closest('a[href], area[href]') : null;
    if (!link) return;

    const rawHref = (link.getAttribute('href') ?? '').trim();
    // 仅允许纯页内锚点（且禁止 `#/` SPA 路由跳转与空 `#`）
    if (rawHref.startsWith('#') && rawHref.length > 1 && !rawHref.startsWith('#/')) {
      return;
    }

    event.preventDefault();
    const external = resolveExternalHref(rawHref);
    if (!external) return;
    openExternal(external).catch((error: unknown) => {
      console.error('打开外部链接失败:', error);
    });
  };

  const handleSubmit = (event: Event) => {
    event.preventDefault();
  };

  node.addEventListener('click', handleLinkEvent);
  node.addEventListener('auxclick', handleLinkEvent);
  node.addEventListener('submit', handleSubmit);
  return {
    destroy() {
      node.removeEventListener('click', handleLinkEvent);
      node.removeEventListener('auxclick', handleLinkEvent);
      node.removeEventListener('submit', handleSubmit);
    },
  };
}
