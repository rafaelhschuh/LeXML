//! Diálogo de recuperação (no estilo do Office): para cada documento que
//! ficou com cópia de recuperação, Restaurar, Descartar ou Ver diferenças
//! entre a cópia e o arquivo em disco.

use crate::backup::Orphan;
use crate::diff::{self, Tag};
use crate::i18n::tr;
use gtk::prelude::*;
use gtk::{gdk, gio, glib};
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

    let head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let spinner = gtk::Spinner::new();
    spinner.start();
    head.append(&spinner);
    let summary = gtk::Label::builder().label(tr("diff_loading")).xalign(0.0).wrap(true).build();
    summary.add_css_class("heading");
    head.append(&summary);
    root.append(&head);
    let legend = gtk::Label::builder().label(tr("diff_legend")).xalign(0.0).wrap(true).build();
    legend.add_css_class("dim-label");
    root.append(&legend);

    let view = gtk::TextView::builder()
        .editable(false)
        .cursor_visible(false)
        .monospace(true)
        .wrap_mode(gtk::WrapMode::None)
        .left_margin(8)
        .top_margin(6)
        .build();
    let buffer = view.buffer();
    let tag = |name: &str, bg: Option<&str>, dim: bool| {
        let t = gtk::TextTag::new(Some(name));
        if let Some(c) = bg.and_then(|c| gdk::RGBA::parse(c).ok()) {
            t.set_paragraph_background_rgba(Some(&c));
        }
        if dim {
            t.set_style(gtk::pango::Style::Italic);
            t.set_foreground_rgba(gdk::RGBA::parse("rgba(128,128,128,0.9)").ok().as_ref());
        }
        buffer.tag_table().add(&t);
    };
    tag("add", Some("rgba(46,160,67,0.22)"), false);
    tag("del", Some("rgba(220,53,69,0.22)"), false);
    tag("gap", None, true);
    let scroller = gtk::ScrolledWindow::builder().vexpand(true).child(&view).build();
    scroller.add_css_class("frame");
    root.append(&scroller);

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
        spinner.stop();
        spinner.set_visible(false);
        let d = match res {
            Ok(d) => d,
            Err(e) => {
                summary.set_text(&format!("{}\n{e}", tr("diff_error")));
                return;
            }
        };
        let mut head_text = if d.added == 0 && d.removed == 0 && d.columns_before == d.columns_after {
            tr("diff_same").to_string()
        } else {
            format!("{} {}  ·  {} {}", d.added, tr("diff_added"), d.removed, tr("diff_removed"))
        };
        if d.columns_before != d.columns_after {
            head_text.push_str(&format!(
                "\n{}: {}  →  {}",
                tr("diff_columns"),
                d.columns_before.join(", "),
                d.columns_after.join(", ")
            ));
        }
        if d.truncated {
            head_text.push_str(&format!("\n{}", tr("diff_truncated")));
        }
        summary.set_text(&head_text);

        let mut end = buffer.end_iter();
        for (t, text) in &d.lines {
            let (line, tag) = match t {
                Tag::Added => (format!("+ {text}\n"), Some("add")),
                Tag::Removed => (format!("− {text}\n"), Some("del")),
                Tag::Same => (format!("  {text}\n"), None),
                Tag::Gap => (format!("  … {text} {} …\n", tr("diff_equal_lines")), Some("gap")),
            };
            match tag.and_then(|n| buffer.tag_table().lookup(n)) {
                Some(tg) => buffer.insert_with_tags(&mut end, &line, &[&tg]),
                None => buffer.insert(&mut end, &line),
            }
        }
    });
}
