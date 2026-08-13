import test from 'node:test';
import assert from 'node:assert/strict';
import { access, readFile } from 'node:fs/promises';
import { createHash } from 'node:crypto';

function getPngSize(buffer: Buffer) {
  assert.equal(buffer.toString('ascii', 1, 4), 'PNG');
  return {
    width: buffer.readUInt32BE(16),
    height: buffer.readUInt32BE(20),
  };
}

function getGifSize(buffer: Buffer) {
  assert.match(buffer.toString('ascii', 0, 6), /^GIF8[79]a$/);
  return {
    width: buffer.readUInt16LE(6),
    height: buffer.readUInt16LE(8),
  };
}

function getGifMetadata(buffer: Buffer) {
  const size = getGifSize(buffer);
  let offset = 13;
  const globalColorTableFlags = buffer[10];
  if (globalColorTableFlags & 0x80) {
    offset += 3 * (2 ** ((globalColorTableFlags & 0x07) + 1));
  }

  let frameCount = 0;
  let durationCentiseconds = 0;
  let pendingDelay = 0;

  const skipSubBlocks = (start: number) => {
    let cursor = start;
    while (cursor < buffer.length) {
      const blockSize = buffer[cursor];
      cursor += 1;
      if (blockSize === 0) return cursor;
      cursor += blockSize;
    }
    throw new Error('GIF 子数据块未正常结束');
  };

  while (offset < buffer.length) {
    const marker = buffer[offset];
    if (marker === 0x3b) break;

    if (marker === 0x21) {
      const extensionLabel = buffer[offset + 1];
      if (extensionLabel === 0xf9) {
        assert.equal(buffer[offset + 2], 4, 'GIF 图形控制扩展长度应为 4');
        pendingDelay = buffer.readUInt16LE(offset + 4);
        offset += 8;
      } else {
        offset = skipSubBlocks(offset + 2);
      }
      continue;
    }

    if (marker === 0x2c) {
      const localColorTableFlags = buffer[offset + 9];
      offset += 10;
      if (localColorTableFlags & 0x80) {
        offset += 3 * (2 ** ((localColorTableFlags & 0x07) + 1));
      }
      offset += 1;
      offset = skipSubBlocks(offset);
      frameCount += 1;
      durationCentiseconds += pendingDelay;
      pendingDelay = 0;
      continue;
    }

    throw new Error(`无法识别的 GIF 数据块标记：0x${marker.toString(16)}`);
  }

  return {
    ...size,
    frameCount,
    durationMilliseconds: durationCentiseconds * 10,
  };
}

test('README 语言切换应突出当前语言且只链接其他语言', async () => {
  const [enSource, zhSource, twSource] = await Promise.all([
    readFile(new URL('./README.md', import.meta.url), 'utf8'),
    readFile(new URL('./README.zh.md', import.meta.url), 'utf8'),
    readFile(new URL('./README.tw.md', import.meta.url), 'utf8'),
  ]);

  assert.match(
    enSource,
    /<strong>English<\/strong> · <a href="\.\/*README\.zh\.md">简体中文<\/a> · <a href="\.\/*README\.tw\.md">繁體中文<\/a>/,
    '英文 README 应高亮 English，并只链接简体/繁体 README'
  );
  assert.doesNotMatch(enSource, /href="\.\/*README\.md"[^>]*>English<\/a>/);

  assert.match(
    zhSource,
    /<a href="\.\/*README\.md">English<\/a> · <strong>简体中文<\/strong> · <a href="\.\/*README\.tw\.md">繁體中文<\/a>/,
    '简体 README 应高亮简体中文，并只链接英文/繁体 README'
  );
  assert.doesNotMatch(zhSource, /href="\.\/*README\.zh\.md"[^>]*>简体中文<\/a>/);

  assert.match(
    twSource,
    /<a href="\.\/*README\.md">English<\/a> · <a href="\.\/*README\.zh\.md">简体中文<\/a> · <strong>繁體中文<\/strong>/,
    '繁体 README 应高亮繁體中文，并只链接英文/简体 README'
  );
  assert.doesNotMatch(twSource, /href="\.\/*README\.tw\.md"[^>]*>繁體中文<\/a>/);

  await access(new URL('./README.zh.md', import.meta.url));
  await access(new URL('./README.tw.md', import.meta.url));
});

