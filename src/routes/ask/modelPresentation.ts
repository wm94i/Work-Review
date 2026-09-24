const BASIC_ASSISTANT_MODEL_ID = '__basic__';

type ProviderLabelLocale = 'zh-CN' | 'en' | 'zh-TW';
type ProviderLabels = Record<ProviderLabelLocale, string>;

type KnownProviderId =
  | 'ollama'
  | 'openai'
  | 'siliconflow'
  | 'deepseek'
  | 'qwen'
  | 'zhipu'
  | 'moonshot'
  | 'doubao'
  | 'minimax'
  | 'gemini'
  | 'claude'
  | 'openrouter'
  | 'requesty'
  | 'groq'
  | 'xai'
  | 'mistral'
  | 'lmstudio'
  | 'custom';

export const MODEL_PROVIDER_DISPLAY_NAMES = {
  ollama: {
    'zh-CN': 'Ollama (本地)',
    en: 'Ollama (Local)',
    'zh-TW': 'Ollama（本機）',
  },
  openai: {
    'zh-CN': 'OpenAI 兼容',
    en: 'OpenAI Compatible',
    'zh-TW': 'OpenAI 相容',
  },
  siliconflow: {
    'zh-CN': '硅基流动',
    en: 'SiliconFlow',
    'zh-TW': '矽基流動',
  },
  deepseek: { 'zh-CN': 'DeepSeek', en: 'DeepSeek', 'zh-TW': 'DeepSeek' },
  qwen: { 'zh-CN': '通义千问', en: 'Qwen', 'zh-TW': '通義千問' },
  zhipu: { 'zh-CN': '智谱清言', en: 'Zhipu', 'zh-TW': '智譜清言' },
  moonshot: { 'zh-CN': 'Kimi', en: 'Moonshot Kimi', 'zh-TW': 'Kimi' },
  doubao: { 'zh-CN': '豆包', en: 'Doubao', 'zh-TW': '豆包' },
  minimax: { 'zh-CN': 'MiniMax', en: 'MiniMax', 'zh-TW': 'MiniMax' },
  gemini: { 'zh-CN': 'Google Gemini', en: 'Google Gemini', 'zh-TW': 'Google Gemini' },
  claude: { 'zh-CN': 'Anthropic Claude', en: 'Anthropic Claude', 'zh-TW': 'Anthropic Claude' },
  openrouter: { 'zh-CN': 'OpenRouter', en: 'OpenRouter', 'zh-TW': 'OpenRouter' },
  requesty: { 'zh-CN': 'Requesty', en: 'Requesty', 'zh-TW': 'Requesty' },
  groq: { 'zh-CN': 'Groq', en: 'Groq', 'zh-TW': 'Groq' },
  xai: { 'zh-CN': 'xAI Grok', en: 'xAI Grok', 'zh-TW': 'xAI Grok' },
  mistral: { 'zh-CN': 'Mistral', en: 'Mistral', 'zh-TW': 'Mistral' },
  lmstudio: {
    'zh-CN': 'LM Studio (本地)',
    en: 'LM Studio (Local)',
    'zh-TW': 'LM Studio（本機）',
  },
  custom: {
    'zh-CN': '自定义接口',
    en: 'Custom endpoint',
    'zh-TW': '自訂介面',
  },
} as const satisfies Record<KnownProviderId, ProviderLabels>;

interface ModelProfileLike {
  id?: unknown;
  name?: unknown;
  model_config?: unknown;
}

interface ModelConfigLike {
  provider?: unknown;
  model?: unknown;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null;
}

function hasOwnKey<ObjectType extends object>(
  value: ObjectType,
  key: PropertyKey,
): key is keyof ObjectType {
  return Object.prototype.hasOwnProperty.call(value, key);
}

function isProviderLabelLocale(value: string): value is ProviderLabelLocale {
  return value === 'zh-CN' || value === 'en' || value === 'zh-TW';
}

function translatedLabel(translate: unknown, key: string): string {
  const label = typeof translate === 'function' ? translate(key) : '';
  return typeof label === 'string' && label.trim() ? label : key;
}

function localizedProviderName(providerId: unknown, locale: unknown): string {
  if (typeof providerId !== 'string') return '';
  if (!hasOwnKey(MODEL_PROVIDER_DISPLAY_NAMES, providerId)) return providerId;

  const providerLabels = MODEL_PROVIDER_DISPLAY_NAMES[providerId];
  const localizedLabel =
    typeof locale === 'string' && isProviderLabelLocale(locale)
      ? providerLabels[locale]
      : '';
  return localizedLabel || providerLabels.en || providerId;
}

export function resolveModelOptionLabel(
  selectedModelId: unknown,
  modelProfiles: unknown,
  locale: unknown,
  translate: unknown,
): string {
  const basicLabel = translatedLabel(translate, 'ask.basicTemplate');
  if (selectedModelId === BASIC_ASSISTANT_MODEL_ID) return basicLabel;

  const profiles = Array.isArray(modelProfiles) ? modelProfiles : [];
  const profileValue = profiles.find(
    (item) => isRecord(item) && item.id === selectedModelId,
  );
  if (!isRecord(profileValue)) return basicLabel;
  const profile: ModelProfileLike = profileValue;

  const profileName = typeof profile.name === 'string' ? profile.name.trim() : '';
  if (profileName) return profileName;

  const modelConfig: ModelConfigLike = isRecord(profile.model_config)
    ? profile.model_config
    : {};
  const providerName = localizedProviderName(modelConfig.provider, locale);
  const modelName = typeof modelConfig.model === 'string' ? modelConfig.model.trim() : '';
  if (providerName && modelName) return `${providerName} · ${modelName}`;
  if (modelName) return modelName;

  return translatedLabel(translate, 'ask.aiEnhanced');
}
