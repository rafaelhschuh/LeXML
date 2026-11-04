//! Diálogo de recuperação (no estilo do Office): para cada documento que
//! ficou com cópia de recuperação, Restaurar, Descartar ou Ver diferenças
//! entre a cópia e o arquivo em disco.

use crate::backup::Orphan;
use crate::diff::{self, ColState, DiffRow, Tag};
use crate::i18n::tr;
use gtk::prelude::*;
use gtk::{gio, glib};
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;

/// Mostra o diálogo com `orphans`. `on_restore` recebe cada cópia que o
/// usuário mandar restaurar; `on_finish` roda quando o diálogo fecha (todas
/// decididas, "Decidir depois" ou fechar a janela).
pub fn show_dialog(
    parent: &gtk::Window,
    orphans: Vec<Orphan>,
    on_restore: Rc<dyn Fn(Orphan)>,
    on_finish: Box<dyn FnOnce()>,
) {
    let win = gtk::Window::builder()
        .title(tr("recover_title"))
        .transient_for(parent)
        .modal(true)
        .resizable(false)
        .default_width(600)
        .build();

    let root = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .margin_top(18)
        .margin_bottom(18)
        .margin_start(18)
        .margin_end(18)
        .build();
    let heading = gtk::Label::builder().label(tr("recover_title")).xalign(0.0).build();
    heading.add_css_class("title-2");
    root.append(&heading);
    let body = gtk::Label::builder().label(tr("recover_body")).wrap(true).xalign(0.0).build();
    body.add_css_class("dim-label");
    root.append(&body);

    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::None);
    list.add_css_class("frame");
    list.add_css_class("recent-list");
    root.append(&list);

    let remaining = Rc::new(Cell::new(orphans.len()));
    for o in orphans {
        let row = gtk::ListBoxRow::builder().activatable(false).build();
        let line = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(12)
            .margin_top(10)
            .margin_bottom(10)
            .margin_start(12)
            .margin_end(12)
            .build();
        let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
        text.set_hexpand(true);
        let name = gtk::Label::builder().label(&o.name).xalign(0.0).build();
        name.add_css_class("heading");
        text.append(&name);
        // Hora da cópia e, numa linha só, o caminho de origem cortado no meio.
        let (when, origin) = describe(&o);
        for t in when.iter().chain([&origin]) {
            let l = gtk::Label::builder()
                .label(t)
                .xalign(0.0)
                .ellipsize(gtk::pango::EllipsizeMode::Middle)
                .max_width_chars(40)
                .tooltip_text(t)
                .build();
            l.add_css_class("dim-label");
            text.append(&l);
        }
        line.append(&text);

        let has_original = o.original.as_ref().is_some_and(|p| p.is_file());
        let (data, original, label) = (o.data.clone(), o.original.clone(), o.name.clone());
        let slot = Rc::new(RefCell::new(Some(o)));

        // Tira a linha do diálogo; o último a sair fecha a janela.
        let done: Rc<dyn Fn()> = {
            let (list, row, remaining, win) = (list.clone(), row.clone(), remaining.clone(), win.clone());
            Rc::new(move || {
                list.remove(&row);
                remaining.set(remaining.get().saturating_sub(1));
                if remaining.get() == 0 {
                    win.close();
                }
            })
        };
        let restore: Rc<dyn Fn()> = {
            let (slot, done, on_restore) = (slot.clone(), done.clone(), on_restore.clone());
            Rc::new(move || {
                if let Some(o) = slot.borrow_mut().take() {
                    on_restore(o);
                }
                done();
            })
        };
        let discard: Rc<dyn Fn()> = {
            let (slot, done) = (slot.clone(), done.clone());
            Rc::new(move || {
                if let Some(o) = slot.borrow_mut().take() {
                    o.discard();
                }
                done();
            })
        };

        let diff_btn = gtk::Button::with_label(tr("diff_open"));
        diff_btn.set_sensitive(has_original);
        if !has_original {
            diff_btn.set_tooltip_text(Some(tr("diff_unavailable")));
        }
        {
            let (win, restore, discard) = (win.clone(), restore.clone(), discard.clone());
            diff_btn.connect_clicked(move |_| {
                if let Some(before) = original.clone() {
                    show_diff(win.upcast_ref(), before, data.clone(), &label, restore.clone(), discard.clone());
                }
            });
        }
        let discard_btn = gtk::Button::with_label(tr("recover_discard"));
        discard_btn.connect_clicked(move |_| discard());
        let restore_btn = gtk::Button::with_label(tr("recover_open"));
        restore_btn.add_css_class("suggested-action");
        restore_btn.connect_clicked(move |_| restore());
        for b in [&diff_btn, &discard_btn, &restore_btn] {
            b.set_valign(gtk::Align::Center);
            line.append(b);
        }
        row.set_child(Some(&line));
        list.append(&row);
    }

    let later = gtk::Button::with_label(tr("recover_later"));
    later.set_halign(gtk::Align::End);
    later.add_css_class("flat");
    {
        let win = win.clone();
        later.connect_clicked(move |_| win.close());
    }
    root.append(&later);

    let on_finish = RefCell::new(Some(on_finish));
    win.connect_close_request(move |_| {
        if let Some(f) = on_finish.borrow_mut().take() {
            f();
        }
        glib::Propagation::Proceed
    });

    win.set_child(Some(&root));
    win.present();
}

