//! Backup fora da pasta do arquivo, em `~/.cache/lexml/backup`:
//!
//! - `recuperacao/`: cópia periódica de cada documento com alterações não
//!   salvas. Some quando o documento é salvo ou fechado normalmente; se o app
//!   cair (queda de energia, travamento), sobra e é oferecida na próxima
//!   abertura.
//! - `versoes/`: cópia do arquivo em disco feita antes de cada salvamento
//!   sobrescrevê-lo.
//!
//! A limpeza apaga o que passou do prazo e, se a pasta ainda passar do
//! tamanho máximo, os arquivos mais antigos primeiro.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub fn root() -> PathBuf {
    let mut dir = gtk::glib::user_cache_dir();
    dir.push("lexml");
    dir.push("backup");
    dir
}

fn recovery_dir() -> PathBuf {
    root().join("recuperacao")
}

fn versions_dir() -> PathBuf {
    root().join("versoes")
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// Carimbo de data/hora local para nomes de arquivo: `20241103-142530`.
fn stamp() -> String {
    gtk::glib::DateTime::now_local()
        .and_then(|d| d.format("%Y%m%d-%H%M%S"))
        .map(|s| s.to_string())
        .unwrap_or_else(|_| now_secs().to_string())
}

/// Cópia de recuperação de um documento aberto nesta sessão.
pub struct Recovery {
    data: PathBuf,
    meta: PathBuf,
}

impl Recovery {
    /// Reserva os nomes da cópia; nada é gravado até `write`.
    pub fn new() -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let id = format!(
            "{}-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed),
            now_secs()
        );
        let dir = recovery_dir();
        Self { data: dir.join(format!("{id}.xml")), meta: dir.join(format!("{id}.info")) }
    }

    /// Grava a cópia (`bytes`) e o registro com o arquivo de origem. Bloqueia:
    /// chame fora da thread da interface.
    pub fn write(data: &Path, meta: &Path, bytes: &[u8], original: Option<&Path>, name: &str) -> anyhow::Result<()> {
        if let Some(dir) = data.parent() {
            fs::create_dir_all(dir)?;
        }
        crate::xmldoc::write_atomic(data, bytes)?;
        let info = format!(
            "pid={}\nsaved={}\nname={}\noriginal={}\n",
            std::process::id(),
            now_secs(),
            name.replace('\n', " "),
            original.map(|p| p.to_string_lossy().into_owned()).unwrap_or_default()
        );
        crate::xmldoc::write_atomic(meta, info.as_bytes())
    }

    pub fn paths(&self) -> (PathBuf, PathBuf) {
        (self.data.clone(), self.meta.clone())
    }

    /// Apaga a cópia: o documento foi salvo ou fechado sem pendências.
    pub fn remove(&self) {
        let _ = fs::remove_file(&self.meta);
        let _ = fs::remove_file(&self.data);
    }
}

/// Cópia de recuperação deixada por uma sessão que não fechou normalmente.
pub struct Orphan {
    pub data: PathBuf,
    meta: PathBuf,
    pub original: Option<PathBuf>,
    pub name: String,
    pub saved: u64,
}

impl Orphan {
    pub fn discard(&self) {
        let _ = fs::remove_file(&self.meta);
        let _ = fs::remove_file(&self.data);
    }
}

/// `true` se `pid` é um Lê-XML ainda em execução (a cópia dele está em uso).
fn alive(pid: u32) -> bool {
    if pid == std::process::id() {
        return true;
    }
    fs::read(format!("/proc/{pid}/cmdline"))
        .map(|c| String::from_utf8_lossy(&c).to_lowercase().contains("lexml"))
        .unwrap_or(false)
}

fn parse_meta(text: &str) -> Option<(u32, u64, String, Option<PathBuf>)> {
    let (mut pid, mut saved, mut name, mut original) = (None, 0, String::new(), None);
    for line in text.lines() {
        match line.split_once('=') {
            Some(("pid", v)) => pid = v.parse().ok(),
            Some(("saved", v)) => saved = v.parse().unwrap_or(0),
            Some(("name", v)) => name = v.to_string(),
            Some(("original", v)) if !v.is_empty() => original = Some(PathBuf::from(v)),
            _ => {}
        }
    }
    Some((pid?, saved, name, original))
}

