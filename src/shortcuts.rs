//! Atalhos de teclado configuráveis. Cada ação tem atalhos padrão; o que o
//! usuário muda fica em `~/.config/lexml/atalhos.ini` (só as diferenças do
//! padrão). Os atalhos usam o formato de acelerador do GTK (`<Control>z`).

use gtk::gdk;
use std::cell::RefCell;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

pub struct Action {
    pub id: &'static str,
    /// Chave de tradução do nome mostrado nas configurações.
    pub label: &'static str,
    pub defaults: &'static [&'static str],
    /// Age sobre a tabela: num campo de texto o atalho fica com o campo.
    pub grid: bool,
}

pub const ACTIONS: &[Action] = &[
    Action {
        id: "save",
        label: "sc_save",
        defaults: &["<Control>s"],
        grid: false,
    },
    Action {
        id: "save_as",
        label: "sc_save_as",
        defaults: &["<Control><Shift>s"],
        grid: false,
    },
    Action {
        id: "undo",
        label: "sc_undo",
        defaults: &["<Control>z"],
        grid: true,
    },
    Action {
        id: "redo",
        label: "sc_redo",
        defaults: &["<Control><Shift>z", "<Control>y"],
        grid: true,
    },
    Action {
        id: "find",
        label: "sc_find",
        defaults: &["<Control>f"],
        grid: false,
    },
    Action {
        id: "replace",
        label: "sc_replace",
        defaults: &["<Control>h"],
        grid: false,
    },
    Action {
        id: "history",
        label: "sc_history",
        defaults: &["<Control><Shift>h"],
        grid: false,
    },
    Action {
        id: "sql",
        label: "sc_sql",
        defaults: &[],
        grid: false,
    },
    Action {
        id: "select_all",
        label: "sc_select_all",
        defaults: &["<Control>a"],
        grid: true,
    },
    Action {
        id: "copy",
        label: "sc_copy",
        defaults: &["<Control>c"],
        grid: true,
    },
    Action {
        id: "cut",
        label: "sc_cut",
        defaults: &["<Control>x"],
        grid: true,
    },
    Action {
        id: "paste",
        label: "sc_paste",
        defaults: &["<Control>v"],
        grid: true,
    },
    Action {
        id: "clear_cells",
        label: "sc_clear_cells",
        defaults: &["BackSpace"],
        grid: true,
    },
    Action {
        id: "delete_rows",
        label: "sc_delete_rows",
        defaults: &["Delete"],
        grid: true,
    },
    Action {
        id: "row_below",
        label: "sc_row_below",
        defaults: &["<Control>Return"],
        grid: true,
    },
    Action {
        id: "row_above",
        label: "sc_row_above",
        defaults: &["<Control><Shift>Return"],
        grid: true,
    },
    Action {
        id: "row_clone",
        label: "sc_row_clone",
        defaults: &["<Control>d"],
        grid: true,
    },
    Action {
        id: "move_up",
        label: "sc_move_up",
        defaults: &["<Alt>Up"],
        grid: true,
    },
    Action {
        id: "move_down",
        label: "sc_move_down",
        defaults: &["<Alt>Down"],
        grid: true,
    },
];

thread_local! {
    static CURRENT: RefCell<HashMap<&'static str, Vec<String>>> = RefCell::new(load());
}

fn file_path() -> PathBuf {
    let mut dir = gtk::glib::user_config_dir();
    dir.push("lexml");
    dir.push("atalhos.ini");
    dir
}

fn defaults(a: &Action) -> Vec<String> {
    a.defaults.iter().map(|s| s.to_string()).collect()
}

fn load() -> HashMap<&'static str, Vec<String>> {
    let mut map: HashMap<&'static str, Vec<String>> =
        ACTIONS.iter().map(|a| (a.id, defaults(a))).collect();
    if let Ok(text) = fs::read_to_string(file_path()) {
        apply_overrides(&mut map, &text, |s| parse(s).is_some());
    }
    map
}

/// `id=acel1;acel2` por linha; `id=` deixa a ação sem atalho. Ações e
/// atalhos desconhecidos são ignorados.
fn apply_overrides(
    map: &mut HashMap<&'static str, Vec<String>>,
    text: &str,
    valid: impl Fn(&str) -> bool,
) {
    for line in text.lines() {
        let Some((id, v)) = line.trim().split_once('=') else {
            continue;
        };
        let Some(a) = ACTIONS.iter().find(|a| a.id == id.trim()) else {
            continue;
        };
        let accels = v
            .split(';')
            .map(str::trim)
            .filter(|s| valid(s))
            .map(String::from)
            .collect();
        map.insert(a.id, accels);
    }
}

fn save(map: &HashMap<&'static str, Vec<String>>) {
    let body: String = ACTIONS
        .iter()
        .filter(|a| map.get(a.id).is_some_and(|v| *v != defaults(a)))
        .map(|a| format!("{}={}\n", a.id, map[a.id].join(";")))
        .collect();
    let path = file_path();
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let _ = fs::write(path, body);
}

/// Atalhos atuais de `id`.
pub fn accels(id: &str) -> Vec<String> {
    CURRENT.with(|c| c.borrow().get(id).cloned().unwrap_or_default())
}

/// Troca os atalhos de `id` e grava.
pub fn set(id: &str, accels: Vec<String>) {
    CURRENT.with(|c| {
        let mut map = c.borrow_mut();
        if let Some(a) = ACTIONS.iter().find(|a| a.id == id) {
            map.insert(a.id, accels);
        }
        save(&map);
    });
}

