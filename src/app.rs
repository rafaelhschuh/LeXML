use crate::document::DocumentView;
use crate::textdoc::TextDocView;
use crate::xmldoc::XmlDoc;
use crate::{config, i18n};
use crate::i18n::tr;
use adw::prelude::*;
use gtk::glib::clone;
use gtk::{gio, glib};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

const REPO_URL: &str = "https://github.com/rafaelhschuh/LeXML";
const AUTHOR: &str = "Rafael H. Schuh";

/// Documento aberto: tabela (formato Datapacket) ou editor de texto simples.
enum OpenDoc {
    Table(Rc<DocumentView>),
    Text(Rc<TextDocView>),
}

impl OpenDoc {
    fn widget(&self) -> gtk::Widget {
        match self {
            OpenDoc::Table(d) => d.root.clone().upcast(),
            OpenDoc::Text(d) => d.root.clone().upcast(),
        }
    }
    fn is_dirty(&self) -> bool {
        match self {
            OpenDoc::Table(d) => d.is_dirty(),
            OpenDoc::Text(d) => d.is_dirty(),
        }
    }
    fn save_then(&self, after: impl FnOnce(bool) + 'static) {
        match self {
            OpenDoc::Table(d) => d.save_then(after),
            OpenDoc::Text(d) => d.save_then(after),
        }
    }
}

/// O que carregar ao criar a janela.
pub enum Initial {
    Empty,
    Open(PathBuf),
    Blank,
}

pub fn build_window(app: &adw::Application, initial: Initial) -> adw::ApplicationWindow {
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Lê-XML")
        .default_width(1100)
        .default_height(700)
        .build();

    let toolbar = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();

    let open_btn = gtk::Button::from_icon_name("document-open-symbolic");
    open_btn.set_tooltip_text(Some(tr("open_new_window_tooltip")));
    header.pack_start(&open_btn);

    let new_btn = gtk::Button::from_icon_name("document-new-symbolic");
    new_btn.set_tooltip_text(Some(tr("new_file_tooltip")));
    header.pack_start(&new_btn);

    let title = adw::WindowTitle::new("Lê-XML", tr("app_subtitle"));
    header.set_title_widget(Some(&title));

    // botão de configurações — abre uma JANELA flutuante (não um popover).
    let settings_btn = gtk::Button::from_icon_name("emblem-system-symbolic");
    settings_btn.set_tooltip_text(Some(tr("settings")));
    settings_btn.connect_clicked(clone!(@weak window => move |_| show_settings(&window)));
    header.pack_end(&settings_btn);

    toolbar.add_top_bar(&header);

    // pilha: vazio vs documento
    let stack = gtk::Stack::new();
    let placeholder = adw::StatusPage::builder()
        .icon_name("x-office-spreadsheet-symbolic")
        .title(tr("no_file_open"))
        .description(tr("no_file_desc"))
        .build();
    let open_center = gtk::Button::with_label(tr("open_file"));
    open_center.add_css_class("pill");
    open_center.add_css_class("suggested-action");
    open_center.set_halign(gtk::Align::Center);
    placeholder.set_child(Some(&open_center));
    stack.add_named(&placeholder, Some("empty"));
    toolbar.set_content(Some(&stack));

    window.set_content(Some(&toolbar));

    // mantém o documento vivo
    let current: Rc<RefCell<Option<OpenDoc>>> = Rc::new(RefCell::new(None));

    // Abrir sempre em uma NOVA janela, exceto quando a janela atual ainda está
    // vazia (sem documento) — nesse caso aproveitamos a própria janela.
    let open_action = {
        let app = app.clone();
        let window = window.clone();
        let stack = stack.clone();
        let current = current.clone();
        let title = title.clone();
        move || {
            let dialog = gtk::FileDialog::builder().title(tr("open_xml_title")).build();
            let filter = gtk::FileFilter::new();
            filter.set_name(Some(tr("xml_files")));
            filter.add_pattern("*.xml");
            filter.add_pattern("*.XML");
            let filters = gio::ListStore::new::<gtk::FileFilter>();
            filters.append(&filter);
            dialog.set_filters(Some(&filters));

            let app = app.clone();
            let window2 = window.clone();
            let stack = stack.clone();
            let current = current.clone();
            let title = title.clone();
            dialog.open(Some(&window), gio::Cancellable::NONE, move |res| {
                if let Ok(file) = res {
                    if let Some(path) = file.path() {
                        if current.borrow().is_some() {
                            let w = build_window(&app, Initial::Open(path));
                            w.present();
                        } else {
                            open_into(&path, &window2, &stack, &current, &title);
                        }
                    }
                }
            });
        }
    };

    open_btn.connect_clicked(clone!(@strong open_action => move |_| open_action()));
    open_center.connect_clicked(clone!(@strong open_action => move |_| open_action()));

    // novo arquivo em branco
    {
        let app = app.clone();
        let window = window.clone();
        let stack = stack.clone();
        let current = current.clone();
        let title = title.clone();
        new_btn.connect_clicked(move |_| {
            if current.borrow().is_some() {
                let w = build_window(&app, Initial::Blank);
                w.present();
            } else {
                blank_into(&window, &stack, &current, &title);
            }
        });
    }

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
                Some(tr("unsaved_title")),
                Some(tr("unsaved_body")),
            );
            dialog.add_responses(&[
                ("cancel", tr("cancel")),
                ("discard", tr("close_without_saving")),
                ("save", tr("save_ellipsis")),
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

    match initial {
        Initial::Open(path) => open_into(&path, &window, &stack, &current, &title),
        Initial::Blank => blank_into(&window, &stack, &current, &title),
        Initial::Empty => {}
    }

    window
}

/// Substitui o documento exibido na pilha e atualiza título/estado.
fn install_doc(
    doc: OpenDoc,
    window: &adw::ApplicationWindow,
    stack: &gtk::Stack,
    current: &Rc<RefCell<Option<OpenDoc>>>,
    title: &adw::WindowTitle,
    file_label: &str,
) {
    if let Some(child) = stack.child_by_name("doc") {
        stack.remove(&child);
    }
    let widget = doc.widget();
    stack.add_named(&widget, Some("doc"));
    stack.set_visible_child_name("doc");
    *current.borrow_mut() = Some(doc);

    // título: nome do arquivo em cima, nome do app embaixo (só com arquivo aberto)
    title.set_title(file_label);
    title.set_subtitle("Lê-XML");
    window.set_title(Some(&format!("{file_label} — Lê-XML")));
}

fn open_into(
    path: &Path,
    window: &adw::ApplicationWindow,
    stack: &gtk::Stack,
    current: &Rc<RefCell<Option<OpenDoc>>>,
    title: &adw::WindowTitle,
) {
    let doc = match XmlDoc::open(path) {
        Ok(dp) => OpenDoc::Table(DocumentView::new(dp, path.to_path_buf())),
        Err(_) => {
            // XML que não segue a estrutura de tabela → editor de texto comum
            let contents = std::fs::read(path)
                .map(|b| String::from_utf8_lossy(&b).into_owned())
                .unwrap_or_default();
            OpenDoc::Text(TextDocView::new(path.to_path_buf(), &contents))
        }
    };
    let label = path
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "arquivo".into());
    install_doc(doc, window, stack, current, title, &label);
}

