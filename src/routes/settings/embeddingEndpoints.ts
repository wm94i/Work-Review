export type EmbeddingProvider = 'ollama' | 'openai';

export interface EmbeddingPreset {
  id: string;
  provider: EmbeddingProvider;
  labelKey: string;
  endpoint: string;
  model: string;
  clearApiKey?: boolean;
}

/**
 * 严格判断嵌入端点是否指向本机回环地址（localhost / *.localhost / 127.0.0.0/8 / ::1）。
 * 仅本机回环才视为“数据不出电脑”；局域网 IP 或公网域名均按外发提示，
 * 并通过标准 URL 解析拒绝 `http://localhost@evil.com`、`http://127.0.0.1.evil.com` 等欺骗地址。
 */
export function isLocalLoopbackEmbeddingEndpoint(endpoint: string | null | undefined): boolean {
  const raw = (endpoint ?? '').trim();
  if (!raw) return false;

  let parsed: URL;
  try {
    parsed = new URL(raw);
  } catch {
    return false;
  }

  if (parsed.protocol !== 'http:' && parsed.protocol !== 'https:') {
    return false;
  }

  if (parsed.username || parsed.password) {
    return false;
  }

  const host = parsed.hostname.replace(/^\[|\]$/g, '').replace(/\.$/, '').toLowerCase();
  if (!host) return false;
  if (host === 'localhost' || host.endsWith('.localhost')) {
    return true;
  }
  if (host === '::1' || host === '0:0:0:0:0:0:0:1') {
    return true;
  }

  const ipv4Match = /^(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})$/.exec(host);
  if (ipv4Match) {
    const octets = ipv4Match.slice(1).map(Number);
    if (octets.every((n) => n >= 0 && n <= 255) && octets[0] === 127) {
      return true;
    }
  }
  return false;
}

export const OLLAMA_EMBEDDING_PRESETS: readonly EmbeddingPreset[] = [
  {
    id: 'ollama-dmeta-zh',
    provider: 'ollama',
    labelKey: 'settingsAI.semanticMemory.presetZhSmall',
    endpoint: 'http://localhost:11434',
    model: 'shaw/dmeta-embedding-zh',
  },
  {
    id: 'ollama-bge-m3',
    provider: 'ollama',
    labelKey: 'settingsAI.semanticMemory.presetBgeM3',
    endpoint: 'http://localhost:11434',
    model: 'bge-m3',
  },
  {
    id: 'ollama-nomic',
    provider: 'ollama',
    labelKey: 'settingsAI.semanticMemory.presetNomic',
    endpoint: 'http://localhost:11434',
    model: 'nomic-embed-text',
  },
];

export const OPENAI_COMPAT_LOCAL_PRESETS: readonly EmbeddingPreset[] = [
  {
    id: 'local-lmstudio',
    provider: 'openai',
    labelKey: 'settingsAI.semanticMemory.presetLmStudio',
    endpoint: 'http://127.0.0.1:1234/v1',
    model: 'text-embedding-bge-small-zh-v1.5',
    clearApiKey: true,
  },
  {
    id: 'local-llamacpp',
    provider: 'openai',
    labelKey: 'settingsAI.semanticMemory.presetLlamaCpp',
    endpoint: 'http://127.0.0.1:8080/v1',
    model: 'bge-small-zh-v1.5',
    clearApiKey: true,
  },
];