/// ("Cópia de 04/11/2026 14:32", "/caminho/arquivo.xml" ou "documento novo").
fn describe(o: &Orphan) -> (Option<String>, String) {
    let when = (o.saved > 0)
        .then(|| glib::DateTime::from_unix_local(o.saved as i64).ok())
        .flatten()
        .and_then(|d| d.format("%d/%m/%Y %H:%M").ok())
        .map(|s| format!("{} {s}", tr("recover_saved_at")));
    let origin = match &o.original {
        Some(p) => p.display().to_string(),
        None => tr("recover_new_doc").to_string(),
    };
    (when, origin)
}

/// Janela de diferenças entre o arquivo em disco e a cópia recuperada.
fn show_diff(
    parent: &gtk::Window,
    before: PathBuf,
    after: PathBuf,
    name: &str,
    restore: Rc<dyn Fn()>,
    discard: Rc<dyn Fn()>,
) {
    let win = gtk::Window::builder()
        .title(format!("{} — {name}", tr("diff_title")))
        .transient_for(parent)
        .modal(true)
        .default_width(960)
        .default_height(620)
        .build();
    let root = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(8)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .build();

    let pane = DiffPane::new(DiffTexts {
        legend: tr("diff_legend"),
        added: tr("diff_added"),
        removed: tr("diff_removed"),
        same: tr("diff_same"),
    });
    root.append(&pane.root);

    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    buttons.set_halign(gtk::Align::End);
    let close_btn = gtk::Button::with_label(tr("close"));
    let discard_btn = gtk::Button::with_label(tr("recover_discard"));
    let restore_btn = gtk::Button::with_label(tr("recover_open"));
    restore_btn.add_css_class("suggested-action");
    for b in [&close_btn, &discard_btn, &restore_btn] {
        buttons.append(b);
    }
    root.append(&buttons);
    {
        let win = win.clone();
        close_btn.connect_clicked(move |_| win.close());
    }
    {
        let win = win.clone();
        discard_btn.connect_clicked(move |_| {
            win.close();
            discard();
        });
    }
    {
        let win = win.clone();
        restore_btn.connect_clicked(move |_| {
            win.close();
            restore();
        });
    }

    win.set_child(Some(&root));
    win.present();

    // A comparação abre os dois arquivos: fora da thread da interface.
    let handle = gio::spawn_blocking(move || diff::diff_files(&before, &after).map_err(|e| e.to_string()));
    glib::spawn_future_local(async move {
        let res = handle.await.unwrap_or_else(|_| Err(String::new()));
        pane.show(res);
    });
}

/// Textos do resumo, que mudam conforme o que está sendo comparado.
pub struct DiffTexts {
    pub legend: &'static str,
    pub added: &'static str,
    pub removed: &'static str,
    pub same: &'static str,
}

/// Classes de cor das células da tabela de diferenças (ver CSS em `app`).
const DIFF_CLASSES: [&str; 7] =
    ["diff-add", "diff-del", "diff-chg", "diff-gap", "diff-cell-chg", "diff-col-add", "diff-col-del"];

/// Resumo + tabela com as linhas incluídas, apagadas e alteradas,
/// reaproveitado pela recuperação e pelo histórico de versões.
pub struct DiffPane {
    pub root: gtk::Box,
    spinner: gtk::Spinner,
    summary: gtk::Label,
    view: gtk::ColumnView,
    store: gio::ListStore,
    texts: DiffTexts,
}

impl DiffPane {
    pub fn new(texts: DiffTexts) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
        let head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let spinner = gtk::Spinner::new();
        head.append(&spinner);
        let summary = gtk::Label::builder().xalign(0.0).wrap(true).build();
        summary.add_css_class("heading");
        head.append(&summary);
        root.append(&head);
        let legend = gtk::Label::builder().label(texts.legend).xalign(0.0).wrap(true).build();
        legend.add_css_class("dim-label");
        root.append(&legend);

        let store = gio::ListStore::new::<glib::BoxedAnyObject>();
        let view = gtk::ColumnView::builder()
            .model(&gtk::NoSelection::new(Some(store.clone())))
            .show_row_separators(true)
            .show_column_separators(true)
            .reorderable(false)
            .build();
        view.add_css_class("data-table");
        view.add_css_class("diff-table");
        let scroller = gtk::ScrolledWindow::builder().vexpand(true).hexpand(true).child(&view).build();
        scroller.add_css_class("frame");
        root.append(&scroller);

