use crate::document::DocumentView;
use crate::textdoc::TextDocView;
use crate::xmldoc::XmlDoc;
use crate::{config, dialog, i18n};
use crate::i18n::tr;
use gtk::glib::clone;
use gtk::prelude::*;
use gtk::{gio, glib};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

const REPO_URL: &str = "https://github.com/rafaelhschuh/LeXML";
const AUTHOR: &str = "Rafael H. Schuh";
const VERSION: &str = env!("CARGO_PKG_VERSION");

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

/// Título de duas linhas para a HeaderBar (substitui `adw::WindowTitle`).
#[derive(Clone)]
struct TitleWidget {
    container: gtk::Box,
    title: gtk::Label,
    subtitle: gtk::Label,
}

impl TitleWidget {
    fn new(title: &str, subtitle: &str) -> Self {
        let container = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .valign(gtk::Align::Center)
            .build();
        let title_lbl = gtk::Label::new(Some(title));
        title_lbl.add_css_class("title");
        title_lbl.set_ellipsize(gtk::pango::EllipsizeMode::End);
        let subtitle_lbl = gtk::Label::new(Some(subtitle));
        subtitle_lbl.add_css_class("subtitle");
        subtitle_lbl.set_ellipsize(gtk::pango::EllipsizeMode::End);
        container.append(&title_lbl);
        container.append(&subtitle_lbl);
        Self {
            container,
            title: title_lbl,
            subtitle: subtitle_lbl,
        }
    }

    fn set(&self, title: &str, subtitle: &str) {
        self.title.set_text(title);
        self.subtitle.set_text(subtitle);
        self.subtitle.set_visible(!subtitle.is_empty());
    }
}

/// O que carregar ao criar a janela.
pub enum Initial {
    Empty,
    Open(PathBuf),
    Blank,
}

