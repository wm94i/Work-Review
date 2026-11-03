//! Windows 以 EXE 同级的 Data 目录显式启用便携模式；其他情况保留本机目录偏好。
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize)]
struct DataDirPreference {
    data_dir: String,
}

fn system_data_dir() -> PathBuf {
    dirs::data_dir()
        .map(|dir| dir.join("work-review"))
        .unwrap_or_else(|| PathBuf::from("./data"))
}

fn preference_path() -> PathBuf {
    dirs::config_dir()
        .map(|dir| dir.join("work-review").join("data-location.json"))
        .unwrap_or_else(|| PathBuf::from("./work-review-data-location.json"))
}

fn portable_dir_at(executable: &Path) -> io::Result<Option<PathBuf>> {
    if !cfg!(target_os = "windows") {
        return Ok(None);
    }
    let directory = executable
        .parent()
        .ok_or_else(|| io::Error::other("无法确定程序所在目录"))?
        .join("Data");
    // 文件、损坏的链接和无权访问的标记不能被误认为“未启用”，否则会切到另一份数据。
    match fs::symlink_metadata(&directory) {
        Ok(_) => Ok(Some(directory)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(io::Error::new(
            error.kind(),
            format!(
                "无法检查便携目录 {}：{error}。已停止操作，不会改用其他数据目录。",
                directory.display()
            ),
        )),
    }
}

pub(crate) fn is_portable_mode() -> io::Result<bool> {
    Ok(portable_dir_at(&std::env::current_exe()?)?.is_some())
}

pub(crate) fn default_data_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|executable| portable_dir_at(&executable).ok().flatten())
        .unwrap_or_else(system_data_dir)
}

fn load_preference(path: &Path) -> Option<PathBuf> {
    let content = fs::read_to_string(path).ok()?;
    let preference: DataDirPreference = serde_json::from_str(&content).ok()?;
    let directory = preference.data_dir.trim();
    (!directory.is_empty()).then(|| PathBuf::from(directory))
}

fn validate_portable_target(portable: &Path, target: &Path) -> io::Result<()> {
    if target.canonicalize().ok().as_ref() == Some(&portable.canonicalize()?) {
        return Ok(());
    }
    Err(io::Error::other(format!(
        "便携模式固定使用程序旁的 Data 目录（{}），未迁移任何数据。请退出程序后移动整个便携目录；如需使用自定义位置，请先备份并移走 Data 目录后重新启动。",
        portable.display()
    )))
}

pub(crate) fn validate_data_dir_target(target: &Path) -> io::Result<()> {
    if let Some(portable) = portable_dir_at(&std::env::current_exe()?)? {
        validate_portable_target(&portable, target)?;
    }
    Ok(())
}

pub(crate) fn save_data_dir_preference(directory: &Path) -> io::Result<()> {
    save_preference_at(
        &std::env::current_exe()?,
        &system_data_dir(),
        &preference_path(),
        directory,
    )
}

fn save_preference_at(
    executable: &Path,
    default: &Path,
    preference: &Path,
    directory: &Path,
) -> io::Result<()> {
    if let Some(portable) = portable_dir_at(executable)? {
        validate_portable_target(&portable, directory)?;
        // 标记本身就是可搬移的选择；绝不写入或删除这台电脑原有的绝对路径偏好。
        return Ok(());
    }
    if directory == default {
        if preference.exists() {
            fs::remove_file(preference)?;
        }
        return Ok(());
    }
    if let Some(parent) = preference.parent() {
        fs::create_dir_all(parent)?;
    }
    let content = serde_json::to_string_pretty(&DataDirPreference {
        data_dir: directory.to_string_lossy().to_string(),
    })
    .map_err(io::Error::other)?;
    fs::write(preference, content)
}

fn ensure_data_dir(path: &Path) -> io::Result<PathBuf> {
    fs::create_dir_all(path)?;
    Ok(path.canonicalize().unwrap_or_else(|_| path.to_path_buf()))
}