        let pane = Self { root, spinner, summary, view, store, texts };
        pane.loading();
        pane
    }

    /// Limpa a tabela e mostra "Comparando…".
    pub fn loading(&self) {
        self.store.remove_all();
        self.spinner.set_visible(true);
        self.spinner.start();
        self.summary.set_text(tr("diff_loading"));
    }

    pub fn show(&self, res: Result<diff::Diff, String>) {
        self.spinner.stop();
        self.spinner.set_visible(false);
        self.store.remove_all();
        let d = match res {
            Ok(d) => d,
            Err(e) => {
                self.summary.set_text(&format!("{}\n{e}", tr("diff_error")));
                return;
            }
        };
        let cols_changed = d.columns.iter().any(|(_, st)| *st != ColState::Same);
        let mut head_text = if d.added == 0 && d.removed == 0 && d.changed == 0 && !cols_changed {
            self.texts.same.to_string()
        } else {
            format!(
                "{} {}  ·  {} {}  ·  {} {}",
                d.added,
                self.texts.added,
                d.removed,
                self.texts.removed,
                d.changed,
                tr("diff_changed")
            )
        };
        for (state, key) in [(ColState::Removed, "diff_cols_removed"), (ColState::Added, "diff_cols_added")] {
            let names: Vec<&str> =
                d.columns.iter().filter(|(_, st)| *st == state).map(|(n, _)| n.as_str()).collect();
            if !names.is_empty() {
                head_text.push_str(&format!("\n{}: {}", tr(key), names.join(", ")));
            }
        }
        if d.truncated {
            head_text.push_str(&format!("\n{}", tr("diff_truncated")));
        }
        self.summary.set_text(&head_text);

        // As colunas mudam a cada comparação: refaz todas.
        let old: Vec<gtk::ColumnViewColumn> =
            self.view.columns().iter::<gtk::ColumnViewColumn>().flatten().collect();
        for c in old {
            self.view.remove_column(&c);
        }
        self.view.append_column(&diff_column("", None, ColState::Same));
        for (k, (name, state)) in d.columns.iter().enumerate() {
            let title = match state {
                ColState::Same => name.clone(),
                ColState::Added => format!("{name} {}", tr("diff_col_added_tag")),
                ColState::Removed => format!("{name} {}", tr("diff_col_removed_tag")),
            };
            self.view.append_column(&diff_column(&title, Some(k), *state));
        }
        let items: Vec<glib::BoxedAnyObject> = d.rows.into_iter().map(glib::BoxedAnyObject::new).collect();
        self.store.extend_from_slice(&items);
    }
}

/// Coluna da tabela de diferenças: `k` é a posição em `DiffRow::values`;
/// `None` é a coluna estreita com o sinal (+, −, ~).
fn diff_column(title: &str, k: Option<usize>, state: ColState) -> gtk::ColumnViewColumn {
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(|_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else { return };
        let label = gtk::Label::builder()
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        item.set_child(Some(&label));
    });
    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else { return };
        let (Some(obj), Some(label)) =
            (item.item().and_downcast::<glib::BoxedAnyObject>(), item.child().and_downcast::<gtk::Label>())
        else {
            return;
        };
        let row = obj.borrow::<DiffRow>();
        let mut classes: Vec<&str> = Vec::new();
        classes.push(match row.tag {
            Tag::Added => "diff-add",
            Tag::Removed => "diff-del",
            Tag::Changed => "diff-chg",
            Tag::Gap => "diff-gap",
            Tag::Same => "",
        });
        let text = match k {
            None => match row.tag {
                Tag::Added => "+".to_string(),
                Tag::Removed => "−".to_string(),
                Tag::Changed => "~".to_string(),
                Tag::Gap | Tag::Same => String::new(),
            },
            Some(k) if row.tag == Tag::Gap => {
                if k == 0 {
                    format!("… {} {} …", row.values[0], tr("diff_equal_lines"))
                } else {
                    String::new()
                }
            }
            Some(k) => {
                let new = row.values.get(k).map_or("", String::as_str);
                let old = row.old.get(k).map_or(new, String::as_str);
                if row.tag != Tag::Gap {
                    match state {
                        ColState::Added => classes.push("diff-col-add"),
                        ColState::Removed => classes.push("diff-col-del"),
                        ColState::Same => {}
                    }
                }
                if row.tag == Tag::Changed && old != new {
                    classes.push("diff-cell-chg");
                    format!("{old} → {new}")
                } else {
                    new.to_string()
                }
            }
        };
        label.set_text(&text);
        label.set_tooltip_text((!text.is_empty()).then_some(text.as_str()));
        // A cor vai na célula inteira (o pai do rótulo), não só no texto.
        let target: gtk::Widget = label.parent().unwrap_or_else(|| label.clone().upcast());
        for c in DIFF_CLASSES {
            target.remove_css_class(c);
        }
        for c in classes.into_iter().filter(|c| !c.is_empty()) {
            target.add_css_class(c);
        }
    });
    let col = gtk::ColumnViewColumn::new(Some(title), Some(factory));
    match k {
        None => col.set_fixed_width(32),
        Some(_) => {
            col.set_expand(true);
            col.set_resizable(true);
        }
    }
    col
}