pub fn build_window(app: &gtk::Application, initial: Initial) -> gtk::ApplicationWindow {
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("Lê-XML")
        .default_width(1100)
        .default_height(700)
        .build();

    let header = gtk::HeaderBar::new();

    let open_btn = gtk::Button::from_icon_name("document-open-symbolic");
    open_btn.set_tooltip_text(Some(tr("open_new_window_tooltip")));
    header.pack_start(&open_btn);

    let new_btn = gtk::Button::from_icon_name("document-new-symbolic");
    new_btn.set_tooltip_text(Some(tr("new_file_tooltip")));
    header.pack_start(&new_btn);

    let title = TitleWidget::new("Lê-XML", tr("app_subtitle"));
    header.set_title_widget(Some(&title.container));

    // botão de configurações — abre uma JANELA flutuante com abas.
    let settings_btn = gtk::Button::from_icon_name("preferences-system-symbolic");
    settings_btn.set_tooltip_text(Some(tr("settings")));
    settings_btn.connect_clicked(clone!(@weak window => move |_| show_settings(&window)));
    header.pack_end(&settings_btn);

    window.set_titlebar(Some(&header));

    // pilha: vazio vs documento
    let stack = gtk::Stack::new();
    let (placeholder, open_center) = build_placeholder();
    stack.add_named(&placeholder, Some("empty"));
    window.set_child(Some(&stack));

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
            let alert = gtk::AlertDialog::builder()
                .modal(true)
                .message(tr("unsaved_title"))
                .detail(tr("unsaved_body"))
                .buttons([tr("cancel"), tr("close_without_saving"), tr("save_ellipsis")])
                .cancel_button(0)
                .default_button(2)
                .build();
            alert.choose(
                Some(win),
                gio::Cancellable::NONE,
                clone!(@strong current, @strong confirmed_close, @weak win => move |res| {
                    match res.unwrap_or(0) {
                        1 => {
                            confirmed_close.set(true);
                            win.close();
                        }
                        2 => {
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

/// Página inicial "Nenhum arquivo aberto" (substitui `adw::StatusPage`).
fn build_placeholder() -> (gtk::Box, gtk::Button) {
    let page = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .valign(gtk::Align::Center)
        .halign(gtk::Align::Center)
        .vexpand(true)
        .hexpand(true)
        .build();

    let icon = gtk::Image::from_icon_name("x-office-spreadsheet-symbolic");
    icon.set_pixel_size(96);
    icon.add_css_class("dim-label");
    page.append(&icon);

    let title = gtk::Label::new(None);
    title.set_markup(&format!(
        "<span size='xx-large' weight='bold'>{}</span>",
        glib::markup_escape_text(tr("no_file_open"))
    ));
    page.append(&title);

    let desc = gtk::Label::builder()
        .label(tr("no_file_desc"))
        .wrap(true)
        .justify(gtk::Justification::Center)
        .build();
    desc.add_css_class("dim-label");
    page.append(&desc);

    let open_center = gtk::Button::with_label(tr("open_file"));
    open_center.add_css_class("suggested-action");
    open_center.add_css_class("pill");
    open_center.set_halign(gtk::Align::Center);
    open_center.set_margin_top(6);
    page.append(&open_center);

    (page, open_center)
}

/// Substitui o documento exibido na pilha e atualiza título/estado.
fn install_doc(
    doc: OpenDoc,
    window: &gtk::ApplicationWindow,
    stack: &gtk::Stack,
    current: &Rc<RefCell<Option<OpenDoc>>>,
    title: &TitleWidget,
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
    title.set(file_label, "Lê-XML");
    window.set_title(Some(&format!("{file_label} — Lê-XML")));
}

fn open_into(
    path: &Path,
    window: &gtk::ApplicationWindow,
    stack: &gtk::Stack,
    current: &Rc<RefCell<Option<OpenDoc>>>,
    title: &TitleWidget,
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
    window: &gtk::ApplicationWindow,
    stack: &gtk::Stack,
    current: &Rc<RefCell<Option<OpenDoc>>>,
    title: &TitleWidget,
) {
    match XmlDoc::new_empty() {
        Ok(dp) => {
            let doc = OpenDoc::Table(DocumentView::new(dp, PathBuf::from("novo.xml")));
            install_doc(doc, window, stack, current, title, "novo.xml");
        }
        Err(e) => dialog::error(Some(window.upcast_ref()), &e.to_string()),
    }
}

/// Aplica o esquema de cores em GTK4 puro. Sem libadwaita, o app herda o tema
/// do sistema (e a cor de destaque dele). Só sobrepomos claro/escuro quando o
/// usuário escolhe explicitamente; em "system" deixamos o GTK seguir o desktop.
fn apply_theme(scheme: &str) {
    let Some(settings) = gtk::Settings::default() else { return };
    match scheme {
        "dark" => settings.set_gtk_application_prefer_dark_theme(true),
        "light" => settings.set_gtk_application_prefer_dark_theme(false),
        _ => {} // system: não sobrepõe — o GTK segue o tema/portal do desktop
    }
}

/// Janela de configurações FLUTUANTE com abas (Notebook), em GTK4 puro.
fn show_settings(parent: &gtk::ApplicationWindow) {
    let cfg = config::load();

    let win = gtk::Window::builder()
        .title(tr("settings"))
        .transient_for(parent)
        .modal(true)
        .resizable(false)
        .default_width(460)
        .default_height(420)
        .build();

    let notebook = gtk::Notebook::new();

    // ---------- Aba: Geral (idioma + tema) ----------
    let geral = settings_page();

    let lang_dd = gtk::DropDown::from_strings(&["Português", "English"]);
    lang_dd.set_selected(if cfg.lang.starts_with("en") { 1 } else { 0 });
    geral.append(&pref_row(tr("language"), &lang_dd));

    let theme_dd = gtk::DropDown::from_strings(&[
        tr("theme_system"),
        tr("theme_light"),
        tr("theme_dark"),
    ]);
    theme_dd.set_selected(match cfg.theme.as_str() {
        "light" => 1,
        "dark" => 2,
        _ => 0,
    });
    geral.append(&pref_row(tr("color_scheme"), &theme_dd));

    let note = gtk::Label::builder()
        .label(tr("lang_restart_note"))
        .wrap(true)
        .xalign(0.0)
        .margin_top(6)
        .build();
    note.add_css_class("dim-label");
    geral.append(&note);

    notebook.append_page(&geral, Some(&gtk::Label::new(Some(tr("general")))));

    lang_dd.connect_selected_notify(move |dd| {
        let code = if dd.selected() == 1 { "en" } else { "pt" };
        let mut c = config::load();
        c.lang = code.to_string();
        config::save(&c);
    });

    theme_dd.connect_selected_notify(move |dd| {
        let val = match dd.selected() {
            1 => "light",
            2 => "dark",
            _ => "system",
        };
        apply_theme(val);
        let mut c = config::load();
        c.theme = val.to_string();
        config::save(&c);
    });

    // ---------- Aba: Sobre ----------
    let sobre = settings_page();

    let app_name = gtk::Label::new(None);
    app_name.set_markup("<span size='x-large' weight='bold'>Lê-XML</span>");
    app_name.set_xalign(0.0);
    sobre.append(&app_name);

    let desc = gtk::Label::builder()
        .label(tr("about_desc"))
        .wrap(true)
        .xalign(0.0)
        .build();
    desc.add_css_class("dim-label");
    sobre.append(&desc);

    sobre.append(&info_row("Versão", VERSION));
    sobre.append(&info_row(tr("author"), AUTHOR));

    let repo_btn = gtk::Button::builder()
        .label(REPO_URL)
        .halign(gtk::Align::Start)
        .margin_top(6)
        .build();
    repo_btn.add_css_class("link");
    repo_btn.connect_clicked(|_| {
        gtk::UriLauncher::new(REPO_URL).launch(
            gtk::Window::NONE,
            gio::Cancellable::NONE,
            |_| {},
        );
    });
    sobre.append(&info_row(tr("repository"), ""));
    sobre.append(&repo_btn);

    notebook.append_page(&sobre, Some(&gtk::Label::new(Some(tr("about")))));

    win.set_child(Some(&notebook));
    win.present();
}

/// Container vertical com margens, usado como página de uma aba de configurações.
fn settings_page() -> gtk::Box {
    gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .margin_top(18)
        .margin_bottom(18)
        .margin_start(18)
        .margin_end(18)
        .build()
}

/// Linha "rótulo … widget" (rótulo à esquerda, controle à direita).
fn pref_row(label: &str, widget: &impl IsA<gtk::Widget>) -> gtk::Box {
    let row = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(12)
        .build();
    let lbl = gtk::Label::builder().label(label).xalign(0.0).hexpand(true).build();
    row.append(&lbl);
    widget.set_halign(gtk::Align::End);
    widget.set_valign(gtk::Align::Center);
    row.append(widget);
    row
}

/// Linha de informação "rótulo: valor" (somente leitura).
fn info_row(label: &str, value: &str) -> gtk::Box {
    let row = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(12)
        .build();
    let lbl = gtk::Label::builder().label(label).xalign(0.0).hexpand(true).build();
    row.append(&lbl);
    if !value.is_empty() {
        let val = gtk::Label::builder().label(value).xalign(1.0).build();
        val.add_css_class("dim-label");
        row.append(&val);
    }
    row
}

pub fn run() -> glib::ExitCode {
    // carrega preferências antes de tudo
    let cfg = config::load();
    i18n::set_lang(i18n::from_code(&cfg.lang));

    let app = gtk::Application::builder()
        .application_id("com.empresa.lexml")
        .flags(gio::ApplicationFlags::HANDLES_OPEN)
        .build();

    let theme = cfg.theme.clone();
    app.connect_startup(move |_| {
        // GTK4 puro: o app segue o tema (e cor de destaque) do sistema. Só
        // sobrepomos claro/escuro quando configurado. LEXML_THEME sobrepõe.
        match std::env::var("LEXML_THEME").as_deref() {
            Ok("dark") => apply_theme("dark"),
            Ok("light") => apply_theme("light"),
            Ok("system") => apply_theme("system"),
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
