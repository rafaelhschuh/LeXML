use crate::dialog;
use crate::i18n::tr;
use gtk::prelude::*;
use gtk::glib::{self, clone};
use gtk::{gdk, gio};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// Editor de texto simples (estilo bloco de notas) para XML que NÃO segue a
/// estrutura de tabela (sem METADATA/FIELDS). Mantém apenas localizar nativo e
/// salvar — sem nenhuma das features de tabela.
pub struct TextDocView {
    pub root: gtk::Box,
    buffer: gtk::TextBuffer,
    status: gtk::Label,
    path: RefCell<PathBuf>,
    dirty: RefCell<bool>,
}

impl TextDocView {
    pub fn new(path: PathBuf, contents: &str) -> Rc<Self> {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);

        let bar = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(6)
            .margin_top(6)
            .margin_bottom(6)
            .margin_start(6)
            .margin_end(6)
            .build();

        let save_btn = gtk::Button::from_icon_name("document-save-symbolic");
        save_btn.set_tooltip_text(Some(tr("save_tooltip_text")));
        bar.append(&save_btn);
        root.append(&bar);

        let buffer = gtk::TextBuffer::new(None);
        buffer.set_text(contents);
        let view = gtk::TextView::builder()
            .buffer(&buffer)
            .monospace(true)
            .left_margin(8)
            .right_margin(8)
            .top_margin(8)
            .bottom_margin(8)
            .build();

        let scroller = gtk::ScrolledWindow::builder().vexpand(true).build();
        scroller.set_child(Some(&view));
        root.append(&scroller);

        let status = gtk::Label::builder()
            .xalign(0.0)
            .margin_start(8)
            .margin_top(4)
            .margin_bottom(4)
            .build();
        status.add_css_class("dim-label");
        status.set_text(tr("text_mode_status"));
        root.append(&status);

        let me = Rc::new(Self {
            root,
            buffer: buffer.clone(),
            status,
            path: RefCell::new(path),
            dirty: RefCell::new(false),
        });

        let me2 = me.clone();
        buffer.connect_changed(move |_| {
            *me2.dirty.borrow_mut() = true;
        });

        save_btn.connect_clicked(clone!(@strong me => move |_| me.do_save()));

        // Ctrl+S salva no arquivo atual; Ctrl+Shift+S abre "Salvar como".
        let save_keys = gtk::EventControllerKey::new();
        save_keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let me_keys = me.clone();
        save_keys.connect_key_pressed(move |_, keyval, _code, state| {
            if state.contains(gdk::ModifierType::CONTROL_MASK)
                && matches!(keyval, gdk::Key::s | gdk::Key::S)
            {
                if state.contains(gdk::ModifierType::SHIFT_MASK) {
                    me_keys.save_as(|_ok| {});
                } else {
                    me_keys.do_save();
                }
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        me.root.add_controller(save_keys);

        me
    }

    pub fn is_dirty(&self) -> bool {
        *self.dirty.borrow()
    }

    fn do_save(self: &Rc<Self>) {
        self.save_then(|_| {});
    }

    /// Grava direto no `path` informado, atualizando o estado. Retorna `true` em sucesso.
    fn write_to(self: &Rc<Self>, path: &Path) -> bool {
        let text = self
            .buffer
            .text(&self.buffer.start_iter(), &self.buffer.end_iter(), false)
            .to_string();
        match std::fs::write(path, text) {
            Ok(()) => {
                *self.dirty.borrow_mut() = false;
                *self.path.borrow_mut() = path.to_path_buf();
                self.status
                    .set_text(&format!("{}: {}", tr("save_tooltip_text"), path.display()));
                true
            }
            Err(e) => {
                self.error(&format!("{}\n{e}", tr("save_error")));
                false
            }
        }
    }

    /// Salva. Se já há um destino em disco (caminho absoluto), sobrescreve direto;
    /// caso contrário, abre "Salvar como".
    pub fn save_then(self: &Rc<Self>, after: impl FnOnce(bool) + 'static) {
        let known = {
            let p = self.path.borrow();
            if p.is_absolute() { Some(p.clone()) } else { None }
        };
        if let Some(path) = known {
            let ok = self.write_to(&path);
            after(ok);
        } else {
            self.save_as(after);
        }
    }

    /// Abre sempre o seletor de salvamento (Salvar como…).
    pub fn save_as(self: &Rc<Self>, after: impl FnOnce(bool) + 'static) {
        let initial = self
            .path
            .borrow()
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "saida.xml".into());
        let dialog = gtk::FileDialog::builder()
            .title(tr("save_as"))
            .initial_name(initial)
            .build();
        let me = self.clone();
        dialog.save(self.window().as_ref(), gio::Cancellable::NONE, move |res| {
            let mut ok = false;
            if let Ok(file) = res {
                if let Some(path) = file.path() {
                    ok = me.write_to(&path);
                }
            }
            after(ok);
        });
    }

    fn window(&self) -> Option<gtk::Window> {
        self.root.root().and_then(|r| r.downcast::<gtk::Window>().ok())
    }

    fn error(&self, body: &str) {
        dialog::error(self.window().as_ref(), body);
    }
}
