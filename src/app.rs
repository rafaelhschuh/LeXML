use crate::document::DocumentView;
use crate::i18n::tr;
use crate::textdoc::TextDocView;
use crate::xmldoc::{QueryResult, XmlDoc};
use crate::{backup, config, dialog, i18n, recent};
use gtk::glib::clone;
use gtk::prelude::*;
use gtk::{gio, glib};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// Identificador do app (reverse-DNS). Deve ser IGUAL ao nome do arquivo
/// `.desktop` e do ícone, para o compositor associar a janela ao lançador
/// (ícone correto na barra/dock, não só no menu de apps).
const APP_ID: &str = "br.dev.schuh.lexml";
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
    /// Libera os recursos pesados do documento (store de linhas, colunas) ao
    /// ser substituído por outro — evita acúmulo de memória entre aberturas.
    fn teardown(&self) {
        if let OpenDoc::Table(d) = self {
            d.teardown();
        }
    }
    /// Fechamento normal: a cópia de recuperação não é mais necessária.
    fn discard_backup(&self) {
        if let OpenDoc::Table(d) = self {
            d.discard_backup();
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

/// De onde vem o conteúdo aberto por `open_into`.
enum Source {
    /// O próprio arquivo em disco.
    File,
    /// Cópia de recuperação deixada por uma sessão que caiu.
    Recovery(backup::Orphan),
    /// Versão do histórico restaurada sobre o documento.
    Version(backup::Version),
}

/// O que carregar ao criar a janela.
pub enum Initial {
    Empty,
    Open(PathBuf),
    Blank,
    /// Reabre uma cópia de recuperação deixada por uma sessão que caiu.
    Recover(backup::Orphan),
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
    // Seta colada ao botão Abrir com a lista de arquivos recentes.
    let recent_menu = gio::Menu::new();
    let recent_btn = gtk::MenuButton::builder()
        .icon_name("pan-down-symbolic")
        .menu_model(&recent_menu)
        .tooltip_text(tr("recent_tooltip"))
        .build();
    let open_box = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    open_box.add_css_class("linked");
    open_box.append(&open_btn);
    open_box.append(&recent_btn);
    header.pack_start(&open_box);

    let new_btn = gtk::Button::from_icon_name("document-new-symbolic");
    new_btn.set_tooltip_text(Some(tr("new_file_tooltip")));
    header.pack_start(&new_btn);

    let home_btn = gtk::Button::from_icon_name("go-home-symbolic");
    home_btn.set_tooltip_text(Some(tr("home_tooltip")));
    home_btn.set_sensitive(false);
    header.pack_start(&home_btn);

    let title = TitleWidget::new("Lê-XML", tr("app_subtitle"));
    header.set_title_widget(Some(&title.container));

    // botão de configurações — abre uma JANELA flutuante com abas.
    let settings_btn = gtk::Button::from_icon_name("preferences-system-symbolic");
    settings_btn.set_tooltip_text(Some(tr("settings")));
    settings_btn.connect_clicked(clone!(
        #[weak]
        window,
        move |_| show_settings(&window)
    ));
    header.pack_end(&settings_btn);

    window.set_titlebar(Some(&header));

    // pilha: vazio vs documento
    let stack = gtk::Stack::new();
    let (placeholder, open_center, recent_box) = build_placeholder();
    stack.add_named(&placeholder, Some("empty"));
    window.set_child(Some(&stack));
    stack.connect_visible_child_name_notify(clone!(
        #[weak]
        home_btn,
        move |s| {
            home_btn.set_sensitive(s.visible_child_name().as_deref() == Some("doc"));
        }
    ));

    // mantém o documento vivo
    let current: Rc<RefCell<Option<OpenDoc>>> = Rc::new(RefCell::new(None));

    // Abrir sempre em uma NOVA janela, exceto quando a janela atual ainda está
    // vazia (sem documento) — nesse caso aproveitamos a própria janela.
    let open_path: Rc<dyn Fn(PathBuf)> = {
        let app = app.clone();
        let window = window.downgrade();
        let stack = stack.clone();
        let current = current.clone();
        let title = title.clone();
        Rc::new(move |path: PathBuf| {
            let Some(window) = window.upgrade() else {
                return;
            };
            if current.borrow().is_some() {
                build_window(&app, Initial::Open(path)).present();
            } else {
                open_checked(&path, &window, &stack, &current, &title);
            }
        })
    };

    let open_action = {
        let window = window.clone();
        let open_path = open_path.clone();
        move || {
            let dialog = gtk::FileDialog::builder()
                .title(tr("open_xml_title"))
                .build();
            let filter = gtk::FileFilter::new();
            filter.set_name(Some(tr("xml_files")));
            filter.add_pattern("*.xml");
            filter.add_pattern("*.XML");
            let filters = gio::ListStore::new::<gtk::FileFilter>();
            filters.append(&filter);
            dialog.set_filters(Some(&filters));

            let open_path = open_path.clone();
            dialog.open(Some(&window), gio::Cancellable::NONE, move |res| {
                if let Some(path) = res.ok().and_then(|f| f.path()) {
                    open_path(path);
                }
            });
        }
    };

    open_btn.connect_clicked(clone!(
        #[strong]
        open_action,
        move |_| open_action()
    ));

    // Arquivos recentes: tela inicial e menu da seta. A lista é relida quando
    // a janela ganha o foco ou o menu abre, porque outra janela pode tê-la
    // alterado.
    let refresh_recent: Rc<dyn Fn()> = {
        let recent_box = recent_box.clone();
        let recent_menu = recent_menu.clone();
        Rc::new(move || {
            let list = recent::list();
            fill_recent_page(&recent_box, &list);
            fill_recent_menu(&recent_menu, &list);
        })
    };
    refresh_recent();
    window.connect_is_active_notify(clone!(
        #[strong]
        refresh_recent,
        move |w| {
            if w.is_active() {
                refresh_recent();
            }
        }
    ));
    recent_btn.connect_active_notify(clone!(
        #[strong]
        refresh_recent,
        move |b| {
            if b.is_active() {
                refresh_recent();
            }
        }
    ));

    let open_recent = gio::SimpleAction::new("open-recent", Some(glib::VariantTy::STRING));
    {
        let open_path = open_path.clone();
        let refresh_recent = refresh_recent.clone();
        let window = window.downgrade();
        open_recent.connect_activate(move |_, param| {
            let Some(path) = param.and_then(|v| v.get::<String>()).map(PathBuf::from) else {
                return;
            };
            if !path.is_file() {
                if let Some(w) = window.upgrade() {
                    dialog::error(Some(w.upcast_ref()), tr("recent_missing"));
                }
                refresh_recent();
                return;
            }
            open_path(path);
        });
    }
    window.add_action(&open_recent);

    let clear_recent = gio::SimpleAction::new("clear-recent", None);
    clear_recent.connect_activate(clone!(
        #[strong]
        refresh_recent,
        move |_, _| {
            recent::clear();
            refresh_recent();
        }
    ));
    window.add_action(&clear_recent);
    open_center.connect_clicked(clone!(
        #[strong]
        open_action,
        move |_| open_action()
    ));

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
        #[strong]
        current,
        #[strong]
        confirmed_close,
        move |win| {
            if confirmed_close.get() {
                if let Some(d) = current.borrow().as_ref() {
                    d.discard_backup();
                }
                return glib::Propagation::Proceed;
            }
            let dirty = current.borrow().as_ref().is_some_and(|d| d.is_dirty());
            if !dirty {
                if let Some(d) = current.borrow().as_ref() {
                    d.discard_backup();
                }
                return glib::Propagation::Proceed;
            }
            let proceed: Rc<dyn Fn()> = Rc::new(clone!(
                #[strong]
                confirmed_close,
                #[weak]
                win,
                move || {
                    confirmed_close.set(true);
                    win.close();
                }
            ));
            confirm_unsaved(win, &current, proceed);
            glib::Propagation::Stop
        }
    ));

    // volta à tela inicial, fechando o documento desta janela
    {
        let (window_w, stack, current, title) = (
            window.downgrade(),
            stack.clone(),
            current.clone(),
            title.clone(),
        );
        let refresh_recent = refresh_recent.clone();
        home_btn.connect_clicked(move |_| {
            let Some(window) = window_w.upgrade() else {
                return;
            };
            let go_home: Rc<dyn Fn()> = Rc::new(clone!(
                #[weak]
                window,
                #[weak]
                stack,
                #[strong]
                current,
                #[strong]
                title,
                #[strong]
                refresh_recent,
                move || {
                    if let Some(doc) = current.borrow_mut().take() {
                        doc.discard_backup();
                        doc.teardown();
                    }
                    stack.set_visible_child_name("empty");
                    if let Some(child) = stack.child_by_name("doc") {
                        stack.remove(&child);
                    }
                    title.set("Lê-XML", tr("app_subtitle"));
                    window.set_title(Some("Lê-XML"));
                    refresh_recent();
                }
            ));
            confirm_unsaved(&window, &current, go_home);
        });
    }

    match initial {
        Initial::Open(path) => open_checked(&path, &window, &stack, &current, &title),
        Initial::Recover(orphan) => {
            let data = orphan.data.clone();
            open_into(
                &data,
                &window,
                &stack,
                &current,
                &title,
                Source::Recovery(orphan),
            );
        }
        Initial::Blank => blank_into(&window, &stack, &current, &title),
        Initial::Empty => {}
    }

    window
}

