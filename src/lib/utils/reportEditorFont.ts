export const REPORT_EDITOR_FONTS = [
  'Microsoft YaHei',
  'PingFang SC',
  'Noto Sans CJK SC',
  'Arial',
  'monospace',
] as const;

export function reportEditorFontFamily(value: unknown): string {
  const font = REPORT_EDITOR_FONTS.find(option => option === value);
  if (!font) return 'inherit';
  return font === 'monospace'
    ? 'ui-monospace, SFMono-Regular, Menlo, Consolas, "Microsoft YaHei", "PingFang SC", "Noto Sans CJK SC", monospace'
    : `"${font}", var(--font-ui)`;
}
