# Release Notes（多语言）

每个版本的**多语言 release notes** 存放在此目录，采用「主体 + 独立语言文件 + 链接切换」模式（参考 [cc-switch](https://github.com/farion1231/cc-switch)）。

## 机制

- **中文主体**：由 `.github/workflows/release.yml` 的 `Extract changelog` 步骤从 `CHANGELOG.md` 对应版本段落自动提取，作为 GitHub Release 的 body。
- **英文 / 繁体版本**：作为独立文件放在本目录；发布时 CI 自动在 Release body 顶部生成指向它们的切换链接。

Release body 形如：

```markdown
**[English →](./v1.0.51-en.md) | [繁體中文 →](./v1.0.51-tw.md)**

---

（以下为中文主体，从 CHANGELOG 自动提取）

### 修复
- ...

### macOS 安装说明
...
```

## 命名规范

针对版本 `1.0.51`（tag `v1.0.51`）：

| 文件 | 语言 | 必需性 |
|---|---|---|
| `v1.0.51-en.md` | English | 可选（存在则自动加链接） |
| `v1.0.51-tw.md` | 繁體中文 | 可选（存在则自动加链接） |

中文不需要独立文件——主体已在 Release body 与 `CHANGELOG.md`。

## 为新版本添加多语言 notes

1. 在 `CHANGELOG.md` 写好该版本中文变更（`## [1.0.51] - YYYY-MM-DD`）—— 这是主体，CI 自动提取。
2. 在本目录创建 `v1.0.51-en.md` / `v1.0.51-tw.md`，翻译该版本内容。
3. **先提交这些文件，再打 tag** `v1.0.51` 并推送——CI 在 tag 对应的 commit 里查找语言文件。
4. CI 自动在 Release body 顶部加入 `English / 繁體中文` 切换链接。

某版本若没有 en/tw 文件，Release body 就只有中文主体（向后兼容，不会报错）。

## Release Candidates

- RC tags use `vMAJOR.MINOR.PATCH-rc.N`, for example `v1.1.3-rc.1`. Leading zeros in numeric components are rejected.
- Synchronize `package.json`, `package-lock.json`, `src-tauri/tauri.conf.json`, `src-tauri/Cargo.toml`, and `Cargo.lock` before tagging. They may contain either the RC's base version (for example `1.1.4`) or its exact full version (`1.1.4-rc.1`).
- Both preflight and each build job run `scripts/prepare-release-version.mjs`: validate the strict tag and all five sources, then apply the full RC version only in the runner checkout. The executable, installer and updater manifest must report the same full version, so later RCs and the corresponding stable release compare correctly. This does not rewrite the source commit or tag. For local RC builds from a base-version source checkout, explicitly run the same script with the exact RC tag before building; use a disposable checkout.
- Add a matching changelog section, including known validation gaps. RC notes use the same bilingual file mechanism as stable releases (Simplified Chinese body in `CHANGELOG.md`, plus `-en.md` / `-tw.md` files in this directory).
- Commit and push the prepared changes to `main`. Only after approving publication, create and push the RC tag at the current `origin/main` commit.
- The existing five-platform build matrix and all release gates also apply to RCs. A release is published only after all build jobs succeed.
- RC releases set `prerelease: true` and `makeLatest: false`. Stable clients retain their existing `/releases/latest/download/` endpoints. Receiving test versions is a separate local opt-in, disabled for new and existing settings by default.
- Users can enable “Receive test versions” on About and reuse the single check button. Opted-in clients select the highest published SemVer from the recent releases, excluding drafts. Test updates always show a warning and require confirmation; supported platforms use the selected tag's signed updater manifest, while other platforms can open its release page.
- Only newer versions are offered: `1.1.3` must not downgrade to `1.1.3-rc.3`, `rc.10` is newer than `rc.3`, and `1.1.4` is newer than `1.1.4-rc.1`. Turning off test updates does not automatically downgrade an installed RC. Downloads are pinned to the exact confirmed version, including proxy fallbacks.
- Before installation, fetch the target release's asset SHA-256 from the official GitHub HTTPS API, preserve Tauri package-signature verification, and compare the downloaded bytes against that independent digest. Never use a proxy-supplied digest as the trust anchor. Missing or mismatched official digests fail closed; an unavailable/rate-limited API requires retrying later or manually downloading from the release page.
- Test with backed-up or disposable data. Verify Windows OCR, fast application switching, privacy saves after restart, category management, data migration, and installer upgrades before promoting to a stable version.
- A packaging/build failure does not authorize incrementing the RC version. Explain the failure and obtain approval before changing the planned version.
- Never move tags for published releases. A failed, unpublished RC tag may be recreated at a corrected `origin/main` commit only with explicit user authorization; rerun the complete release gates. Delete only the failed runs named by the user. Withdrawing any successful release or its tag requires separate approval.
- Promotion requires a separate stable version bump and tag; do not relabel an RC release or move a published tag.