/// Com edições não salvas, pergunta Cancelar / Fechar sem salvar / Salvar…
/// antes de `proceed`; sem edições segue direto.
fn confirm_unsaved(
    win: &gtk::ApplicationWindow,
    current: &Rc<RefCell<Option<OpenDoc>>>,
    proceed: Rc<dyn Fn()>,
) {
    let dirty = current.borrow().as_ref().is_some_and(|d| d.is_dirty());
    if !dirty {
        proceed();
        return;
    }
    let alert = gtk::AlertDialog::builder()
        .modal(true)
        .message(tr("unsaved_title"))
        .detail(tr("unsaved_body"))
        .buttons([
            tr("cancel"),
            tr("close_without_saving"),
            tr("save_ellipsis"),
        ])
        .cancel_button(0)
        .default_button(2)
        .build();
    let current = current.clone();
    alert.choose(Some(win), gio::Cancellable::NONE, move |res| {
        match res.unwrap_or(0) {
            1 => proceed(),
            2 => {
                if let Some(doc) = current.borrow().as_ref() {
                    let proceed = proceed.clone();
                    doc.save_then(move |ok| {
                        if ok {
                            proceed();
                        }
                    });
                }
            }
            _ => {}
        }
    });
}

thread_local! {
    /// Cópias de recuperação já oferecidas ao abrir o arquivo de origem, para
    /// o diálogo geral da abertura do app não perguntar de novo por elas.
    static CLAIMED: RefCell<std::collections::HashSet<PathBuf>> = RefCell::default();
}

