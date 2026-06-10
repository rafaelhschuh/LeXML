//! Diálogos em GTK4 puro (sem libadwaita).
//!
//! - `alert` / `info` / `error`: mensagem simples com botão OK (`gtk::AlertDialog`).
//! - `form`: janela modal flutuante com um widget customizado + Cancelar/OK,
//!   usada onde antes havia `adw::MessageDialog` com `extra_child`.

use crate::i18n::tr;
use gtk::prelude::*;
use std::rc::Rc;

/// Mensagem informativa simples (cabeçalho + corpo, botão OK).
pub fn info(parent: Option<&gtk::Window>, heading: &str, body: &str) {
    let alert = gtk::AlertDialog::builder()
        .modal(true)
        .message(heading)
        .detail(body)
        .build();
    alert.show(parent);
}

/// Mensagem de erro (cabeçalho fixo "Erro").
pub fn error(parent: Option<&gtk::Window>, body: &str) {
    info(parent, tr("error"), body);
}

/// Janela modal flutuante com um widget customizado e botões Cancelar/OK.
/// `on_ok` é chamado ao confirmar (clique no botão ou Enter no formulário).
pub fn form(
    parent: Option<&gtk::Window>,
    title: &str,
    body: &str,
    child: &impl IsA<gtk::Widget>,
    ok_label: &str,
    on_ok: impl Fn() + 'static,
) {
    let win = gtk::Window::builder()
        .title(title)
        .modal(true)
        .resizable(false)
        .default_width(420)
        .build();
    if let Some(p) = parent {
        win.set_transient_for(Some(p));
    }

    let vbox = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .margin_top(18)
        .margin_bottom(18)
        .margin_start(18)
        .margin_end(18)
        .build();

    if !body.is_empty() {
        let lbl = gtk::Label::builder()
            .label(body)
            .wrap(true)
            .xalign(0.0)
            .build();
        vbox.append(&lbl);
    }
    vbox.append(child);

    let btns = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(6)
        .halign(gtk::Align::End)
        .margin_top(6)
        .build();
    let cancel = gtk::Button::with_label(tr("cancel"));
    let ok = gtk::Button::with_label(ok_label);
    ok.add_css_class("suggested-action");
    btns.append(&cancel);
    btns.append(&ok);
    vbox.append(&btns);

    win.set_child(Some(&vbox));

    let on_ok = Rc::new(on_ok);
    {
        let win2 = win.clone();
        let on_ok = on_ok.clone();
        ok.connect_clicked(move |_| {
            on_ok();
            win2.close();
        });
    }
    {
        let win2 = win.clone();
        cancel.connect_clicked(move |_| win2.close());
    }
    win.present();
}