/// Dá `accel` a `id` (no lugar dos atalhos que ele tinha) e o tira de
/// outra ação que o usasse. Devolve essa outra ação.
pub fn assign(id: &str, accel: &str) -> Option<&'static Action> {
    let taken = owner(accel, id);
    if let (Some(o), Some(target)) = (taken, parse(accel)) {
        let rest = accels(o.id)
            .into_iter()
            .filter(|s| parse(s) != Some(target))
            .collect();
        set(o.id, rest);
    }
    set(id, vec![accel.to_string()]);
    taken
}

/// Volta os atalhos de `id` ao padrão.
pub fn reset(id: &str) {
    if let Some(a) = ACTIONS.iter().find(|a| a.id == id) {
        set(id, defaults(a));
    }
}

/// Volta todos os atalhos ao padrão.
pub fn reset_all() {
    CURRENT.with(|c| {
        let mut map = c.borrow_mut();
        *map = ACTIONS.iter().map(|a| (a.id, defaults(a))).collect();
        save(&map);
    });
}

const MODS: gdk::ModifierType = gdk::ModifierType::CONTROL_MASK
    .union(gdk::ModifierType::SHIFT_MASK)
    .union(gdk::ModifierType::ALT_MASK)
    .union(gdk::ModifierType::SUPER_MASK);

/// Tecla e modificadores comparáveis: minúscula e teclado numérico igual
/// ao principal.
fn normalize(key: gdk::Key, mods: gdk::ModifierType) -> (gdk::Key, gdk::ModifierType) {
    let key = match key {
        gdk::Key::KP_Delete => gdk::Key::Delete,
        gdk::Key::KP_Enter => gdk::Key::Return,
        gdk::Key::ISO_Left_Tab => gdk::Key::Tab,
        k => k.to_lower(),
    };
    (key, mods & MODS)
}

fn parse(accel: &str) -> Option<(gdk::Key, gdk::ModifierType)> {
    gtk::accelerator_parse(accel).map(|(k, m)| normalize(k, m))
}

/// A ação cujo atalho é esta tecla, se houver.
pub fn find(key: gdk::Key, state: gdk::ModifierType) -> Option<&'static Action> {
    let pressed = normalize(key, state);
    CURRENT.with(|c| {
        let map = c.borrow();
        ACTIONS.iter().find(|a| {
            map.get(a.id)
                .is_some_and(|v| v.iter().any(|s| parse(s) == Some(pressed)))
        })
    })
}

/// Outra ação que já usa `accel`.
pub fn owner(accel: &str, except: &str) -> Option<&'static Action> {
    let target = parse(accel)?;
    CURRENT.with(|c| {
        let map = c.borrow();
        ACTIONS.iter().filter(|a| a.id != except).find(|a| {
            map.get(a.id)
                .is_some_and(|v| v.iter().any(|s| parse(s) == Some(target)))
        })
    })
}

/// Acelerador de uma tecla pressionada, ou `None` se for só um modificador.
pub fn from_event(key: gdk::Key, state: gdk::ModifierType) -> Option<String> {
    let name = key.name().map(|n| n.to_string()).unwrap_or_default();
    let modifier = [
        "Shift",
        "Control",
        "Alt",
        "Super",
        "Meta",
        "Hyper",
        "ISO_Level3",
        "Caps_Lock",
        "Num_Lock",
    ];
    if modifier.iter().any(|m| name.starts_with(m)) {
        return None;
    }
    let (key, mods) = normalize(key, state);
    Some(gtk::accelerator_name(key, mods).to_string())
}

/// Texto legível dos atalhos de `id` ("Ctrl+Shift+Z / Ctrl+Y").
pub fn label(id: &str) -> String {
    accels(id)
        .iter()
        .filter_map(|s| gtk::accelerator_parse(s))
        .map(|(k, m)| gtk::accelerator_get_label(k, m).to_string())
        .collect::<Vec<_>>()
        .join(" / ")
}

/// Atalho exibido ao lado do item no menu de contexto (o primeiro).
pub fn menu_accel(id: &str) -> Option<String> {
    accels(id).into_iter().next()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atalhos_padrao_nao_se_repetem() {
        let mut seen = std::collections::HashSet::new();
        for a in ACTIONS {
            for s in a.defaults {
                assert!(seen.insert(s.to_lowercase()), "{s} usado duas vezes");
            }
        }
    }

    #[test]
    fn alteracoes_do_usuario_sobrepoem_o_padrao() {
        let mut map: HashMap<&'static str, Vec<String>> =
            ACTIONS.iter().map(|a| (a.id, defaults(a))).collect();
        let text = "undo=<Control>u\nsql=\nnao_existe=<Control>q\nredo=<Control>y;lixo<<\n";
        apply_overrides(&mut map, text, |s| !s.is_empty() && !s.contains("<<"));
        assert_eq!(map["undo"], ["<Control>u"]);
        assert!(map["sql"].is_empty());
        assert_eq!(map["redo"], ["<Control>y"]);
        assert_eq!(map["save"], ["<Control>s"]);
    }

    #[test]
    fn maiuscula_e_teclado_numerico_casam_com_o_atalho() {
        let ctrl = gdk::ModifierType::CONTROL_MASK;
        assert_eq!(normalize(gdk::Key::Z, ctrl), (gdk::Key::z, ctrl));
        assert_eq!(
            normalize(gdk::Key::KP_Delete, gdk::ModifierType::empty()).0,
            gdk::Key::Delete
        );
        // trava numérica e afins não contam
        assert_eq!(
            normalize(gdk::Key::a, ctrl | gdk::ModifierType::LOCK_MASK).1,
            ctrl
        );
    }
}