/// Uma vez por execução: se sobraram cópias de recuperação de uma sessão que
/// não fechou normalmente, oferece Restaurar / Descartar / Ver diferenças.
fn offer_recovery(app: &gtk::Application, parent: &gtk::ApplicationWindow) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static ASKED: AtomicBool = AtomicBool::new(false);
    if ASKED.swap(true, Ordering::Relaxed) {
        return;
    }
    let orphans: Vec<backup::Orphan> = backup::orphans()
        .into_iter()
        .filter(|o| !CLAIMED.with(|c| c.borrow().contains(&o.data)))
        .collect();
    if orphans.is_empty() {
        return;
    }
    let app = app.clone();
    crate::recovery::show_dialog(
        parent.upcast_ref(),
        orphans,
        Rc::new(move |o| build_window(&app, Initial::Recover(o)).present()),
        Box::new(|| {}),
    );
}

/// Abre `path`, mas antes, se ele tem uma cópia de recuperação deixada por
/// uma sessão que caiu, pergunta o que fazer com ela (como o Office faz).
fn open_checked(
    path: &Path,
    window: &gtk::ApplicationWindow,
    stack: &gtk::Stack,
    current: &Rc<RefCell<Option<OpenDoc>>>,
    title: &TitleWidget,
) {
    let canon = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    let target = canon(path);
    let found = backup::orphans()
        .into_iter()
        .find(|o| o.original.as_deref().map(canon).as_ref() == Some(&target));
    let Some(orphan) = found else {
        open_into(path, window, stack, current, title, Source::File);
        return;
    };
    CLAIMED.with(|c| c.borrow_mut().insert(orphan.data.clone()));

    let restored = Rc::new(std::cell::Cell::new(false));
    let on_restore: Rc<dyn Fn(backup::Orphan)> = {
        let (window, stack, current, title) = (
            window.clone(),
            stack.clone(),
            current.clone(),
            title.clone(),
        );
        let restored = restored.clone();
        Rc::new(move |o: backup::Orphan| {
            restored.set(true);
            let data = o.data.clone();
            open_into(
                &data,
                &window,
                &stack,
                &current,
                &title,
                Source::Recovery(o),
            );
        })
    };
    // Descartar ou decidir depois: abre o arquivo como está em disco.
    let on_finish: Box<dyn FnOnce()> = {
        let (window, stack, current, title) = (
            window.clone(),
            stack.clone(),
            current.clone(),
            title.clone(),
        );
        let path = path.to_path_buf();
        Box::new(move || {
            if !restored.get() {
                open_into(&path, &window, &stack, &current, &title, Source::File);
            }
        })
    };
    crate::recovery::show_dialog(window.upcast_ref(), vec![orphan], on_restore, on_finish);
}

/// Página inicial "Nenhum arquivo aberto" (substitui `adw::StatusPage`).
/// Devolve também a caixa onde a lista de recentes é montada.
fn build_placeholder() -> (gtk::Box, gtk::Button, gtk::Box) {
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

    let recent_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .margin_top(24)
        .width_request(460)
        .build();
    page.append(&recent_box);

    (page, open_center, recent_box)
}

/// Quantos recentes aparecem na tela inicial (o menu mostra todos).
const RECENT_ON_PAGE: usize = 8;

/// Pasta do arquivo para exibição, com a pasta pessoal abreviada para `~`.
fn display_dir(path: &Path) -> String {
    let dir = path.parent().unwrap_or(Path::new("/"));
    let home = glib::home_dir();
    match dir.strip_prefix(&home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".into(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => dir.display().to_string(),
    }
}

/// Encurta `s` para no máximo `max` caracteres, trocando o meio por `…` —
/// o começo e o fim de um caminho são as partes que identificam o arquivo.
fn shorten_middle(s: &str, max: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max || max < 3 {
        return s.to_string();
    }
    let keep = max - 1;
    let head = keep / 2;
    let tail = keep - head;
    let mut out: String = chars[..head].iter().collect();
    out.push('…');
    out.extend(&chars[chars.len() - tail..]);
    out
}

/// Limite, em caracteres, do nome e da pasta nas listas de recentes.
const RECENT_MAX_CHARS: usize = 60;

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// Monta a lista de recentes da tela inicial. Cada linha dispara a ação
/// `win.open-recent` com o caminho do arquivo.
fn fill_recent_page(container: &gtk::Box, list: &[PathBuf]) {
    while let Some(c) = container.first_child() {
        container.remove(&c);
    }
    container.set_visible(!list.is_empty());
    if list.is_empty() {
        return;
    }

    let head = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let heading = gtk::Label::builder()
        .label(tr("recent_title"))
        .xalign(0.0)
        .hexpand(true)
        .build();
    heading.add_css_class("heading");
    head.append(&heading);
    let clear = gtk::Button::builder()
        .label(tr("clear_recent"))
        .action_name("win.clear-recent")
        .build();
    clear.add_css_class("flat");
    head.append(&clear);
    container.append(&head);

    let listbox = gtk::ListBox::new();
    listbox.set_selection_mode(gtk::SelectionMode::None);
    listbox.add_css_class("frame");
    listbox.add_css_class("recent-list");
    for path in list.iter().take(RECENT_ON_PAGE) {
        let text = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(2)
            .margin_start(12)
            .margin_end(12)
            .margin_top(8)
            .margin_bottom(8)
            .build();
        // `max_width_chars` limita a largura natural; sem ele o rótulo
        // reivindica o caminho inteiro e a reticência nunca aparece.
        let name = gtk::Label::builder()
            .label(file_name(path))
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::Middle)
            .max_width_chars(RECENT_MAX_CHARS as i32)
            .build();
        let dir = gtk::Label::builder()
            .label(display_dir(path))
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::Middle)
            .max_width_chars(RECENT_MAX_CHARS as i32)
            .build();
        dir.add_css_class("dim-label");
        text.append(&name);
        text.append(&dir);

        let row = gtk::ListBoxRow::new();
        row.set_child(Some(&text));
        row.set_tooltip_text(Some(&path.display().to_string()));
        row.set_action_name(Some("win.open-recent"));
        row.set_action_target_value(Some(&path.to_string_lossy().to_variant()));
        listbox.append(&row);
    }
    container.append(&listbox);
}

