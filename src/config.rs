use std::fs;
use std::io::Write;
use std::path::PathBuf;

use anyhow::{Context, Result};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub client_identifier: String,
    pub token: Option<String>,
    pub server_url: Option<String>,
    /// `machineIdentifier` of the chosen server, so rediscovery picks the same one
    pub server_id: Option<String>,
    pub server_token: Option<String>,
    pub music_section: Option<String>,
    pub volume: f32,
    /// anything ratatui's `Color` parses: a name, a 256-colour index or `#rrggbb`
    pub accent_color: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            client_identifier: uuid::Uuid::new_v4().to_string(),
            token: None,
            server_url: None,
            server_id: None,
            server_token: None,
            music_section: None,
            volume: 0.8,
            accent_color: None,
        }
    }
}

pub fn dirs() -> Result<ProjectDirs> {
    ProjectDirs::from("", "", "rex").context("no home directory")
}

fn config_path() -> Result<PathBuf> {
    Ok(dirs()?.config_dir().join("config.toml"))
}

pub fn state_dir() -> Result<PathBuf> {
    let d = dirs()?;
    Ok(d.state_dir().unwrap_or(d.data_local_dir()).to_path_buf())
}

impl Config {
    /// attemps to load the app config, creates new one if it does not exist
    pub fn load() -> Result<Self> {
        let path = config_path()?;
        match fs::read_to_string(&path) {
            Ok(s) => toml::from_str(&s).with_context(|| format!("parsing {}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let cfg = Self::default();
                cfg.save()?;
                Ok(cfg)
            }
            Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
        }
    }

    pub fn save(&self) -> Result<()> {
        let path = config_path()?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("toml.tmp");
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
        let mut f = opts
            .open(&tmp)
            .with_context(|| format!("writing {}", tmp.display()))?;
        f.write_all(toml::to_string(self)?.as_bytes())?;
        f.sync_all()?;
        fs::rename(&tmp, &path)?;
        Ok(())
    }
}
