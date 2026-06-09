use crate::document::DocumentView;
use crate::xmldoc::XmlDoc;
use adw::prelude::*;
use gtk::glib::clone;
use gtk::{gio, glib};
use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

pub fn build_window(
    app: &adw::Application,
    initial: Option<std::path::PathBuf>,
) -> adw::ApplicationWindow {
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Lê-XML")
        .default_width(1100)
        .default_height(700)
        .build();

    let toolbar = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();

    let open_btn = gtk::Button::from_icon_name("document-open-symbolic");
    open_btn.set_tooltip_text(Some("Abrir XML em nova janela"));
    header.pack_start(&open_btn);

    header.set_title_widget(Some(&adw::WindowTitle::new("Lê-XML", "Leitor de XML em tabela")));
    toolbar.add_top_bar(&header);

    // pilha: vazio vs documento
    let stack = gtk::Stack::new();
    let placeholder = adw::StatusPage::builder()
        .icon_name("x-office-spreadsheet-symbolic")
        .title("Nenhum arquivo aberto")
        .description("Abra um arquivo .xml para visualizar em tabela.")
        .build();
    let open_center = gtk::Button::with_label("Abrir arquivo");
    open_center.add_css_class("pill");
    open_center.add_css_class("suggested-action");
    open_center.set_halign(gtk::Align::Center);
    placeholder.set_child(Some(&open_center));
    stack.add_named(&placeholder, Some("empty"));
    toolbar.set_content(Some(&stack));

    window.set_content(Some(&toolbar));

    // mantém o DocumentView vivo
    let current: Rc<RefCell<Option<Rc<DocumentView>>>> = Rc::new(RefCell::new(None));

    // Abrir sempre em uma NOVA janela, exceto quando a janela atual ainda está
    // vazia (sem documento) — nesse caso aproveitamos a própria janela.
    let open_action = {
        let app = app.clone();
        let window = window.clone();
        let stack = stack.clone();
        let current = current.clone();
        move || {
            let dialog = gtk::FileDialog::builder().title("Abrir XML").build();
            let filter = gtk::FileFilter::new();
            filter.set_name(Some("Arquivos XML"));
            filter.add_pattern("*.xml");
            filter.add_pattern("*.XML");
            let filters = gio::ListStore::new::<gtk::FileFilter>();
            filters.append(&filter);
            dialog.set_filters(Some(&filters));

            let app = app.clone();
            let window2 = window.clone();
            let stack = stack.clone();
            let current = current.clone();
            dialog.open(Some(&window), gio::Cancellable::NONE, move |res| {
                if let Ok(file) = res {
                    if let Some(path) = file.path() {
                        if current.borrow().is_some() {
                            // já há um documento aberto aqui → nova janela
                            let w = build_window(&app, Some(path));
                            w.present();
                        } else {
                            open_into(&path, &window2, &stack, &current);
                        }
                    }
                }
            });
        }
    };

    open_btn.connect_clicked(clone!(@strong open_action => move |_| open_action()));
    open_center.connect_clicked(clone!(@strong open_action => move |_| open_action()));

    // confirmação ao fechar com edições não salvas
    let confirmed_close = Rc::new(std::cell::Cell::new(false));
    window.connect_close_request(clone!(
        @strong current, @strong confirmed_close => @default-return glib::Propagation::Proceed,
        move |win| {
            if confirmed_close.get() {
                return glib::Propagation::Proceed;
            }
            let dirty = current.borrow().as_ref().map_or(false, |d| d.is_dirty());
            if !dirty {
                return glib::Propagation::Proceed;
            }
            let dialog = adw::MessageDialog::new(
                Some(win),
                Some("Alterações não salvas"),
                Some("Você fez edições que ainda não foram salvas. O que deseja fazer?"),
            );
            dialog.add_responses(&[
                ("cancel", "Cancelar"),
                ("discard", "Fechar sem salvar"),
                ("save", "Salvar…"),
            ]);
            dialog.set_response_appearance("discard", adw::ResponseAppearance::Destructive);
            dialog.set_response_appearance("save", adw::ResponseAppearance::Suggested);
            dialog.set_default_response(Some("save"));
            dialog.set_close_response("cancel");
            dialog.connect_response(
                None,
                clone!(@strong current, @strong confirmed_close, @weak win => move |_, resp| {
                    match resp {
                        "discard" => {
                            confirmed_close.set(true);
                            win.close();
                        }
                        "save" => {
                            if let Some(doc) = current.borrow().as_ref() {
                                doc.save_then(clone!(@strong confirmed_close, @weak win => move |ok| {
                                    if ok {
                                        confirmed_close.set(true);
                                        win.close();
                                    }
                                }));
                            }
                        }
                        _ => {}
                    }
                }),
            );
            dialog.present();
            glib::Propagation::Stop
        }
    ));

    if let Some(path) = initial {
        open_into(&path, &window, &stack, &current);
    }

    window
}

fn open_into(
    path: &Path,
    window: &adw::ApplicationWindow,
    stack: &gtk::Stack,
    current: &Rc<RefCell<Option<Rc<DocumentView>>>>,
) {
    match XmlDoc::open(path) {
        Ok(dp) => {
            let doc = DocumentView::new(dp, path.to_path_buf());
            if let Some(child) = stack.child_by_name("doc") {
                stack.remove(&child);
            }
            stack.add_named(&doc.root, Some("doc"));
            stack.set_visible_child_name("doc");
            *current.borrow_mut() = Some(doc);

            // título da janela = nome do arquivo
            if let Some(name) = path.file_name() {
                window.set_title(Some(&format!("{} — Lê-XML", name.to_string_lossy())));
            }
        }
        Err(e) => {
            let d = adw::MessageDialog::new(
                Some(window),
                Some("Falha ao abrir"),
                Some(&format!("{}\n\n{e}", path.display())),
            );
            d.add_response("ok", "OK");
            d.present();
        }
    }
}

pub fn run() -> glib::ExitCode {
    let app = adw::Application::builder()
        .application_id("com.empresa.lexml")
        .build();
    app.connect_startup(|_| {
        // Por padrão segue o sistema (claro/escuro). Override opcional:
        //   LEXML_THEME=light|dark  (força um modo)
        use adw::ColorScheme;
        let scheme = match std::env::var("LEXML_THEME").as_deref() {
            Ok("dark") => ColorScheme::ForceDark,
            Ok("light") => ColorScheme::ForceLight,
            _ => ColorScheme::Default,
        };
        adw::StyleManager::default().set_color_scheme(scheme);
    });
    app.connect_activate(|app| {
        let initial = std::env::args().nth(1).map(std::path::PathBuf::from);
        let window = build_window(app, initial);
        window.present();
    });
    // evita que o GApplication tente tratar o argumento como opção
    app.run_with_args::<String>(&[])
}
