use std::fs;
use std::io::Write;
use std::path::PathBuf;

use anyhow::{Context, Result};
use directories::ProjectDirs;
use serde::{Deserialize, Deserializer, Serialize};

use crate::app::queue::SavedQueue;

#[cfg(feature = "cast")]
use crate::cast::Device;

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Config {
    pub accent_color: Option<String>,
    #[cfg(feature = "cast")]
    pub static_cast_devices: Option<Vec<Device>>,
    client_identifier: Option<String>,
    token: Option<String>,
    server_url: Option<String>,
    server_id: Option<String>,
    server_token: Option<String>,
    music_section: Option<String>,
    volume: Option<f32>,
}

#[derive(Serialize, Deserialize)]
#[serde(default)]
pub struct SavedState {
    pub client_identifier: String,
    pub token: Option<String>,
    pub server_url: Option<String>,
    /// `machineIdentifier` of the chosen server, so rediscovery picks the same one
    pub server_id: Option<String>,
    pub server_token: Option<String>,
    pub music_section: Option<String>,
    pub volume: f32,
    #[serde(deserialize_with = "queue_or_none")]
    pub queue: Option<SavedQueue>,
}

/// a queue saved by another rexamp version may not parse..
/// dropping it is "better" than refusing to start
fn queue_or_none<'de, D: Deserializer<'de>>(d: D) -> Result<Option<SavedQueue>, D::Error> {
    let v = serde_json::Value::deserialize(d)?;
    Ok(serde_json::from_value(v)
        .inspect_err(|e| tracing::warn!("dropping the saved queue: {e}"))
        .ok()
        .flatten())
}

impl Default for SavedState {
    fn default() -> Self {
        Self {
            client_identifier: uuid::Uuid::new_v4().to_string(),
            token: None,
            server_url: None,
            server_id: None,
            server_token: None,
            music_section: None,
            volume: 0.8,
            queue: None,
        }
    }
}

pub fn dirs() -> Result<ProjectDirs> {
    ProjectDirs::from("", "", "rexamp").context("no home directory")
}

fn config_path() -> Result<PathBuf> {
    Ok(dirs()?.config_dir().join("config.toml"))
}

pub fn state_dir() -> Result<PathBuf> {
    let d = dirs()?;
    Ok(d.state_dir().unwrap_or(d.data_local_dir()).to_path_buf())
}

fn state_path() -> Result<PathBuf> {
    Ok(state_dir()?.join("state.json"))
}

impl Config {
    /// loads the config, or the defaults when there is none
    pub fn load() -> Result<Self> {
        let path = config_path()?;
        match fs::read_to_string(&path) {
            Ok(s) => toml::from_str(&s).with_context(|| format!("parsing {}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
        }
    }
}

impl SavedState {
    /// loads the saved state
    /// on first run, seeds it from the keys older versions kept in the config
    pub fn load(cfg: &Config) -> Result<Self> {
        if let Some(saved) = Self::read()? {
            return Ok(saved);
        }
        let d = Self::default();
        let saved = Self {
            client_identifier: cfg.client_identifier.clone().unwrap_or(d.client_identifier),
            token: cfg.token.clone(),
            server_url: cfg.server_url.clone(),
            server_id: cfg.server_id.clone(),
            server_token: cfg.server_token.clone(),
            music_section: cfg.music_section.clone(),
            volume: cfg.volume.unwrap_or(d.volume),
            queue: None,
        };
        saved.save()?;
        Ok(saved)
    }

    pub fn read() -> Result<Option<Self>> {
        let path = state_path()?;
        match fs::read_to_string(&path) {
            Ok(s) => serde_json::from_str(&s)
                .map(Some)
                .with_context(|| format!("parsing {}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
        }
    }

    pub fn save(&self) -> Result<()> {
        let path = state_path()?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("json.tmp");
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);

        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
        let mut f = opts
            .open(&tmp)
            .with_context(|| format!("writing {}", tmp.display()))?;
        f.write_all(&serde_json::to_vec(self)?)?;
        f.sync_all()?;
        fs::rename(&tmp, &path)?;
        Ok(())
    }
}