fn ensure_portable_dir(directory: &Path) -> io::Result<PathBuf> {
    let directory = directory.canonicalize()?;
    fs::read_dir(&directory)?;
    for name in ["config.json", "config.json.bak", "workreview.db"] {
        match OpenOptions::new()
            .read(true)
            .write(true)
            .open(directory.join(name))
        {
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    let probe_path = directory.join(format!(".work-review-write-{}", uuid::Uuid::new_v4()));
    let mut probe = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe_path)?;
    let written = probe.write_all(b"portable");
    drop(probe);
    let removed = fs::remove_file(&probe_path);
    written?;
    removed?;
    Ok(directory)
}

pub(crate) fn resolve_data_dir() -> io::Result<PathBuf> {
    resolve_at(
        &std::env::current_exe()?,
        &system_data_dir(),
        &preference_path(),
    )
}

fn resolve_at(executable: &Path, default: &Path, preference: &Path) -> io::Result<PathBuf> {
    if let Some(portable) = portable_dir_at(executable)? {
        return ensure_portable_dir(&portable).map_err(|error| {
            io::Error::new(error.kind(), format!(
                "无法读写便携数据目录 {}：{error}。已停止启动，不会回退、迁移或覆盖其他目录的数据。请将整个程序目录移至可读写位置。",
                portable.display()
            ))
        });
    }
    let preferred = load_preference(preference).unwrap_or_else(|| default.to_path_buf());
    let directory = match ensure_data_dir(&preferred) {
        Ok(directory) => directory,
        Err(error) => {
            log::warn!("创建数据目录失败，回退默认目录: {error}");
            if preferred != default {
                if let Ok(directory) = ensure_data_dir(default) {
                    if let Err(error) =
                        save_preference_at(executable, default, preference, &directory)
                    {
                        log::warn!("保存回退目录偏好失败: {error}");
                    }
                    migrate_legacy_data_dir(executable, &directory);
                    return Ok(directory);
                }
            }
            let fallback = PathBuf::from("./data");
            if let Err(error) = fs::create_dir_all(&fallback) {
                log::warn!("创建兜底数据目录失败: {error}");
            }
            fallback
        }
    };
    migrate_legacy_data_dir(executable, &directory);
    Ok(directory)
}

fn migrate_legacy_data_dir(executable: &Path, target: &Path) {
    let Some(legacy) = executable.parent().map(|parent| parent.join("data")) else {
        return;
    };
    if legacy == target || !legacy.exists() {
        return;
    }
    if ["config.json", "workreview.db", "screenshots"]
        .iter()
        .any(|name| target.join(name).exists())
    {
        return;
    }
    if let Err(error) = crate::copy_dir_contents(&legacy, target, false) {
        log::warn!("迁移旧版数据目录失败: {error}");
    } else {
        log::info!("已将旧版数据目录迁移到稳定目录: {target:?}");
    }
}

pub(crate) fn report_startup_error(error: &io::Error) {
    log::error!("无法初始化数据目录: {error}");
    eprintln!("无法初始化数据目录: {error}");
    #[cfg(target_os = "windows")]
    {
        use winapi::um::winuser::{MessageBoxW, MB_ICONERROR, MB_OK};
        let title: Vec<u16> = "Work Review".encode_utf16().chain(Some(0)).collect();
        let message: Vec<u16> = error.to_string().encode_utf16().chain(Some(0)).collect();
        unsafe {
            MessageBoxW(
                std::ptr::null_mut(),
                message.as_ptr(),
                title.as_ptr(),
                MB_OK | MB_ICONERROR,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(target_os = "windows")]
    use work_review_core::config::AppConfig;
    #[cfg(target_os = "windows")]
    use work_review_core::database::{Activity, Database};

    struct TestDirs(PathBuf);

    impl TestDirs {
        fn new() -> Self {
            let root =
                std::env::temp_dir().join(format!("work-review-portable-{}", uuid::Uuid::new_v4()));
            fs::create_dir_all(root.join("app")).unwrap();
            Self(root.canonicalize().unwrap())
        }
        fn executable(&self) -> PathBuf {
            self.0.join("app/Work_Review.exe")
        }
        fn default(&self) -> PathBuf {
            self.0.join("system-data")
        }
        fn preference(&self) -> PathBuf {
            self.0.join("system-config/data-location.json")
        }
        fn portable(&self) -> PathBuf {
            self.0.join("app/Data")
        }
        fn resolve(&self) -> io::Result<PathBuf> {
            resolve_at(&self.executable(), &self.default(), &self.preference())
        }
    }

    impl Drop for TestDirs {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn 无便携标记时保持默认和自定义目录行为() {
        let dirs = TestDirs::new();
        assert_eq!(dirs.resolve().unwrap(), dirs.default());
        let custom = dirs.0.join("custom");
        save_preference_at(
            &dirs.executable(),
            &dirs.default(),
            &dirs.preference(),
            &custom,
        )
        .unwrap();
        assert_eq!(dirs.resolve().unwrap(), custom);
        save_preference_at(
            &dirs.executable(),
            &dirs.default(),
            &dirs.preference(),
            &dirs.default(),
        )
        .unwrap();
        assert!(!dirs.preference().exists());
        assert_eq!(dirs.resolve().unwrap(), dirs.default());
    }

    #[test]
    fn 失效的普通目录偏好仍可回退默认目录() {
        let dirs = TestDirs::new();
        let invalid = dirs.0.join("not-a-directory");
        fs::write(&invalid, b"user-file").unwrap();
        save_preference_at(
            &dirs.executable(),
            &dirs.default(),
            &dirs.preference(),
            &invalid,
        )
        .unwrap();
        assert_eq!(dirs.resolve().unwrap(), dirs.default());
        assert!(!dirs.preference().exists());
        assert_eq!(fs::read(invalid).unwrap(), b"user-file");
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn 便携目录优先且不读取迁移或改写本机数据偏好() {
        let dirs = TestDirs::new();
        fs::create_dir_all(dirs.portable()).unwrap();
        let custom = dirs.0.join("old-custom");
        fs::create_dir_all(&custom).unwrap();
        fs::write(custom.join("config.json"), b"old-config").unwrap();
        fs::create_dir_all(dirs.preference().parent().unwrap()).unwrap();
        fs::write(
            dirs.preference(),
            serde_json::to_vec(&DataDirPreference {
                data_dir: custom.to_string_lossy().into_owned(),
            })
            .unwrap(),
        )
        .unwrap();
        let before = fs::read(dirs.preference()).unwrap();
        assert_eq!(dirs.resolve().unwrap(), dirs.portable());
        save_preference_at(
            &dirs.executable(),
            &dirs.default(),
            &dirs.preference(),
            &dirs.portable(),
        )
        .unwrap();
        assert_eq!(fs::read(dirs.preference()).unwrap(), before);
        assert_eq!(fs::read(custom.join("config.json")).unwrap(), b"old-config");
        assert!(!dirs.default().exists());
        assert_eq!(fs::read_dir(dirs.portable()).unwrap().count(), 0);
        assert!(save_preference_at(
            &dirs.executable(),
            &dirs.default(),
            &dirs.preference(),
            &custom
        )
        .is_err());
        assert_eq!(fs::read(dirs.preference()).unwrap(), before);
        assert!(!dirs.portable().join("data-location.json").exists());
        assert!(validate_portable_target(&dirs.portable(), &dirs.portable().join(".")).is_ok());
        let outside = dirs.0.join("outside");
        assert!(validate_portable_target(&dirs.portable(), &outside).is_err());
        assert!(!outside.exists());

        fs::rename(dirs.portable(), dirs.0.join("saved-Data")).unwrap();
        assert_eq!(dirs.resolve().unwrap(), custom);
        assert_eq!(fs::read(dirs.preference()).unwrap(), before);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn 移动整个便携目录后配置数据库和相对截图仍可读取() {
        let dirs = TestDirs::new();
        fs::create_dir_all(dirs.portable().join("screenshots/day")).unwrap();
        let data = dirs.resolve().unwrap();
        let config = AppConfig {
            report_editor_font: "Microsoft YaHei".into(),
            ..AppConfig::default()
        };
        config.save(&data.join("config.json")).unwrap();
        let screenshot = "screenshots/day/test.jpg";
        fs::write(data.join(screenshot), b"test-image").unwrap();
        let db = Database::new(&data.join("workreview.db")).unwrap();
        let activity = Activity {
            id: None,
            timestamp: 1_700_000_000,
            app_name: "Notepad".into(),
            window_title: "portable test".into(),
            screenshot_path: screenshot.into(),
            ocr_text: None,
            category: "work".into(),
            duration: 30,
            browser_url: None,
            executable_path: None,
            semantic_category: None,
            semantic_confidence: None,
            screenshot_url: None,
        };
        let id = db.insert_activity(&activity).unwrap();
        drop(db);
        let moved = dirs.0.join("moved/工作回顾");
        fs::create_dir_all(moved.parent().unwrap()).unwrap();
        fs::rename(dirs.executable().parent().unwrap(), &moved).unwrap();
        let new_data = resolve_at(
            &moved.join("Work_Review.exe"),
            &dirs.default(),
            &dirs.preference(),
        )
        .unwrap();
        assert_eq!(new_data, moved.join("Data"));
        assert_eq!(
            AppConfig::load(&new_data.join("config.json"))
                .unwrap()
                .report_editor_font,
            "Microsoft YaHei"
        );
        let reopened = Database::new(&new_data.join("workreview.db")).unwrap();
        let record = reopened.get_activity_by_id(id).unwrap().unwrap();
        assert_eq!(record.duration, 30);
        assert_eq!(
            fs::read(new_data.join(record.screenshot_path)).unwrap(),
            b"test-image"
        );
        assert!(!dirs.preference().exists());
        assert!(!dirs.default().exists());
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn 便携标记不是目录时必须报错而非回退() {
        let dirs = TestDirs::new();
        fs::write(dirs.portable(), b"not-a-directory").unwrap();
        assert!(dirs.resolve().is_err());
        assert!(!dirs.default().exists());
        assert!(!dirs.preference().exists());
        assert_eq!(fs::read(dirs.portable()).unwrap(), b"not-a-directory");
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn 便携配置只读时必须报错且保留原文件() {
        let dirs = TestDirs::new();
        fs::create_dir_all(dirs.portable()).unwrap();
        let config = dirs.portable().join("config.json");
        fs::write(&config, b"read-only-config").unwrap();
        let original = fs::metadata(&config).unwrap().permissions();
        let mut read_only = original.clone();
        read_only.set_readonly(true);
        fs::set_permissions(&config, read_only).unwrap();
        let result = dirs.resolve();
        fs::set_permissions(&config, original).unwrap();
        assert!(result.is_err());
        assert_eq!(fs::read(&config).unwrap(), b"read-only-config");
        assert!(!dirs.default().exists());
        assert!(!dirs.preference().exists());
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn 其他平台不启用便携标记() {
        let dirs = TestDirs::new();
        fs::create_dir_all(dirs.portable()).unwrap();
        assert_eq!(dirs.resolve().unwrap(), dirs.default());
    }
}