/// Monta o menu de recentes da seta ao lado do botão Abrir.
fn fill_recent_menu(menu: &gio::Menu, list: &[PathBuf]) {
    menu.remove_all();
    let files = gio::Menu::new();
    if list.is_empty() {
        // item sem ação: aparece desabilitado
        files.append(Some(tr("no_recent")), None);
    }
    for path in list {
        // Rótulos de menu tratam `_` como mnemônico; dobrar o mantém visível.
        // Itens de menu não reticenciam sozinhos: o texto já vai encurtado.
        let label = format!(
            "{}  —  {}",
            shorten_middle(&file_name(path), RECENT_MAX_CHARS),
            shorten_middle(&display_dir(path), RECENT_MAX_CHARS)
        )
        .replace('_', "__");
        let item = gio::MenuItem::new(Some(&label), None);
        item.set_action_and_target_value(
            Some("win.open-recent"),
            Some(&path.to_string_lossy().to_variant()),
        );
        files.append_item(&item);
    }
    menu.append_section(None, &files);
    if !list.is_empty() {
        let actions = gio::Menu::new();
        actions.append(Some(tr("clear_recent")), Some("win.clear-recent"));
        menu.append_section(None, &actions);
    }
}

/// Substitui o documento exibido na pilha e atualiza título/estado.
/// Instala `doc` no stack. `reveal` controla se a tabela/texto já aparece: na
/// abertura de tabela grande passamos `false` para manter o spinner visível até
/// o carregamento em blocos terminar; `true` mostra imediatamente. O documento
/// anterior é liberado (`teardown`) para não acumular memória entre aberturas.
fn install_doc(
    doc: OpenDoc,
    window: &gtk::ApplicationWindow,
    stack: &gtk::Stack,
    current: &Rc<RefCell<Option<OpenDoc>>>,
    title: &TitleWidget,
    file_label: &str,
    reveal: bool,
) {
    if let Some(old) = current.borrow_mut().take() {
        old.teardown();
    }
    if let Some(child) = stack.child_by_name("doc") {
        stack.remove(&child);
    }
    let widget = doc.widget();
    stack.add_named(&widget, Some("doc"));
    if let OpenDoc::Table(view) = &doc {
        // Restaurar uma versão do histórico troca o documento desta janela.
        let (window, stack, current, title) = (
            window.downgrade(),
            stack.downgrade(),
            Rc::downgrade(current),
            title.clone(),
        );
        view.set_restore_handler(Rc::new(move |v: backup::Version| {
            let (Some(window), Some(stack), Some(current)) =
                (window.upgrade(), stack.upgrade(), current.upgrade())
            else {
                return;
            };
            let data = v.data.clone();
            open_into(&data, &window, &stack, &current, &title, Source::Version(v));
        }));
    }
    *current.borrow_mut() = Some(doc);

    if reveal {
        reveal_doc(window, stack, title, file_label);
    }
}

/// Torna a página do documento visível e ajusta o título (fim do carregamento).
fn reveal_doc(
    window: &gtk::ApplicationWindow,
    stack: &gtk::Stack,
    title: &TitleWidget,
    file_label: &str,
) {
    stack.set_visible_child_name("doc");
    // título: nome do arquivo em cima, nome do app embaixo (só com arquivo aberto)
    title.set(file_label, "Lê-XML");
    window.set_title(Some(&format!("{file_label} — Lê-XML")));
}

/// Resultado do carregamento feito em segundo plano (fora da thread de UI).
/// `XmlDoc` e `QueryResult` são `Send`, então tanto o parse (XML → SQLite)
/// quanto a consulta inicial completa rodam no pool de threads da GIO; só o
/// resultado pronto volta para a main thread para montar os widgets em blocos.
#[allow(clippy::large_enum_variant)] // passa uma vez só, da thread para a UI
enum Loaded {
    Table(XmlDoc, QueryResult),
    Text(TextLoad),
}

/// Página central com spinner exibida enquanto o arquivo carrega.
fn build_loading() -> gtk::Box {
    let page = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .valign(gtk::Align::Center)
        .halign(gtk::Align::Center)
        .vexpand(true)
        .hexpand(true)
        .build();
    let spinner = gtk::Spinner::new();
    spinner.set_size_request(48, 48);
    spinner.start();
    page.append(&spinner);
    let lbl = gtk::Label::new(Some(tr("loading")));
    lbl.add_css_class("dim-label");
    page.append(&lbl);
    page
}

