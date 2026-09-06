import { writable } from 'svelte/store';

export type UiTemplate = 'classic' | 'evidence-star-map';

const DEFAULT_UI_TEMPLATE: UiTemplate = 'classic';

export function normalizeUiTemplate(value: unknown): UiTemplate {
  if (typeof value !== 'string') {
    return DEFAULT_UI_TEMPLATE;
  }

  const normalized = value.trim().toLowerCase();
  return normalized === 'evidence-star-map' ? normalized : DEFAULT_UI_TEMPLATE;
}

export const uiTemplate = writable<UiTemplate>(DEFAULT_UI_TEMPLATE);

export function applyUiTemplate(value: unknown): UiTemplate {
  const normalized = normalizeUiTemplate(value);
  uiTemplate.set(normalized);

  if (typeof document !== 'undefined') {
    document.documentElement.dataset.uiTemplate = normalized;
  }

  return normalized;
}