/// Cópias de recuperação de sessões que não estão mais rodando, da mais
/// recente para a mais antiga.
pub fn orphans() -> Vec<Orphan> {
    let Ok(entries) = fs::read_dir(recovery_dir()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for e in entries.flatten() {
        let meta = e.path();
        if meta.extension().is_none_or(|x| x != "info") {
            continue;
        }
        let data = meta.with_extension("xml");
        let Some((pid, saved, name, original)) =
            fs::read_to_string(&meta).ok().as_deref().and_then(parse_meta)
        else {
            continue;
        };
        if alive(pid) || !data.is_file() {
            continue;
        }
        out.push(Orphan { data, meta, original, name, saved });
    }
    out.sort_by(|a, b| b.saved.cmp(&a.saved));
    out
}

/// Guarda uma cópia de `path` em `versoes/` antes de o salvamento
/// sobrescrevê-lo. Falhas são ignoradas: o backup nunca impede o salvamento.
pub fn save_version(path: &Path) {
    if !path.is_file() {
        return;
    }
    let dir = versions_dir();
    if fs::create_dir_all(&dir).is_err() {
        return;
    }
    let stem = path.file_stem().map_or("arquivo".into(), |s| s.to_string_lossy().into_owned());
    let ext = path.extension().map_or("xml".into(), |s| s.to_string_lossy().into_owned());
    let _ = fs::copy(path, dir.join(format!("{stem}_{}.{ext}", stamp())));
}

/// Arquivos de backup que a limpeza pode apagar: versões e cópias de
/// recuperação órfãs — nunca as de uma sessão em execução.
fn removable() -> Vec<(PathBuf, SystemTime, u64)> {
    let mut files = Vec::new();
    let mut push = |p: PathBuf| {
        if let Ok(m) = fs::metadata(&p) {
            files.push((p, m.modified().unwrap_or(UNIX_EPOCH), m.len()));
        }
    };
    if let Ok(entries) = fs::read_dir(versions_dir()) {
        for e in entries.flatten() {
            push(e.path());
        }
    }
    for o in orphans() {
        push(o.data);
        push(o.meta);
    }
    files
}

/// Apaga backups com mais de `keep_days` dias e, se ainda passar de `max_mb`,
/// os mais antigos até caber. Devolve quantos arquivos apagou.
pub fn cleanup(keep_days: u32, max_mb: u32) -> usize {
    plan_cleanup(removable(), SystemTime::now(), keep_days, max_mb)
        .into_iter()
        .filter(|p| fs::remove_file(p).is_ok())
        .count()
}

/// Decide o que apagar (separado de `cleanup` para ser testável).
fn plan_cleanup(
    mut files: Vec<(PathBuf, SystemTime, u64)>,
    now: SystemTime,
    keep_days: u32,
    max_mb: u32,
) -> Vec<PathBuf> {
    let max_age = Duration::from_secs(u64::from(keep_days) * 86_400);
    let mut doomed = Vec::new();
    files.retain(|(p, t, _)| {
        let old = now.duration_since(*t).is_ok_and(|age| age > max_age);
        if old {
            doomed.push(p.clone());
        }
        !old
    });
    // por tamanho: os mais antigos saem primeiro
    files.sort_by_key(|(_, t, _)| *t);
    let limit = u64::from(max_mb) * 1024 * 1024;
    let mut total: u64 = files.iter().map(|(_, _, s)| s).sum();
    for (p, _, size) in files {
        if total <= limit {
            break;
        }
        total -= size;
        doomed.push(p);
    }
    doomed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limpeza_apaga_os_vencidos_e_depois_os_mais_antigos_ate_caber() {
        let now = SystemTime::now();
        let day = Duration::from_secs(86_400);
        let mb = 1024 * 1024;
        let files = vec![
            (PathBuf::from("velho"), now - day * 10, mb),
            (PathBuf::from("a"), now - day * 3, 300 * mb),
            (PathBuf::from("b"), now - day * 2, 300 * mb),
            (PathBuf::from("c"), now - day, 100 * mb),
        ];
        let doomed = plan_cleanup(files, now, 7, 500);
        assert_eq!(doomed, [PathBuf::from("velho"), PathBuf::from("a")]);
    }

    #[test]
    fn registro_de_recuperacao_e_lido_de_volta() {
        let (pid, saved, name, original) =
            parse_meta("pid=42\nsaved=1700000000\nname=dados.xml\noriginal=/home/x/dados.xml\n").unwrap();
        assert_eq!((pid, saved, name.as_str()), (42, 1_700_000_000, "dados.xml"));
        assert_eq!(original, Some(PathBuf::from("/home/x/dados.xml")));
        let (_, _, _, original) = parse_meta("pid=1\noriginal=\n").unwrap();
        assert!(original.is_none(), "documento novo, nunca salvo");
        assert!(parse_meta("lixo").is_none());
    }
}
