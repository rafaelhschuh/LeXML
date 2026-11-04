use std::fs;
use std::path::PathBuf;

/// Preferências persistidas do app.
#[derive(Clone)]
pub struct Config {
    pub theme: String, // "system" | "light" | "dark"
    pub lang: String,  // "pt" | "en"
    /// Dias que os backups ficam guardados.
    pub backup_keep_days: u32,
    /// Tamanho máximo da pasta de backups, em MB.
    pub backup_max_mb: u32,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            theme: "system".into(),
            lang: "pt".into(),
            backup_keep_days: 7,
            backup_max_mb: 500,
        }
    }
}

fn config_path() -> PathBuf {
    let mut dir = gtk::glib::user_config_dir();
    dir.push("lexml");
    dir.push("config.ini");
    dir
}

pub fn load() -> Config {
    let mut cfg = Config::default();
    let Ok(text) = fs::read_to_string(config_path()) else {
        return cfg;
    };
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            let v = v.trim().to_string();
            match k.trim() {
                "theme" => cfg.theme = v,
                "lang" => cfg.lang = v,
                "backup_keep_days" => cfg.backup_keep_days = v.parse().unwrap_or(7).clamp(1, 365),
                "backup_max_mb" => cfg.backup_max_mb = v.parse().unwrap_or(500).clamp(10, 100_000),
                _ => {}
            }
        }
    }
    cfg
}

pub fn save(cfg: &Config) {
    let path = config_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let body = format!(
        "theme={}\nlang={}\nbackup_keep_days={}\nbackup_max_mb={}\n",
        cfg.theme, cfg.lang, cfg.backup_keep_days, cfg.backup_max_mb
    );
    let _ = fs::write(path, body);
}