/// Mostra imediatamente a janela com um spinner e carrega o arquivo em segundo
/// plano; ao terminar, troca o spinner pela tabela (ou pelo editor de texto).
/// Assim o duplo-clique no .xml abre a janela na hora, sem o usuário achar que
/// nada aconteceu e clicar várias vezes.
fn open_into(
    path: &Path,
    window: &gtk::ApplicationWindow,
    stack: &gtk::Stack,
    current: &Rc<RefCell<Option<OpenDoc>>>,
    title: &TitleWidget,
    source: Source,
) {
    let label = match &source {
        Source::Recovery(o) => format!("{} ({})", o.name, tr("recovered")),
        Source::Version(v) => format!(
            "{} ({} {})",
            v.name,
            tr("version_of"),
            crate::history::format_when(v.when)
        ),
        Source::File => path
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "arquivo".into()),
    };

    // Exibe a página de carregamento de imediato.
    if stack.child_by_name("loading").is_none() {
        let page = build_loading();
        stack.add_named(&page, Some("loading"));
    }
    stack.set_visible_child_name("loading");
    title.set(&label, tr("loading"));
    window.set_title(Some(&format!("{label} — Lê-XML")));

    // Parse pesado + consulta inicial completa, ambos no pool de threads da GIO
    // (fora da thread de UI). Só dados prontos voltam para a main thread.
    let path_bg = path.to_path_buf();
    let handle = gio::spawn_blocking(move || match XmlDoc::open(&path_bg) {
        Ok(dp) => match dp.filter(None) {
            Ok(res) => Loaded::Table(dp, res),
            Err(e) => Loaded::Text(read_text_with_note(&path_bg, &e)),
        },
        // XML que não segue a estrutura de tabela → editor de texto comum
        Err(e) => Loaded::Text(read_text_with_note(&path_bg, &e)),
    });

    // Retoma na main thread para montar os widgets e instalar o documento.
    let path_ui = path.to_path_buf();
    let window = window.clone();
    let stack = stack.clone();
    let current = current.clone();
    let title = title.clone();
    glib::spawn_future_local(async move {
        match handle.await {
            Ok(Loaded::Table(dp, initial)) => {
                // Na recuperação, o documento "é" o arquivo de origem (Salvar
                // grava nele), não a cópia que está no cache.
                let view_path = match &source {
                    Source::Recovery(o) => {
                        o.original.clone().unwrap_or_else(|| PathBuf::from(&o.name))
                    }
                    Source::Version(v) => v.original.clone(),
                    Source::File => {
                        recent::add(&path_ui);
                        path_ui.clone()
                    }
                };
                let view = DocumentView::new(dp, view_path);
                match &source {
                    Source::Recovery(o) => {
                        view.mark_recovered(tr("recovered_status"));
                        // o conteúdo está na memória e entra na cópia desta sessão
                        o.discard();
                    }
                    // a versão continua no histórico
                    Source::Version(_) => view.mark_recovered(tr("restored_status")),
                    Source::File => {}
                }
                // Instala a tabela mas mantém o spinner até a carga terminar.
                install_doc(
                    OpenDoc::Table(view.clone()),
                    &window,
                    &stack,
                    &current,
                    &title,
                    &label,
                    false,
                );
                // Carrega as linhas em blocos; ao concluir, revela a tabela.
                let (window, stack, title) = (window.clone(), stack.clone(), title.clone());
                let label = label.clone();
                view.load_initial(initial, move || {
                    reveal_doc(&window, &stack, &title, &label);
                });
            }
            Ok(Loaded::Text(contents)) => {
                recent::add(&path_ui);
                let doc = OpenDoc::Text(TextDocView::new(
                    path_ui,
                    &contents.text,
                    contents.read_only,
                    contents.note.as_deref(),
                ));
                install_doc(doc, &window, &stack, &current, &title, &label, true);
            }
            Err(_) => {
                dialog::error(Some(window.upcast_ref()), tr("open_error"));
            }
        }
    });
}

/// Lê um arquivo como texto (UTF-8, com perda tolerada) para o modo texto.
/// Teto para abrir um arquivo no editor de texto. Sem limite, um XML de vários
/// GB é carregado inteiro na memória (e o editor do GTK fica inutilizável).
const MAX_TEXT_BYTES: u64 = 64 * 1024 * 1024;

/// Conteúdo carregado no editor de texto.
pub struct TextLoad {
    pub text: String,
    /// `true` quando gravar por cima destruiria o arquivo: ou ele não é UTF-8
    /// válido (os bytes inválidos viraram U+FFFD na leitura), ou foi cortado
    /// no teto de tamanho. Nos dois casos o editor abre somente para leitura.
    pub read_only: bool,
    /// Por que o arquivo não abriu em tabela, quando houve um motivo concreto
    /// (campo com nome inválido, METADATA ausente…). Mostrado na barra de
    /// status para o fallback não parecer arbitrário.
    pub note: Option<String>,
}

fn read_text(path: &Path) -> TextLoad {
    use std::io::Read;
    let mut buf = Vec::new();
    let mut truncated = false;
    if let Ok(f) = std::fs::File::open(path) {
        let _ = f.take(MAX_TEXT_BYTES).read_to_end(&mut buf);
        truncated = buf.len() as u64 >= MAX_TEXT_BYTES;
    }
    match String::from_utf8(buf) {
        Ok(text) => TextLoad {
            text,
            read_only: truncated,
            note: None,
        },
        Err(e) => TextLoad {
            text: String::from_utf8_lossy(e.as_bytes()).into_owned(),
            read_only: true,
            note: None,
        },
    }
}

