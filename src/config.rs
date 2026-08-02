use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::error::{AppError, Result};

pub const APP_NAME: &str = "SELFsonic";
pub const API_VERSION: &str = "1.16.1";
pub const CLIENT_NAME: &str = "SELFsonic";
/// Шлях старого (перейменованого) конфігу — для одноразової міграції.
const LEGACY_APP_NAME: &str = "subsonic-tui";

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Config {
    pub server: ServerConfig,
    #[serde(default)]
    pub audio: AudioConfig,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ServerConfig {
    /// Базовий URL, напр. `http://127.0.0.1:4533`
    pub url: String,
    pub username: String,
    pub password: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AudioConfig {
    /// Початкова гучність 0.0–1.0
    #[serde(default = "default_volume")]
    pub volume: f32,
    /// Крок перемотування `[`/`]`, секунди
    #[serde(default = "default_seek_step")]
    pub seek_step: u64,
}

fn default_volume() -> f32 {
    0.8
}

fn default_seek_step() -> u64 {
    10
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            volume: default_volume(),
            seek_step: default_seek_step(),
        }
    }
}

impl Config {
    pub fn default_path() -> Result<PathBuf> {
        let dir = config_dir()?;
        Ok(dir.join("config.toml"))
    }

    /// Якщо нового конфігу немає, а старий (`~/.config/subsonic-tui`) існує —
    /// перенести його (одноразова міграція після перейменування).
    fn migrate_legacy_config(path: &Path) {
        if path.exists() {
            return;
        }
        let legacy = dirs::config_dir().map(|d| d.join(LEGACY_APP_NAME).join("config.toml"));
        let Some(legacy) = legacy else { return };
        if !legacy.exists() {
            return;
        }
        match std::fs::read_to_string(&legacy) {
            Ok(raw) => {
                if write_config(path, &raw).is_ok() {
                    warn!(
                        "config migrated: {} → {}",
                        legacy.display(),
                        path.display()
                    );
                }
            }
            Err(e) => warn!("failed to read legacy config {}: {e}", legacy.display()),
        }
    }

    pub fn load(path: &Path) -> Result<Self> {
        Self::migrate_legacy_config(path);
        if !path.exists() {
            let template = Self::template();
            write_config(path, &toml::to_string(&template).map_err(|e| AppError::Config(format!("serialize: {e}")))?)?;
            warn!("config not found, template created: {}", path.display());
            return Ok(template);
        }
        let raw = fs::read_to_string(path)
            .map_err(|e| AppError::io(path.to_path_buf(), e))?;
        let cfg: Config = toml::from_str(&raw)
            .map_err(|e| AppError::Config(format!("{}: {e}", path.display())))?;
        Ok(cfg)
    }

    fn template() -> Self {
        Self {
            server: ServerConfig {
                url: "http://127.0.0.1:4533".into(),
                username: "user".into(),
                password: "password".into(),
            },
            audio: AudioConfig::default(),
        }
    }
}

fn write_config(path: &Path, raw: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| AppError::io(parent.to_path_buf(), e))?;
    }
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)
        .map_err(|e| AppError::io(path.to_path_buf(), e))?;
    // Пароль у відкритому тексті — файл має бути 0600.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = f.set_permissions(fs::Permissions::from_mode(0o600));
    }
    f.write_all(raw.as_bytes())
        .map_err(|e| AppError::io(path.to_path_buf(), e))?;
    Ok(())
}

/// `~/.config/SELFsonic`
pub fn config_dir() -> Result<PathBuf> {
    dirs::config_dir()
        .map(|d| d.join(APP_NAME))
        .ok_or_else(|| AppError::Config("failed to determine config dir".into()))
}

/// `~/.local/state/SELFsonic`, fallback на config dir.
pub fn state_dir() -> Result<PathBuf> {
    let dir = dirs::state_dir()
        .or_else(dirs::cache_dir)
        .map(|d| d.join(APP_NAME))
        .ok_or_else(|| AppError::Config("failed to determine state dir".into()))?;
    fs::create_dir_all(&dir).map_err(|e| AppError::io(dir.clone(), e))?;
    Ok(dir)
}

/// `~/.cache/SELFsonic`, fallback на config dir.
pub fn cache_dir() -> Result<PathBuf> {
    let dir = dirs::cache_dir()
        .map(|d| d.join(APP_NAME))
        .ok_or_else(|| AppError::Config("failed to determine cache dir".into()))?;
    fs::create_dir_all(&dir).map_err(|e| AppError::io(dir.clone(), e))?;
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let cfg = Config {
            server: ServerConfig {
                url: "http://h:1".into(),
                username: "u".into(),
                password: "p".into(),
            },
            audio: AudioConfig::default(),
        };
        let raw = toml::to_string(&cfg).unwrap();
        let back: Config = toml::from_str(&raw).unwrap();
        assert_eq!(back.server.url, "http://h:1");
        assert_eq!(back.server.password, "p");
    }

    #[test]
    fn default_audio_config_is_applied_when_missing() {
        let raw = r#"
[server]
url = "http://h:1"
username = "u"
password = "p"
"#;
        let cfg: Config = toml::from_str(raw).unwrap();
        assert_eq!(cfg.audio.volume, 0.8);
        assert_eq!(cfg.audio.seek_step, 10);
    }
}
