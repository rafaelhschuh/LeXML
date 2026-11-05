use std::fs;
use std::path::{Path, PathBuf};

/// Quantos arquivos a lista de recentes guarda.
const MAX_RECENT: usize = 12;

/// Lista de arquivos recentes: um caminho absoluto por linha, do mais recente
/// para o mais antigo, ao lado do `config.ini`.
fn recent_path() -> PathBuf {
    let mut dir = gtk::glib::user_config_dir();
    dir.push("lexml");
    dir.push("recentes.txt");
    dir
}

fn read_list(file: &Path) -> Vec<PathBuf> {
    let Ok(text) = fs::read_to_string(file) else {
        return Vec::new();
    };
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(PathBuf::from)
        .collect()
}

fn write_list(file: &Path, list: &[PathBuf]) {
    if let Some(parent) = file.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let body: String = list
        .iter()
        .map(|p| format!("{}\n", p.to_string_lossy()))
        .collect();
    let _ = crate::xmldoc::write_atomic(file, body.as_bytes());
}

/// Coloca `path` no topo de `list`, sem duplicar e respeitando o limite.
fn push_front(list: &mut Vec<PathBuf>, path: PathBuf) {
    list.retain(|p| *p != path);
    list.insert(0, path);
    list.truncate(MAX_RECENT);
}

/// Arquivos recentes que ainda existem, do mais recente para o mais antigo.
pub fn list() -> Vec<PathBuf> {
    read_list(&recent_path())
        .into_iter()
        .filter(|p| p.is_file())
        .collect()
}

/// Registra `path` como o arquivo usado mais recentemente.
pub fn add(path: &Path) {
    // Só caminhos absolutos: um relativo apontaria para outro lugar quando o
    // app fosse aberto a partir de outra pasta.
    let Ok(abs) = std::path::absolute(path) else {
        return;
    };
    let file = recent_path();
    let mut list = read_list(&file);
    push_front(&mut list, abs);
    write_list(&file, &list);
}

/// Esvazia a lista.
pub fn clear() {
    write_list(&recent_path(), &[]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_front_move_para_o_topo_sem_duplicar_e_corta_no_limite() {
        let mut l: Vec<PathBuf> = (0..MAX_RECENT)
            .map(|i| PathBuf::from(format!("/a/{i}.xml")))
            .collect();
        push_front(&mut l, PathBuf::from("/a/5.xml"));
        assert_eq!(l[0], PathBuf::from("/a/5.xml"));
        assert_eq!(l.len(), MAX_RECENT);
        assert_eq!(
            l.iter()
                .filter(|p| p.as_path() == Path::new("/a/5.xml"))
                .count(),
            1
        );

        push_front(&mut l, PathBuf::from("/b/novo.xml"));
        assert_eq!(l[0], PathBuf::from("/b/novo.xml"));
        assert_eq!(l.len(), MAX_RECENT, "o mais antigo sai da lista");
    }
}