/// Mesmo que `read_text`, guardando o motivo de o arquivo não ter aberto em tabela.
fn read_text_with_note(path: &Path, why: &anyhow::Error) -> TextLoad {
    let mut load = read_text(path);
    load.note = Some(why.to_string());
    load
}

fn blank_into(
    window: &gtk::ApplicationWindow,
    stack: &gtk::Stack,
    current: &Rc<RefCell<Option<OpenDoc>>>,
    title: &TitleWidget,
) {
    match XmlDoc::new_empty() {
        Ok(dp) => {
            let view = DocumentView::new(dp, PathBuf::from("novo.xml"));
            install_doc(
                OpenDoc::Table(view.clone()),
                window,
                stack,
                current,
                title,
                "novo.xml",
                true,
            );
            view.load_blank();
        }
        Err(e) => dialog::error(Some(window.upcast_ref()), &e.to_string()),
    }
}

/// Aplica o esquema de cores em GTK4 puro. Sem libadwaita, o app herda o tema
/// do sistema (e a cor de destaque dele). Só sobrepomos claro/escuro quando o
/// usuário escolhe explicitamente; em "system" deixamos o GTK seguir o desktop.
fn apply_theme(scheme: &str) {
    let Some(settings) = gtk::Settings::default() else {
        return;
    };
    match scheme {
        "dark" => settings.set_gtk_application_prefer_dark_theme(true),
        "light" => settings.set_gtk_application_prefer_dark_theme(false),
        _ => {} // system: não sobrepõe — o GTK segue o tema/portal do desktop
    }
}

/// Pede o novo atalho de `action`: a próxima tecla pressionada (com os
/// modificadores) vira o atalho. Esc cancela.
fn capture_shortcut(
    parent: &gtk::Window,
    action: &'static crate::shortcuts::Action,
    refresh: Rc<dyn Fn()>,
    note: gtk::Label,
) {
    let win = gtk::Window::builder()
        .title(tr("sc_change"))
        .transient_for(parent)
        .modal(true)
        .resizable(false)
        .default_width(380)
        .build();
    let root = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(10)
        .margin_top(18)
        .margin_bottom(18)
        .margin_start(18)
        .margin_end(18)
        .build();
    let heading = gtk::Label::builder()
        .label(format!("{} «{}»", tr("sc_press"), tr(action.label)))
        .wrap(true)
        .xalign(0.0)
        .build();
    heading.add_css_class("heading");
    root.append(&heading);
    let current = gtk::Label::builder()
        .label(format!("{}: {}", tr("sc_current"), {
            let l = crate::shortcuts::label(action.id);
            if l.is_empty() {
                tr("sc_none").to_string()
            } else {
                l
            }
        }))
        .xalign(0.0)
        .build();
    current.add_css_class("dim-label");
    root.append(&current);
    let hint = gtk::Label::builder()
        .label(tr("sc_press_hint"))
        .xalign(0.0)
        .build();
    hint.add_css_class("dim-label");
    root.append(&hint);

    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    buttons.set_halign(gtk::Align::End);
    buttons.set_margin_top(6);
    let none_btn = gtk::Button::with_label(tr("sc_none"));
    let default_btn = gtk::Button::with_label(tr("sc_default"));
    let cancel_btn = gtk::Button::with_label(tr("cancel"));
    for b in [&none_btn, &default_btn, &cancel_btn] {
        // os botões não pegam o foco: Enter e Espaço também podem virar atalho
        b.set_focus_on_click(false);
        b.set_can_focus(false);
        buttons.append(b);
    }
    root.append(&buttons);

    let done = {
        let (win, refresh) = (win.clone(), refresh.clone());
        move || {
            refresh();
            win.close();
        }
    };
    {
        let (done, note) = (done.clone(), note.clone());
        none_btn.connect_clicked(move |_| {
            crate::shortcuts::set(action.id, Vec::new());
            note.set_text("");
            done();
        });
    }
    {
        let (done, note) = (done.clone(), note.clone());
        default_btn.connect_clicked(move |_| {
            crate::shortcuts::reset(action.id);
            note.set_text("");
            done();
        });
    }
    {
        let win = win.clone();
        cancel_btn.connect_clicked(move |_| win.close());
    }

    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    {
        let win = win.clone();
        keys.connect_key_pressed(move |_, key, _, state| {
            if key == gtk::gdk::Key::Escape {
                win.close();
                return glib::Propagation::Stop;
            }
            let Some(accel) = crate::shortcuts::from_event(key, state) else {
                return glib::Propagation::Stop; // só o modificador, ainda
            };
            match crate::shortcuts::assign(action.id, &accel) {
                Some(other) => note.set_text(&format!("{} «{}».", tr("sc_moved"), tr(other.label))),
                None => note.set_text(""),
            }
            done();
            glib::Propagation::Stop
        });
    }
    win.add_controller(keys);
    win.set_child(Some(&root));
    win.present();
}

