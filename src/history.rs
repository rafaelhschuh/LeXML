//! Histórico de versões de um arquivo: lista as cópias guardadas no backup,
//! mostra o que mudou de cada uma até o documento aberto e restaura a
//! escolhida.

use crate::backup::{Kind, Version};
use crate::diff;
use crate::i18n::tr;
use crate::recovery::{DiffPane, DiffTexts};
use gtk::prelude::*;
use gtk::{gio, glib};
use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

pub fn show(
    parent: Option<&gtk::Window>,
    name: &str,
    versions: Vec<Version>,
    current: diff::Table,
    on_restore: Rc<dyn Fn(Version)>,
) {
    let win = gtk::Window::builder()
        .title(format!("{} — {name}", tr("history_title")))
        .modal(true)
        .default_width(1100)
        .default_height(680)
        .build();
    win.set_transient_for(parent);

    let root = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(10)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .build();

    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::Single);
    for v in &versions {
        let row = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(2)
            .margin_top(8)
            .margin_bottom(8)
            .margin_start(10)
            .margin_end(10)
            .build();
        let when = gtk::Label::builder().label(format_when(v.when)).xalign(0.0).build();
        when.add_css_class("heading");
        row.append(&when);
        let detail = format!("{}  ·  {}", kind_label(v.kind), glib::format_size(v.size));
        let detail = gtk::Label::builder().label(detail).xalign(0.0).build();
        detail.add_css_class("dim-label");
        row.append(&detail);
        list.append(&row);
    }
    let side = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .width_request(260)
        .child(&list)
        .build();
    side.add_css_class("frame");

    let pane = Rc::new(DiffPane::new(DiffTexts {
        legend: tr("history_legend"),
        added: tr("history_added"),
        removed: tr("history_removed"),
        same: tr("history_same"),
    }));

    pane.root.set_margin_start(10);
    let paned = gtk::Paned::builder()
        .orientation(gtk::Orientation::Horizontal)
        .start_child(&side)
        .end_child(&pane.root)
        .resize_start_child(false)
        .shrink_start_child(false)
        .position(280)
        .vexpand(true)
        .build();
    root.append(&paned);

    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    buttons.set_halign(gtk::Align::End);
    let close_btn = gtk::Button::with_label(tr("close"));
    let restore_btn = gtk::Button::with_label(tr("history_restore"));
    restore_btn.add_css_class("suggested-action");
    restore_btn.set_sensitive(false);
    buttons.append(&close_btn);
    buttons.append(&restore_btn);
    root.append(&buttons);

    let versions = Rc::new(versions);
    let current = Arc::new(current);
    // cada escolha invalida a comparação anterior que ainda esteja rodando
    let generation = Rc::new(Cell::new(0u64));
    list.connect_row_selected({
        let (versions, pane, restore_btn) = (versions.clone(), pane.clone(), restore_btn.clone());
        move |_, row| {
            let Some(v) = row.and_then(|r| versions.get(r.index() as usize)) else {
                restore_btn.set_sensitive(false);
                return;
            };
            restore_btn.set_sensitive(true);
            pane.loading();
            let gen = generation.get() + 1;
            generation.set(gen);
            let (current, path) = (current.clone(), v.data.clone());
            let handle = gio::spawn_blocking(move || {
                diff::diff_since(&path, &current).map_err(|e| e.to_string())
            });
            let (pane, generation) = (pane.clone(), generation.clone());
            glib::spawn_future_local(async move {
                let res = handle.await.unwrap_or_else(|_| Err(String::new()));
                if generation.get() == gen {
                    pane.show(res);
                }
            });
        }
    });

    {
        let win = win.clone();
        close_btn.connect_clicked(move |_| win.close());
    }
    {
        let (win, list) = (win.clone(), list.clone());
        restore_btn.connect_clicked(move |_| {
            let Some(v) = list.selected_row().and_then(|r| versions.get(r.index() as usize)) else {
                return;
            };
            let v = v.clone();
            win.close();
            on_restore(v);
        });
    }

    win.set_child(Some(&root));
    win.present();
    if let Some(first) = list.row_at_index(0) {
        list.select_row(Some(&first));
    }
}

fn kind_label(kind: Kind) -> &'static str {
    match kind {
        Kind::Saved => tr("history_kind_saved"),
        Kind::Editing => tr("history_kind_editing"),
        Kind::BeforeRestore => tr("history_kind_before_restore"),
    }
}

/// "04/11/2026 14:32:10".
pub fn format_when(secs: u64) -> String {
    glib::DateTime::from_unix_local(secs as i64)
        .and_then(|d| d.format("%d/%m/%Y %H:%M:%S"))
        .map(|s| s.to_string())
        .unwrap_or_default()
}