test('多语言 README 底部都应展示 Star History，并在 License 后加入分隔线', async () => {
  const [zhSource, enSource, twSource] = await Promise.all([
    readFile(new URL('./README.zh.md', import.meta.url), 'utf8'),
    readFile(new URL('./README.md', import.meta.url), 'utf8'),
    readFile(new URL('./README.tw.md', import.meta.url), 'utf8'),
  ]);

  assert.match(zhSource, /## License\s+\[MIT\]\(\.\/LICENSE\)[\s\S]*---\s+## 历史星标/);
  assert.match(enSource, /## License\s+\[MIT\]\(\.\/LICENSE\)[\s\S]*---\s+## Star History/);
  assert.match(twSource, /## License\s+\[MIT\]\(\.\/LICENSE\)[\s\S]*---\s+## 歷史星標/);
  assert.match(enSource, /star-history\.com\/#wm94i\/Work-Review&Date/);
  for (const source of [zhSource, enSource, twSource]) {
    assert.match(source, /srcset="docs\/star-history-dark\.svg"/);
    assert.match(source, /srcset="docs\/star-history\.svg"/);
    assert.match(source, /<img alt="Star History" src="docs\/star-history\.svg" width="720" \/>/);
  }
});

test('README 不应把默认关闭的 Localhost API 描述为启动后自动开放', async () => {
  const sources = await Promise.all([
    readFile(new URL('./README.md', import.meta.url), 'utf8'),
    readFile(new URL('./README.zh.md', import.meta.url), 'utf8'),
    readFile(new URL('./README.tw.md', import.meta.url), 'utf8'),
  ]);

  for (const source of sources) {
    assert.doesNotMatch(source, /automatically exposes a local HTTP API after launch/);
    assert.doesNotMatch(source, /应用启动后自动在本地开放 HTTP API/);
    assert.doesNotMatch(source, /應用啟動後自動在本地開放 HTTP API/);
  }
});

test('多语言 README 应覆盖当前版本关键能力和安装资产', async () => {
  const readmes = [
    {
      file: './README.md',
      patterns: [
        /Windows \| `\.exe` \/ portable `\.zip` \|/,
        /hourly activity across Today, Week, Date, and Range views/,
        /dynamic opening prompts after a model is configured/,
        /browser sources, page counts, duration, inline expansion, and editable semantic categorization/,
      ],
    },
    {
      file: './README.zh.md',
      patterns: [
        /Windows \| `\.exe` \/ 便携版 `\.zip` \|/,
        /按今日、本周、指定日期、日期范围查看小时活跃度/,
        /配置模型后显示动态开场提示/,
        /展示浏览器来源、页面数和时长，支持卡片内展开全部网站并编辑语义分类/,
      ],
    },
    {
      file: './README.tw.md',
      patterns: [
        /Windows \| `\.exe` \/ 便攜版 `\.zip` \|/,
        /按今日、本週、指定日期、日期範圍查看小時活躍度/,
        /配置模型後顯示動態開場提示/,
        /展示瀏覽器來源、頁面數和時長，支援在卡片內展開全部網站並編輯語義分類/,
      ],
    },
  ];

  for (const readme of readmes) {
    const source = await readFile(new URL(readme.file, import.meta.url), 'utf8');
    for (const pattern of readme.patterns) {
      assert.match(source, pattern);
    }
  }
});

test('英文 README 应说明 Linux 安装包与 GLIBC 兼容性边界', async () => {
  const source = await readFile(new URL('./README.md', import.meta.url), 'utf8');

  assert.match(
    source,
    /\| Linux x86_64 \(X11 \/ Wayland\) \| `\.deb` \/ `\.rpm` \/ `\.AppImage` \|/,
  );
  assert.match(
    source,
    /\| Linux ARM64 \(aarch64\) \| `\.deb` \/ `\.rpm` \/ `\.AppImage` \|/,
  );
  assert.match(source, /AppImage is not a universal package for every Linux distribution\./);
  assert.match(
    source,
    /The GLIBC compatibility gate checks only the main program ELF inside each Linux release package; it does not scan every bundled library or plugin\./,
  );
  assert.match(
    source,
    /The current maximum allowed GLIBC version is 2\.35\./,
  );
  assert.match(
    source,
    /The actual requirement of each build is shown in the release workflow's GLIBC check output\./,
  );
});

test('简体中文 README 应说明 Linux 安装包与 GLIBC 兼容性边界', async () => {
  const source = await readFile(new URL('./README.zh.md', import.meta.url), 'utf8');

  assert.match(
    source,
    /\| Linux x86_64 \(X11 \/ Wayland\) \| `\.deb` \/ `\.rpm` \/ `\.AppImage` \|/,
  );
  assert.match(
    source,
    /\| Linux ARM64 \(aarch64\) \| `\.deb` \/ `\.rpm` \/ `\.AppImage` \|/,
  );
  assert.match(source, /AppImage 并非适用于任意 Linux 发行版的万能包。/);
  assert.match(
    source,
    /GLIBC 兼容性门禁只检查每个 Linux 发布包内的主程序 ELF，不代表扫描包内所有库或插件。/,
  );
  assert.match(source, /当前允许的 GLIBC 版本上限为 2\.35。/);
  assert.match(source, /每次构建的实际要求以发布工作流中的 GLIBC 检测输出为准。/);
});

test('繁体中文 README 应说明 Linux 安装包与 GLIBC 兼容性边界', async () => {
  const source = await readFile(new URL('./README.tw.md', import.meta.url), 'utf8');

  assert.match(
    source,
    /\| Linux x86_64 \(X11 \/ Wayland\) \| `\.deb` \/ `\.rpm` \/ `\.AppImage` \|/,
  );
  assert.match(
    source,
    /\| Linux ARM64 \(aarch64\) \| `\.deb` \/ `\.rpm` \/ `\.AppImage` \|/,
  );
  assert.match(source, /AppImage 並非適用於任意 Linux 發行版的萬用套件。/);
  assert.match(
    source,
    /GLIBC 相容性門檻只檢查每個 Linux 發布套件內的主程式 ELF，不代表掃描套件內所有函式庫或外掛程式。/,
  );
  assert.match(source, /目前允許的 GLIBC 版本上限為 2\.35。/);
  assert.match(source, /每次建置的實際需求以發布工作流程中的 GLIBC 檢測輸出為準。/);
});

test('多语言 README 应展示完整界面预览截图且图片文件存在', async () => {
  const readmes = [
    {
      file: './README.md',
      dir: 'Introduction_en',
    },
    {
      file: './README.zh.md',
      dir: 'Introduction_zh',
    },
    {
      file: './README.tw.md',
      dir: 'Introduction_tw',
    },
  ];
  const labels = [
    '概览',
    '时间线',
    '时间线详情',
    '小时总结',
    '日报',
    '助手',
    '设置-通用',
    '设置-外观',
    '设置-AI模型',
    '设置-桌面化身',
    '设置-隐私',
    '设置-存储',
    '接入管理',
    '关于',
  ];
  const expectedPngSize = { width: 2982, height: 1682 };
  const expectedGifMetadata = {
    width: 960,
    height: 541,
    frameCount: 60,
    durationMilliseconds: 7510,
  };

  for (const readme of readmes) {
    const source = await readFile(new URL(readme.file, import.meta.url), 'utf8');
    assert.match(source, /<details>/);
    assert.match(source, /<\/details>/);
    const gifPath = `docs/${readme.dir}/工作流.gif`;
    assert.match(source, new RegExp(`<img src="${gifPath}"[^>]*width="720"`));
    const gifUrl = new URL(`./${gifPath}`, import.meta.url);
    await access(gifUrl);
    const gifMetadata = getGifMetadata(await readFile(gifUrl));
    assert.deepEqual(gifMetadata, expectedGifMetadata);
    for (const label of labels) {
      const imagePath = `docs/${readme.dir}/${label}.png`;
      assert.match(source, new RegExp(`<img src="${imagePath}"[^>]*width="720"`));
      const imageUrl = new URL(`./${imagePath}`, import.meta.url);
      await access(imageUrl);
      const pngSize = getPngSize(await readFile(imageUrl));
      assert.deepEqual(pngSize, expectedPngSize);
    }
  }
});

test('README 截图脚本依赖应在项目中精确声明', async () => {
  const packageJson = JSON.parse(await readFile(new URL('./package.json', import.meta.url), 'utf8'));
  assert.equal(packageJson.devDependencies?.playwright, '1.61.1');
});

test('开发文档与自动化流程应统一要求 Node.js 22+', async () => {
  const [packageSource, englishReadme, simplifiedReadme, traditionalReadme, ci, release] =
    await Promise.all([
      readFile(new URL('./package.json', import.meta.url), 'utf8'),
      readFile(new URL('./README.md', import.meta.url), 'utf8'),
      readFile(new URL('./README.zh.md', import.meta.url), 'utf8'),
      readFile(new URL('./README.tw.md', import.meta.url), 'utf8'),
      readFile(new URL('./.github/workflows/ci.yml', import.meta.url), 'utf8'),
      readFile(new URL('./.github/workflows/release.yml', import.meta.url), 'utf8'),
    ]);
  const packageJson = JSON.parse(packageSource);

  assert.equal(packageJson.engines?.node, '>=22');
  assert.match(englishReadme, /Requires: Node\.js 22\+/);
  assert.match(simplifiedReadme, /要求：Node\.js 22\+/);
  assert.match(traditionalReadme, /要求：Node\.js 22\+/);
  assert.doesNotMatch(englishReadme, /Node\.js 18\+/);
  assert.doesNotMatch(simplifiedReadme, /Node\.js 18\+/);
  assert.doesNotMatch(traditionalReadme, /Node\.js 18\+/);
  assert.match(ci, /node-version:\s*22/);
  assert.match(release, /node-version:\s*22/);
});

test('多语言 README 的社区图片应使用统一规格展示资产', async () => {
  const sources = await Promise.all([
    readFile(new URL('./README.md', import.meta.url), 'utf8'),
    readFile(new URL('./README.zh.md', import.meta.url), 'utf8'),
    readFile(new URL('./README.tw.md', import.meta.url), 'utf8'),
  ]);
  const imagePaths = [
    'docs/group/wechat-group.png',
    'docs/group/official-account.png',
  ];

  for (const source of sources) {
    assert.doesNotMatch(source, /docs\/group\/vx\.jpg/);
    assert.doesNotMatch(source, /docs\/group\/gzh\.jpg/);
    for (const imagePath of imagePaths) {
      assert.match(source, new RegExp(`<img src="${imagePath}"[^>]*width="220"`));
    }
  }

  // 两张社区图片（微信群二维码 + 公众号二维码）内容不同、来源不同，
  // 底层 PNG 像素尺寸不必一致（它们在 README 里都统一以 width="220" 展示，
  // 上面的 HTML 断言已覆盖"统一规格"的意图）。这里只验证文件存在且是有效 PNG。
  for (const imagePath of imagePaths) {
    const imageUrl = new URL(`./${imagePath}`, import.meta.url);
    await access(imageUrl);
    const size = getPngSize(await readFile(imageUrl));
    assert.ok(size.width > 0 && size.height > 0, `${imagePath} 应是有效 PNG`);
  }
});

test('多语言 README 的设置页面截图不应重复使用同一画面', async () => {
  const dirs = ['Introduction_en', 'Introduction_zh', 'Introduction_tw'];
  const labels = ['设置-通用', '设置-存储', '设置-桌面化身', '设置-隐私'];

  for (const dir of dirs) {
    const hashes = new Map();
    for (const label of labels) {
      const buffer = await readFile(new URL(`./docs/${dir}/${label}.png`, import.meta.url));
      hashes.set(label, createHash('sha256').update(buffer).digest('hex'));
    }

    assert.notEqual(
      hashes.get('设置-通用'),
      hashes.get('设置-存储'),
      `${dir} 的通用设置与存储设置截图不应相同`
    );
    assert.notEqual(
      hashes.get('设置-通用'),
      hashes.get('设置-桌面化身'),
      `${dir} 的通用设置与桌面化身设置截图不应相同`
    );
    assert.notEqual(
      hashes.get('设置-通用'),
      hashes.get('设置-隐私'),
      `${dir} 的通用设置与隐私设置截图不应相同`
    );
  }
});
