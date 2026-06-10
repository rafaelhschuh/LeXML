use std::fs;
use std::path::PathBuf;

/// Preferências persistidas do app (tema e idioma).
#[derive(Clone)]
pub struct Config {
    pub theme: String, // "system" | "light" | "dark"
    pub lang: String,  // "pt" | "en"
}

impl Default for Config {
    fn default() -> Self {
        Config {
            theme: "system".into(),
            lang: "pt".into(),
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
    let body = format!("theme={}\nlang={}\n", cfg.theme, cfg.lang);
    let _ = fs::write(path, body);
}
