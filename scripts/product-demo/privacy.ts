export interface PrivacyFinding {
  source: string;
  rule:
    | 'macos-home'
    | 'windows-home'
    | 'email'
    | 'bearer-token'
    | 'api-key'
    | 'cookie'
    | 'forbidden-domain'
    | 'external-url'
    | 'forbidden-capability';
  match: string;
}

type FindingRule = PrivacyFinding['rule'];

type TextRule = {
  rule: FindingRule;
  pattern: RegExp;
  mask?: (match: string) => string;
};

const ALLOWED_HTTP_HOSTS = new Set(['docs.example.test', '127.0.0.1', 'localhost', '::1']);
const FORBIDDEN_DOMAINS = ['github.com', 'openai.com'] as const;

const SECRET_MASK = '***';

const TEXT_RULES: readonly TextRule[] = [
  {
    rule: 'macos-home',
    pattern: /\/Users\/[^/\s"'`\\]+(?:\/[^\s"'`<>]*)?/giu,
  },
  {
    rule: 'windows-home',
    pattern: /[A-Za-z]:\\Users\\[^\\\s"'`]+(?:\\[^\s"'`<>]*)?/giu,
  },
  {
    rule: 'email',
    pattern: /\b[A-Z0-9._%+-]+@[A-Z0-9.-]+\.[A-Z]{2,}\b/giu,
  },
  {
    rule: 'bearer-token',
    pattern: /\bBearer\s+[A-Z0-9._~+/=-]{8,}/giu,
    mask: () => `Bearer ${SECRET_MASK}`,
  },
  {
    rule: 'api-key',
    pattern:
      /\b(?:OPENAI_API_KEY|API[_ -]?KEY|APIKEY)\b\s*(?:=|:)\s*(?:["']?)[^\s"',;}]{8,}(?:["']?)/giu,
    mask: (match) => `${match.split(/\s*(?:=|:)\s*/u, 1)[0]}=${SECRET_MASK}`,
  },
  {
    rule: 'cookie',
    pattern: /\b(?:Cookie|Set-Cookie)\s*:\s*[^\r\n]+/giu,
    mask: (match) => `${match.slice(0, match.indexOf(':') + 1)} ${SECRET_MASK}`,
  },
  {
    rule: 'forbidden-capability',
    pattern: /多选片段生成/gu,
  },
  {
    rule: 'forbidden-capability',
    pattern: /独立\s*OCR\s*开关/giu,
  },
  {
    rule: 'forbidden-capability',
    pattern: /关闭所有\s*AI/giu,
  },
  {
    rule: 'forbidden-capability',
    pattern: /所有数据永远不会离开本机/gu,
  },
];

function isForbiddenDomain(hostname: string): boolean {
  const normalized = hostname.toLowerCase().replace(/\.$/u, '');
  return FORBIDDEN_DOMAINS.some(
    (domain) => normalized === domain || normalized.endsWith(`.${domain}`),
  );
}

function normalizeUrlMatch(match: string): string {
  return match.replace(/[.,;:!?，。；：！？]+$/u, '');
}

function maskUrlSecrets(match: string): string {
  return match.replace(
    /([?&](?:api[_-]?key|apikey|access[_-]?token|token)=)[^&#\s]+/giu,
    `$1${SECRET_MASK}`,
  );
}

function scanHttpUrls(text: string, source: string): PrivacyFinding[] {
  const findings: PrivacyFinding[] = [];
  const urlPattern = /https?:\/\/[^\s"'<>（）()[\]{}]+/giu;

  for (const rawMatch of text.matchAll(urlPattern)) {
    const match = normalizeUrlMatch(rawMatch[0]);
    let url: URL;
    try {
      url = new URL(match);
    } catch {
      continue;
    }

    const hostname = url.hostname.toLowerCase();
    const safeMatch = maskUrlSecrets(match);
    if (isForbiddenDomain(hostname)) {
      findings.push({ source, rule: 'forbidden-domain', match: safeMatch });
    } else if (!ALLOWED_HTTP_HOSTS.has(hostname)) {
      findings.push({ source, rule: 'external-url', match: safeMatch });
    }
  }

  return findings;
}

function scanBareForbiddenDomains(text: string, source: string): PrivacyFinding[] {
  const findings: PrivacyFinding[] = [];
  const domainPattern = /(?:[a-z0-9-]+\.)*(?:github\.com|openai\.com)\b/giu;

  for (const rawMatch of text.matchAll(domainPattern)) {
    const index = rawMatch.index ?? 0;
    const prefix = text.slice(Math.max(0, index - 8), index).toLowerCase();
    if (/https?:\/\/$/u.test(prefix)) continue;
    findings.push({ source, rule: 'forbidden-domain', match: rawMatch[0] });
  }

  return findings;
}

function uniqueFindings(findings: PrivacyFinding[]): PrivacyFinding[] {
  const seen = new Set<string>();
  return findings.filter((finding) => {
    const key = `${finding.source}\0${finding.rule}\0${finding.match}`;
    if (seen.has(key)) return false;
    seen.add(key);
    return true;
  });
}

export function scanSensitiveText(text: string, source: string): PrivacyFinding[] {
  const findings: PrivacyFinding[] = [];

  for (const { rule, pattern, mask } of TEXT_RULES) {
    for (const match of text.matchAll(pattern)) {
      findings.push({
        source,
        rule,
        match: mask ? mask(match[0]) : match[0],
      });
    }
  }

  findings.push(...scanHttpUrls(text, source));
  findings.push(...scanBareForbiddenDomains(text, source));
  return uniqueFindings(findings);
}

function childSource(source: string, key: string | number, isArray: boolean): string {
  return isArray ? `${source}[${key}]` : `${source}.${key}`;
}

export function scanDemoObject(value: unknown, source: string): PrivacyFinding[] {
  const findings: PrivacyFinding[] = [];
  const visited = new WeakSet<object>();

  function visit(current: unknown, currentSource: string): void {
    if (typeof current === 'string') {
      findings.push(...scanSensitiveText(current, currentSource));
      return;
    }
    if (current === null || typeof current !== 'object') return;
    if (visited.has(current)) return;
    visited.add(current);

    if (Array.isArray(current)) {
      current.forEach((item, index) => visit(item, childSource(currentSource, index, true)));
      return;
    }

    for (const [key, item] of Object.entries(current)) {
      visit(item, childSource(currentSource, key, false));
    }
  }

  visit(value, source);
  return uniqueFindings(findings);
}

export function assertNoSensitiveContent(findings: readonly PrivacyFinding[]): void {
  if (findings.length === 0) return;

  const details = findings
    .map((finding) => `- ${finding.source} [${finding.rule}]: ${finding.match}`)
    .join('\n');
  throw new Error(`演示内容隐私与产品真实性扫描失败（${findings.length} 项）：\n${details}`);
}