/// Janela de configurações FLUTUANTE com abas (Notebook), em GTK4 puro.
fn show_settings(parent: &gtk::ApplicationWindow) {
    let cfg = config::load();

    let win = gtk::Window::builder()
        .title(tr("settings"))
        .transient_for(parent)
        .modal(true)
        .resizable(false)
        .default_width(520)
        .default_height(460)
        .build();

    let notebook = gtk::Notebook::new();

    // ---------- Aba: Geral (idioma + tema) ----------
    let geral = settings_page();

    let lang_dd = gtk::DropDown::from_strings(&["Português", "English"]);
    lang_dd.set_selected(if cfg.lang.starts_with("en") { 1 } else { 0 });
    geral.append(&pref_row(tr("language"), &lang_dd));

    let theme_dd =
        gtk::DropDown::from_strings(&[tr("theme_system"), tr("theme_light"), tr("theme_dark")]);
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

    // ---------- Aba: Backup ----------
    let bk = settings_page();
    let spin = |min: f64, max: f64, val: u32| {
        let s = gtk::SpinButton::with_range(min, max, 1.0);
        s.set_value(f64::from(val));
        s
    };
    let keep = spin(1.0, 365.0, cfg.backup_keep_days);
    bk.append(&pref_row(tr("backup_keep_days"), &keep));
    let max_mb = spin(10.0, 100_000.0, cfg.backup_max_mb);
    bk.append(&pref_row(tr("backup_max_mb"), &max_mb));

    let bk_note = gtk::Label::builder()
        .label(tr("backup_note"))
        .wrap(true)
        .xalign(0.0)
        .margin_top(6)
        .build();
    bk_note.add_css_class("dim-label");
    bk.append(&bk_note);

    let open_dir = gtk::Button::builder()
        .label(tr("backup_open_folder"))
        .halign(gtk::Align::Start)
        .build();
    open_dir.connect_clicked(|b| {
        let dir = backup::root();
        let _ = std::fs::create_dir_all(&dir);
        let win = b.root().and_downcast::<gtk::Window>();
        gtk::FileLauncher::new(Some(&gio::File::for_path(dir))).launch(
            win.as_ref(),
            gio::Cancellable::NONE,
            |_| {},
        );
    });
    bk.append(&open_dir);

    let save_backup_cfg = {
        let (keep, max_mb) = (keep.clone(), max_mb.clone());
        move || {
            let mut c = config::load();
            c.backup_keep_days = keep.value() as u32;
            c.backup_max_mb = max_mb.value() as u32;
            config::save(&c);
        }
    };
    for s in [&keep, &max_mb] {
        let f = save_backup_cfg.clone();
        s.connect_value_changed(move |_| f());
    }

    notebook.append_page(&bk, Some(&gtk::Label::new(Some(tr("backup")))));

    // ---------- Aba: Atalhos ----------
    let sc = settings_page();
    let sc_list = gtk::ListBox::new();
    sc_list.set_selection_mode(gtk::SelectionMode::None);
    sc_list.add_css_class("frame");
    let sc_values: Rc<Vec<(&'static str, gtk::Label)>> = Rc::new(
        crate::shortcuts::ACTIONS
            .iter()
            .map(|a| (a.id, gtk::Label::builder().xalign(1.0).build()))
            .collect(),
    );
    let sc_note = gtk::Label::builder().wrap(true).xalign(0.0).build();
    sc_note.add_css_class("dim-label");
    let refresh_sc: Rc<dyn Fn()> = {
        let sc_values = sc_values.clone();
        Rc::new(move || {
            for (id, l) in sc_values.iter() {
                let text = crate::shortcuts::label(id);
                l.set_text(if text.is_empty() {
                    tr("sc_none")
                } else {
                    &text
                });
            }
        })
    };
    refresh_sc();
    for (action, (_, value)) in crate::shortcuts::ACTIONS.iter().zip(sc_values.iter()) {
        let line = gtk::Box::builder()
            .spacing(12)
            .margin_top(4)
            .margin_bottom(4)
            .margin_start(10)
            .margin_end(6)
            .build();
        let name = gtk::Label::builder()
            .label(tr(action.label))
            .xalign(0.0)
            .hexpand(true)
            .build();
        value.add_css_class("dim-label");
        let change = gtk::Button::with_label(tr("sc_change"));
        change.add_css_class("flat");
        {
            let (win, refresh_sc, sc_note) = (win.clone(), refresh_sc.clone(), sc_note.clone());
            change.connect_clicked(move |_| {
                capture_shortcut(&win, action, refresh_sc.clone(), sc_note.clone())
            });
        }
        line.append(&name);
        line.append(value);
        line.append(&change);
        sc_list.append(&line);
    }
    let sc_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .min_content_height(260)
        .vexpand(true)
        .child(&sc_list)
        .build();
    sc.append(&sc_scroll);
    let sc_hint = gtk::Label::builder()
        .label(tr("sc_hint"))
        .wrap(true)
        .xalign(0.0)
        .build();
    sc_hint.add_css_class("dim-label");
    sc.append(&sc_hint);
    sc.append(&sc_note);
    let sc_reset = gtk::Button::builder()
        .label(tr("sc_reset_all"))
        .halign(gtk::Align::Start)
        .build();
    {
        let (refresh_sc, sc_note) = (refresh_sc.clone(), sc_note.clone());
        sc_reset.connect_clicked(move |_| {
            crate::shortcuts::reset_all();
            sc_note.set_text("");
            refresh_sc();
        });
    }
    sc.append(&sc_reset);
    notebook.append_page(&sc, Some(&gtk::Label::new(Some(tr("shortcuts")))));

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
        gtk::UriLauncher::new(REPO_URL).launch(gtk::Window::NONE, gio::Cancellable::NONE, |_| {});
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
    let lbl = gtk::Label::builder()
        .label(label)
        .xalign(0.0)
        .hexpand(true)
        .build();
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
    let lbl = gtk::Label::builder()
        .label(label)
        .xalign(0.0)
        .hexpand(true)
        .build();
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
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::HANDLES_OPEN)
        .build();

    let theme = cfg.theme.clone();
    app.connect_startup(move |_| {
        // Declara o ícone padrão das janelas com o nome do app (= .desktop =
        // ícone instalado). Ajuda o ícone a aparecer na barra/dock, não só no
        // menu de apps.
        gtk::Window::set_default_icon_name(APP_ID);

        // Carrega CSS customizado para indicar a célula em foco na grade
        let provider = gtk::CssProvider::new();
        provider.load_from_string(
            "
            /* Destaque da linha selecionada (1º clique / botão direito /
               navegação por teclado): tinge a linha inteira. */
            columnview > listview > row:selected {
                background-color: alpha(@accent_bg_color, 0.12);
            }
            columnview > listview > row:selected cell {
                background-color: transparent;
            }
            /* Destaque mais forte na célula atual. Vem de uma classe, e não
               de :focus-within, para continuar visível com o menu de
               contexto aberto ou o foco em outro lugar da janela. */
            columnview > listview > row cell.cell-current,
            columnview > listview > row:selected cell.cell-current {
                outline: 2px solid @accent_bg_color;
                outline-offset: -2px;
                background-color: alpha(@accent_bg_color, 0.15);
            }
            /* Células selecionadas (Shift, Ctrl, arrasto): tom mais forte
               que o da linha selecionada. */
            columnview > listview > row cell.cell-selected,
            columnview > listview > row:selected cell.cell-selected {
                background-color: alpha(@accent_bg_color, 0.28);
            }
            /* Tabela de diferenças (recuperação e histórico de versões):
               linha incluída em verde, apagada em vermelho, alterada em
               amarelo com a célula que mudou mais forte. */
            columnview.diff-table > listview > row cell.diff-add {
                background-color: rgba(46, 160, 67, 0.22);
            }
            columnview.diff-table > listview > row cell.diff-del {
                background-color: rgba(220, 53, 69, 0.22);
            }
            columnview.diff-table > listview > row cell.diff-chg {
                background-color: rgba(210, 153, 34, 0.10);
            }
            columnview.diff-table > listview > row cell.diff-col-add {
                background-image: linear-gradient(rgba(46, 160, 67, 0.10), rgba(46, 160, 67, 0.10));
            }
            columnview.diff-table > listview > row cell.diff-col-del {
                background-image: linear-gradient(rgba(220, 53, 69, 0.10), rgba(220, 53, 69, 0.10));
            }
            columnview.diff-table > listview > row cell.diff-cell-chg {
                background-color: rgba(210, 153, 34, 0.38);
            }
            columnview.diff-table > listview > row cell.diff-gap {
                font-style: italic;
                color: alpha(currentColor, 0.6);
            }
            /* Limpa o foco nativo do EditableLabel para não
               duplicar com o contorno da célula. */
            columnview cell editablelabel:focus,
            columnview cell editablelabel text:focus {
                outline: none;
                box-shadow: none;
            }
            /* Autocompletar de colunas no campo de filtro. */
            popover.filter-suggestions-popover > contents {
                padding: 4px;
            }
            listbox.filter-suggestions {
                background: transparent;
            }
            listbox.filter-suggestions row {
                border-radius: 6px;
                padding: 0;
                margin: 1px 0;
            }
            listbox.filter-suggestions row:selected {
                background-color: alpha(@accent_bg_color, 0.18);
                color: @accent_color;
            }
            listbox.filter-suggestions row:hover {
                background-color: alpha(@accent_bg_color, 0.10);
            }
            /* Remove o contorno de foco quadrado desenhado ao navegar
               por teclado; o fundo arredondado do :selected já indica
               a linha ativa. */
            /* Sem o brilho persistente na borda direita da tabela (o de
               baixo, que indica mais linhas, continua). */
            scrolledwindow.data-scroller > overshoot.right,
            scrolledwindow.data-scroller > undershoot.right {
                background: none;
                box-shadow: none;
            }
            /* Camada de andamento sobre a tabela (operações longas). */
            .busy-shade {
                background-color: alpha(@window_bg_color, 0.6);
            }
            .busy-card {
                background-color: @window_bg_color;
                border-radius: 12px;
                padding: 20px 28px;
                box-shadow: 0 2px 12px alpha(black, 0.25);
            }
            listbox.recent-list {
                border-radius: 8px;
            }
            listbox.recent-list row {
                border-radius: 0;
            }
            listbox.recent-list row:first-child {
                border-top-left-radius: 8px;
                border-top-right-radius: 8px;
            }
            listbox.recent-list row:last-child {
                border-bottom-left-radius: 8px;
                border-bottom-right-radius: 8px;
            }
            listbox.filter-suggestions row:focus,
            listbox.filter-suggestions row:focus-visible,
            listbox.filter-suggestions row:focus-within {
                outline: none;
                box-shadow: none;
                border-radius: 6px;
            }
            ",
        );
        gtk::style_context_add_provider_for_display(
            &gtk::gdk::Display::default().expect("Não foi possível conectar ao display"),
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );

        // Limpeza dos backups ao iniciar e a cada hora, fora da thread da UI.
        let clean = || {
            let c = config::load();
            std::thread::spawn(move || backup::cleanup(c.backup_keep_days, c.backup_max_mb));
        };
        clean();
        glib::timeout_add_seconds_local(3600, move || {
            clean();
            glib::ControlFlow::Continue
        });

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
        offer_recovery(app, &window);
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
        if let Some(w) = app
            .active_window()
            .and_then(|w| w.downcast::<gtk::ApplicationWindow>().ok())
        {
            offer_recovery(app, &w);
        }
    });
    app.run()
}