fn blank_into(
    window: &adw::ApplicationWindow,
    stack: &gtk::Stack,
    current: &Rc<RefCell<Option<OpenDoc>>>,
    title: &adw::WindowTitle,
) {
    match XmlDoc::new_empty() {
        Ok(dp) => {
            let doc = OpenDoc::Table(DocumentView::new(dp, PathBuf::from("novo.xml")));
            install_doc(doc, window, stack, current, title, "novo.xml");
        }
        Err(e) => {
            let d = adw::MessageDialog::new(Some(window), Some(tr("error")), Some(&e.to_string()));
            d.add_response("ok", tr("ok"));
            d.present();
        }
    }
}

/// Aplica o esquema de cores. O app é SEMPRE Adwaita (GNOME) — nunca mexemos em
/// gtk-theme-name. Só alternamos claro/escuro, de forma FIXA (independente do
/// sistema): ForceLight ou ForceDark. Qualquer valor != "dark" vira claro.
fn apply_theme(scheme_str: &str) {
    use adw::ColorScheme;
    let scheme = if scheme_str == "dark" {
        ColorScheme::ForceDark
    } else {
        ColorScheme::ForceLight
    };
    adw::StyleManager::default().set_color_scheme(scheme);
}

/// Aplica a aparência de plataforma (tema de widgets GTK).
/// Janela de configurações FLUTUANTE com abas (igual à janela "Sobre"):
/// uma `adw::PreferencesWindow` com as páginas Geral, Tema e Sobre.
fn show_settings(parent: &adw::ApplicationWindow) {
    let cfg = config::load();

    let win = adw::PreferencesWindow::builder()
        .title(tr("settings"))
        .transient_for(parent)
        .modal(true)
        .search_enabled(false)
        .default_width(440)
        .default_height(420)
        .build();

    // ---------- Aba: Geral (idioma) ----------
    let geral = adw::PreferencesPage::builder()
        .title(tr("general"))
        .icon_name("preferences-system-symbolic")
        .build();
    let g_group = adw::PreferencesGroup::builder().build();
    let lang_combo = adw::ComboRow::builder()
        .title(tr("language"))
        .model(&gtk::StringList::new(&["Português", "English"]))
        .build();
    lang_combo.set_selected(if cfg.lang.starts_with("en") { 1 } else { 0 });
    g_group.add(&lang_combo);

    // Tema (claro/escuro, fixo) — agora na mesma aba Geral
    let theme_combo = adw::ComboRow::builder()
        .title(tr("color_scheme"))
        .model(&gtk::StringList::new(&[tr("theme_light"), tr("theme_dark")]))
        .build();
    theme_combo.set_selected(if cfg.theme == "dark" { 1 } else { 0 });
    g_group.add(&theme_combo);

    geral.add(&g_group);
    win.add(&geral);

    {
        let win = win.downgrade();
        lang_combo.connect_selected_notify(move |r| {
            let code = if r.selected() == 1 { "en" } else { "pt" };
            let mut c = config::load();
            c.lang = code.to_string();
            config::save(&c);
            if let Some(w) = win.upgrade() {
                w.add_toast(adw::Toast::new(tr("lang_restart_note")));
            }
        });
    }

    theme_combo.connect_selected_notify(move |r| {
        let val = if r.selected() == 1 { "dark" } else { "light" };
        apply_theme(val);
        let mut c = config::load();
        c.theme = val.to_string();
        config::save(&c);
    });

    // ---------- Aba: Sobre ----------
    let sobre = adw::PreferencesPage::builder()
        .title(tr("about"))
        .icon_name("help-about-symbolic")
        .build();
    let s_group = adw::PreferencesGroup::builder().title("Lê-XML").build();
    let desc_row = adw::ActionRow::builder()
        .title(tr("about_desc"))
        .build();
    desc_row.set_title_lines(0);
    s_group.add(&desc_row);
    let ver_row = adw::ActionRow::builder().title("Versão").subtitle("0.2.0").build();
    s_group.add(&ver_row);
    let author_row = adw::ActionRow::builder().title(tr("author")).subtitle(AUTHOR).build();
    s_group.add(&author_row);
    let repo_row = adw::ActionRow::builder()
        .title(tr("repository"))
        .subtitle(REPO_URL)
        .activatable(true)
        .build();
    repo_row.add_suffix(&gtk::Image::from_icon_name("adw-external-link-symbolic"));
    repo_row.connect_activated(|_| {
        gtk::UriLauncher::new(REPO_URL).launch(
            gtk::Window::NONE,
            gio::Cancellable::NONE,
            |_| {},
        );
    });
    s_group.add(&repo_row);
    sobre.add(&s_group);
    win.add(&sobre);

    win.present();
}

pub fn run() -> glib::ExitCode {
    // carrega preferências antes de tudo
    let cfg = config::load();
    i18n::set_lang(i18n::from_code(&cfg.lang));

    let app = adw::Application::builder()
        .application_id("com.empresa.lexml")
        .flags(gio::ApplicationFlags::HANDLES_OPEN)
        .build();

    let theme = cfg.theme.clone();
    app.connect_startup(move |_| {
        // App é sempre Adwaita (GNOME). Só claro/escuro, fixo (independente do
        // sistema). LEXML_THEME sobrepõe a configuração salva.
        match std::env::var("LEXML_THEME").as_deref() {
            Ok("dark") => apply_theme("dark"),
            Ok("light") => apply_theme("light"),
            _ => apply_theme(&theme),
        }
    });
    app.connect_activate(|app| {
        let window = build_window(app, Initial::Empty);
        window.present();
    });
    app.connect_open(|app, files, _hint| {
        for file in files {
            if let Some(path) = file.path() {
                let window = build_window(app, Initial::Open(path));
                window.present();
            }
        }
        if files.is_empty() {
            let window = build_window(app, Initial::Empty);
            window.present();
        }
    });
    app.run()
}
