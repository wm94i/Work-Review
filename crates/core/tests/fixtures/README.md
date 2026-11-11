# SQLite upgrade fixtures

`sqlite-3.44.0.sqlite` and its matching `sqlite-3.44.0.sqlite-wal` contain **only synthetic test data**, generated with the previously shipped `rusqlite 0.30.0` / bundled SQLite `3.44.0` on 2026-11-11. No user database or screenshots were copied.

The database uses 4096-byte pages and the old eight-column `activities` schema, plus `daily_reports` without `fallback_reason`. One activity (30 seconds, `Legacy Editor`) and one Chinese report (`2026-11-01`) are checkpointed in the main file. A second activity (45 seconds, `Legacy Browser`) and report (`2026-11-02`) are committed only in the WAL. The pair was copied while the synthetic writer was idle and before closing it, so closing could not checkpoint away the WAL fixture. No SHM file is necessary.

Tests copy these files into a unique temporary directory before opening them. They verify the old database both alone and with its legitimate, matching WAL, schema migration, data preservation, SQL limits, backup and reopen under the patched SQLite. Fixtures must never be opened in place or used as application data.

These are normal compatibility fixtures, not an exploit for CVE-2026-119375. A separate test asserts the minimum patched bundled SQLite version. The complete one-shot generator and generation log are retained with the local `1.1.4-rc.2` release verification evidence.
