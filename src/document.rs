use crate::xmldoc::{Change, XmlDoc, QueryResult};
use crate::row_object::RowObject;
use crate::dialog;
use crate::replace::Matcher;
use crate::rowclip;
use crate::sort::{cmp_keys, sort_key, SortKey};
use crate::i18n::tr;
use gtk::prelude::*;
use gtk::glib::{self, clone};
use gtk::{gdk, gio};
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;

/// Estado compartilhado de um documento aberto.
struct State {
    dp: XmlDoc,
    columns: RefCell<Vec<String>>, // colunas visíveis (sem as de controle)
    editable: RefCell<bool>,
    dirty: RefCell<bool>,
    path: PathBuf,
    saved_path: RefCell<Option<PathBuf>>, // caminho em disco já conhecido (Salvar sobrescreve)
    last_filter: RefCell<String>, // último WHERE aplicado (para recarregar)
    // Geração da carga atual: cada recarga (abertura, filtro, SQL, reload)
    // incrementa este contador; um carregador em blocos em andamento verifica-o
    // e para assim que fica obsoleto (cancelamento de carga antiga).
    generation: Cell<u64>,
    // `true` enquanto o carregamento em blocos ainda não terminou — bloqueia
    // ações estruturais (inserir/mover/clonar/excluir) sobre um store parcial.
    loading: Cell<bool>,
    // Contador de alterações e o valor dele na última cópia de recuperação:
    // a cópia periódica só grava quando há algo novo.
    rev: Cell<u64>,
    backed_rev: Cell<u64>,
    // Ordenação da grade pelo cabeçalho: (coluna, crescente). `None` é a
    // ordem do documento. Só muda a exibição — o arquivo salvo mantém a
    // ordem das linhas.
    sort: RefCell<Option<(String, bool)>>,
    // Ordem original de um resultado SQL (somente leitura), guardada ao
    // ordená-lo, para o terceiro clique poder voltar a ela.
    unsorted: RefCell<Option<Vec<RowObject>>>,
}

/// Item do menu de contexto com o atalho configurado ao lado.
fn menu_item(label: &str, action: &str, shortcut: &str) -> gio::MenuItem {
    let item = gio::MenuItem::new(Some(tr(label)), Some(action));
    if let Some(accel) = crate::shortcuts::menu_accel(shortcut) {
        item.set_attribute_value("accel", Some(&accel.to_variant()));
    }
    item
}

/// Intervalo mínimo entre instantâneos do histórico durante a edição.
const SNAPSHOT_EVERY: std::time::Duration = std::time::Duration::from_secs(5 * 60);

pub struct DocumentView {
    pub root: gtk::Box,
    state: Rc<State>,
    store: gio::ListStore,
    selection: gtk::MultiSelection,
    // Linha do cursor (onde Localizar continua, alvo de Ctrl+V) e âncora do
    // Shift+clique/Shift+seta. Guardadas como objeto, e não posição, para
    // continuarem certas depois de inserções e exclusões acima delas.
    cursor: RefCell<Option<RowObject>>,
    // Célula atual (linha, coluna), destacada com a classe `cell-current`,
    // e o widget de célula que está com a classe agora.
    cur_cell: RefCell<Option<(RowObject, usize)>>,
    cur_widget: RefCell<Option<gtk::Widget>>,
    // Widget de cada célula → seu ListItem, para achar a linha sob o
    // ponteiro ao arrastar a seleção.
    cell_items: RefCell<std::collections::HashMap<gtk::Widget, (glib::WeakRef<gtk::ListItem>, usize)>>,
    // Seleção de células (retângulos de linhas × colunas), para limpar o
    // conteúdo com Backspace. Copiar/colar continuam agindo sobre linhas.
    cell_rects: RefCell<Vec<CellRect>>,
    // Coluna da âncora do Shift (a linha da âncora é `anchor`).
    anchor_col: Cell<usize>,

    anchor: RefCell<Option<RowObject>>,
    // Seção do menu de contexto com "Colar", preenchida só quando a área de
    // transferência tem linhas.
    paste_section: gio::Menu,
    sel_label: gtk::Label,
    colview: gtk::ColumnView,
    status: gtk::Label,
    find_entry: gtk::SearchEntry,
    toolbar: gtk::Box,
    replace_revealer: gtk::Revealer,
    // Camada de "trabalhando…" sobre a tabela durante operações longas.
    busy: Cell<bool>,
    // Cópia de recuperação deste documento (ver `backup`).
    recovery: crate::backup::Recovery,
    backup_writing: Cell<bool>,
    // Geração da cópia agendada: cada alteração adia a gravação, para uma
    // sequência de edições virar uma gravação só.
    backup_gen: Cell<u64>,
    // Referência fraca a si mesmo, para agendar a cópia a partir de `&self`.
    weak_self: RefCell<std::rc::Weak<Self>>,
    // Último instantâneo do histórico de versões tirado durante a edição.
    last_snapshot: Cell<std::time::Instant>,
    // Quem troca o documento da janela por uma versão restaurada (`app`).
    restore_handler: RefCell<Option<Rc<dyn Fn(crate::backup::Version)>>>,
    busy_shade: gtk::Box,
    busy_spinner: gtk::Spinner,
    busy_label: gtk::Label,
    busy_bar: gtk::ProgressBar,
    // Barra de Substituir (Ctrl+H), abaixo da barra de ferramentas.
    replace_toggle: gtk::ToggleButton,
    replace_entry: gtk::Entry,
    // Escopo: "Todas as colunas" (posição 0) seguido das colunas visíveis.
    scope_list: gtk::StringList,
    scope_dd: gtk::DropDown,
    case_chk: gtk::CheckButton,
    whole_chk: gtk::CheckButton,
    undo_btn: gtk::Button,
    redo_btn: gtk::Button,
    menu: gtk::PopoverMenu,
    ctx_rowid: Cell<i64>,
    ctx_col: Cell<usize>,
    // Geração da busca com debounce: cada tecla incrementa; o timeout só executa
    // a busca se sua geração ainda for a atual (evita varredura por tecla).
    search_gen: Cell<u64>,
    // Títulos do cabeçalho, na ordem das colunas — âncora do campo de
    // renomear e do menu de coluna aberto pelo cabeçalho.
    titles: RefCell<Vec<gtk::Widget>>,
    // Menu do botão direito no cabeçalho: o mesmo submenu "Coluna" das células.
    header_menu: gtk::PopoverMenu,
}

impl DocumentView {
    pub fn new(dp: XmlDoc, path: PathBuf) -> Rc<Self> {
        let state = Rc::new(State {
            dp,
            columns: RefCell::new(Vec::new()),
            editable: RefCell::new(true),
            dirty: RefCell::new(false),
            // Arquivos abertos do disco vêm com caminho absoluto; um documento
            // novo ("novo.xml") vem com caminho relativo e ainda não tem destino.
            saved_path: RefCell::new(if path.is_absolute() { Some(path.clone()) } else { None }),
            path,
            last_filter: RefCell::new(String::new()),
            generation: Cell::new(0),
            loading: Cell::new(false),
            rev: Cell::new(0),
            backed_rev: Cell::new(0),
            sort: RefCell::new(None),
            unsorted: RefCell::new(None),
        });

        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);

        // ---------- barra de ferramentas ----------
        let bar = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(6)
            .margin_top(6)
            .margin_bottom(6)
            .margin_start(6)
            .margin_end(6)
            .build();

        let undo_btn = gtk::Button::from_icon_name("edit-undo-symbolic");
        undo_btn.set_tooltip_text(Some(tr("undo_tooltip")));
        undo_btn.set_sensitive(false);
        bar.append(&undo_btn);

        let history_btn = gtk::Button::from_icon_name("document-open-recent-symbolic");
        history_btn.set_tooltip_text(Some(tr("history_tooltip")));
        bar.append(&history_btn);

        let redo_btn = gtk::Button::from_icon_name("edit-redo-symbolic");
        redo_btn.set_tooltip_text(Some(tr("redo_tooltip")));
        redo_btn.set_sensitive(false);
        bar.append(&redo_btn);

        let find_entry = gtk::SearchEntry::builder()
            .placeholder_text(tr("find_placeholder"))
            .width_chars(18)
            .build();
        bar.append(&find_entry);

        let replace_toggle = gtk::ToggleButton::builder()
            .icon_name("edit-find-replace-symbolic")
            .tooltip_text(tr("replace_tooltip"))
            .build();
        bar.append(&replace_toggle);

        let filter_entry = gtk::Entry::builder()
            .placeholder_text(tr("filter_placeholder"))
            .hexpand(true)
            .build();
        bar.append(&filter_entry);

        let filter_btn = gtk::Button::from_icon_name("system-search-symbolic");
        filter_btn.set_tooltip_text(Some(tr("filter_tooltip")));
        bar.append(&filter_btn);

        let sql_btn = gtk::Button::with_label("SQL…");
        sql_btn.set_tooltip_text(Some(tr("sql_tooltip")));
        bar.append(&sql_btn);

        let sum_btn = gtk::Button::with_label(tr("sum_label"));
        sum_btn.set_tooltip_text(Some(tr("sum_tooltip")));
        bar.append(&sum_btn);

        let csv_btn = gtk::Button::from_icon_name("x-office-spreadsheet-symbolic");
        csv_btn.set_tooltip_text(Some(tr("csv_tooltip")));
        bar.append(&csv_btn);

        // Ícone de disquete: padrão universal de "salvar".
        let save_btn = gtk::Button::from_icon_name("media-floppy-symbolic");
        save_btn.set_tooltip_text(Some(tr("save_tooltip")));
        bar.append(&save_btn);

        root.append(&bar);

        // ---------- barra de substituir ----------
        let rbar = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(6)
            .margin_bottom(6)
            .margin_start(6)
            .margin_end(6)
            .build();
        let replace_entry = gtk::Entry::builder()
            .placeholder_text(tr("replace_placeholder"))
            .width_chars(18)
            .build();
        rbar.append(&replace_entry);
        let scope_list = gtk::StringList::new(&[tr("all_columns")]);
        let scope_dd = gtk::DropDown::builder().model(&scope_list).build();
        scope_dd.set_tooltip_text(Some(tr("replace_scope_tooltip")));
        rbar.append(&scope_dd);
        let case_chk = gtk::CheckButton::with_label(tr("match_case"));
        rbar.append(&case_chk);
        let whole_chk = gtk::CheckButton::with_label(tr("whole_cell"));
        whole_chk.set_tooltip_text(Some(tr("whole_cell_tooltip")));
        rbar.append(&whole_chk);
        let spacer = gtk::Box::builder().hexpand(true).build();
        rbar.append(&spacer);
        let replace_btn = gtk::Button::with_label(tr("replace"));
        rbar.append(&replace_btn);
        let replace_all_btn = gtk::Button::with_label(tr("replace_all"));
        rbar.append(&replace_all_btn);
        let replace_close = gtk::Button::from_icon_name("window-close-symbolic");
        replace_close.add_css_class("flat");
        rbar.append(&replace_close);
        let replace_revealer = gtk::Revealer::builder().child(&rbar).build();
        replace_toggle
            .bind_property("active", &replace_revealer, "reveal-child")
            .sync_create()
            .build();
        root.append(&replace_revealer);

        // ---------- tabela ----------
        let store = gio::ListStore::new::<RowObject>();
        let selection = gtk::MultiSelection::new(Some(store.clone()));
        let colview = gtk::ColumnView::builder()
            .model(&selection)
            .show_row_separators(true)
            .show_column_separators(true)
            .vexpand(true)
            .build();
        colview.add_css_class("data-table");

        let scroller = gtk::ScrolledWindow::builder()
            .vexpand(true)
            // Barras ocupam espaço próprio (não flutuam sobre o conteúdo),
            // evitando que a barra horizontal cubra a última linha.
            .overlay_scrolling(false)
            .build();
        scroller.add_css_class("data-scroller");
        scroller.set_child(Some(&colview));

        // Camada sobre a tabela para operações longas: escurece a grade,
        // bloqueia cliques e mostra o andamento em vez de congelar a janela.
        let busy_shade = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .visible(false)
            .build();
        busy_shade.add_css_class("busy-shade");
        let card = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(10)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .vexpand(true)
            .build();
        card.add_css_class("busy-card");
        let busy_spinner = gtk::Spinner::new();
        busy_spinner.set_size_request(32, 32);
        card.append(&busy_spinner);
        let busy_label = gtk::Label::new(None);
        busy_label.add_css_class("heading");
        card.append(&busy_label);
        let busy_bar = gtk::ProgressBar::builder().show_text(true).width_request(280).build();
        card.append(&busy_bar);
        busy_shade.append(&card);

        let overlay = gtk::Overlay::builder().vexpand(true).child(&scroller).build();
        overlay.add_overlay(&busy_shade);
        root.append(&overlay);

        // ---------- rodapé ----------
        let status = gtk::Label::builder()
            .xalign(0.0)
            .margin_start(8)
            .margin_top(4)
            .margin_bottom(4)
            .build();
        status.add_css_class("dim-label");
        status.set_hexpand(true);
        let sel_label = gtk::Label::builder().margin_end(8).build();
        sel_label.add_css_class("dim-label");
        let footer = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        footer.append(&status);
        footer.append(&sel_label);
        root.append(&footer);

        // ---------- menu de contexto (botão direito) ----------
        let menu_model = gio::Menu::new();

        // "Colar": só aparece quando há linhas copiadas (ver `popup_ctx_menu`).
        let paste_section = gio::Menu::new();
        menu_model.append_section(None, &paste_section);

        // Submenu "Linha": copiar / inserir / clonar / mover / excluir
        let rows_menu = gio::Menu::new();
        let sec_copy = gio::Menu::new();
        sec_copy.append_item(&menu_item("ctx_row_copy", "doc.row-copy", "copy"));
        rows_menu.append_section(None, &sec_copy);
        let sec_insert = gio::Menu::new();
        sec_insert.append_item(&menu_item("ctx_row_above", "doc.row-above", "row_above"));
        sec_insert.append_item(&menu_item("ctx_row_below", "doc.row-below", "row_below"));
        rows_menu.append_section(None, &sec_insert);
        let sec_clone = gio::Menu::new();
        sec_clone.append(Some(tr("ctx_row_clone_above")), Some("doc.row-clone-above"));
        sec_clone.append_item(&menu_item("ctx_row_clone_below", "doc.row-clone-below", "row_clone"));
        rows_menu.append_section(None, &sec_clone);
        let sec_move = gio::Menu::new();
        sec_move.append_item(&menu_item("ctx_row_move_up", "doc.row-move-up", "move_up"));
        sec_move.append_item(&menu_item("ctx_row_move_down", "doc.row-move-down", "move_down"));
        rows_menu.append_section(None, &sec_move);
        let sec_row_del = gio::Menu::new();
        sec_row_del.append_item(&menu_item("ctx_row_delete", "doc.row-delete", "delete_rows"));
        rows_menu.append_section(None, &sec_row_del);
        menu_model.append_submenu(Some(tr("ctx_row_menu")), &rows_menu);

        // Submenu "Coluna": somar / adicionar / excluir
        let cols_menu = gio::Menu::new();
        cols_menu.append(Some(tr("ctx_col_sum")), Some("doc.col-sum"));
        cols_menu.append(Some(tr("ctx_col_rename")), Some("doc.col-rename"));
        cols_menu.append(Some(tr("ctx_col_add")), Some("doc.col-add"));
        cols_menu.append(Some(tr("ctx_col_delete")), Some("doc.col-delete"));
        menu_model.append_submenu(Some(tr("ctx_col_menu")), &cols_menu);

        // Atalho em destaque, fora dos submenus: Somar coluna.
        let sec_quick = gio::Menu::new();
        sec_quick.append_item(&menu_item("ctx_cells_clear", "doc.cells-clear", "clear_cells"));
        sec_quick.append(Some(tr("ctx_col_sum")), Some("doc.col-sum"));
        menu_model.append_section(None, &sec_quick);

        // NESTED: cada submenu abre em seu próprio popover, dimensionado ao
        // conteúdo (evita o scroll do modo deslizante, que não recalcula altura).
        let menu = gtk::PopoverMenu::from_model_full(&menu_model, gtk::PopoverMenuFlags::NESTED);
        menu.set_parent(&colview);
        menu.set_has_arrow(false);
        // Sem autohide: um popover com autohide consome o clique que o fecha,
        // e clicar em outra célula exigiria um segundo clique. O fechamento
        // por clique fora é feito por `close_menus_on_outside_click`.
        menu.set_autohide(false);
        menu.set_halign(gtk::Align::Start);

        // Menu do cabeçalho: ações da coluna e, numa seção à parte, Somar —
        // o mesmo arranjo do menu das células.
        let header_model = gio::Menu::new();
        let sec_col = gio::Menu::new();
        sec_col.append(Some(tr("ctx_col_rename")), Some("doc.col-rename"));
        sec_col.append(Some(tr("ctx_col_add")), Some("doc.col-add"));
        sec_col.append(Some(tr("ctx_col_delete")), Some("doc.col-delete"));
        header_model.append_section(None, &sec_col);
        let sec_sum = gio::Menu::new();
        sec_sum.append(Some(tr("ctx_col_sum")), Some("doc.col-sum"));
        header_model.append_section(None, &sec_sum);
        let header_menu = gtk::PopoverMenu::from_model(Some(&header_model));
        header_menu.set_parent(&colview);
        header_menu.set_has_arrow(false);
        header_menu.set_autohide(false);
        header_menu.set_halign(gtk::Align::Start);

        let me = Rc::new(Self {
            root,
            state,
            store,
            selection,
            cursor: RefCell::new(None),
            cur_cell: RefCell::new(None),
            cur_widget: RefCell::new(None),
            cell_items: RefCell::default(),
            cell_rects: RefCell::default(),
            anchor_col: Cell::new(0),
            anchor: RefCell::new(None),
            paste_section,
            sel_label,
            colview,
            status,
            find_entry: find_entry.clone(),
            toolbar: bar.clone(),
            replace_revealer: replace_revealer.clone(),
            busy: Cell::new(false),
            recovery: crate::backup::Recovery::new(),
            backup_writing: Cell::new(false),
            backup_gen: Cell::new(0),
            weak_self: RefCell::new(std::rc::Weak::new()),
            last_snapshot: Cell::new(std::time::Instant::now()),
            restore_handler: RefCell::new(None),
            busy_shade,
            busy_spinner,
            busy_label,
            busy_bar,
            replace_toggle: replace_toggle.clone(),
            replace_entry: replace_entry.clone(),
            scope_list,
            scope_dd,
            case_chk,
            whole_chk,
            undo_btn: undo_btn.clone(),
            redo_btn: redo_btn.clone(),
            menu,
            ctx_rowid: Cell::new(-1),
            ctx_col: Cell::new(0),
            search_gen: Cell::new(0),
            titles: RefCell::new(Vec::new()),
            header_menu,
        });

        // ações do menu de contexto
        let actions = gio::SimpleActionGroup::new();
        let mk = |name: &str, f: Box<dyn Fn()>| {
            let a = gio::SimpleAction::new(name, None);
            a.connect_activate(move |_, _| f());
            a
        };
        actions.add_action(&mk("row-above", Box::new(clone!(@strong me => move || me.ctx_insert_row(true)))));
        actions.add_action(&mk("row-below", Box::new(clone!(@strong me => move || me.ctx_insert_row(false)))));
        actions.add_action(&mk("cells-clear", Box::new(clone!(@strong me => move || me.clear_selected_cells()))));
        actions.add_action(&mk("row-copy", Box::new(clone!(@strong me => move || { me.copy_rows(); }))));
        actions.add_action(&mk("paste-above", Box::new(clone!(@strong me => move || me.paste_rows(Some(me.ctx_rowid.get()), true)))));
        actions.add_action(&mk("paste-below", Box::new(clone!(@strong me => move || me.paste_rows(Some(me.ctx_rowid.get()), false)))));
        actions.add_action(&mk("row-delete", Box::new(clone!(@strong me => move || me.ctx_delete_row()))));
        actions.add_action(&mk("row-clone-above", Box::new(clone!(@strong me => move || me.ctx_clone_row(true)))));
        actions.add_action(&mk("row-clone-below", Box::new(clone!(@strong me => move || me.ctx_clone_row(false)))));
        actions.add_action(&mk("row-move-up", Box::new(clone!(@strong me => move || me.ctx_move_row(true)))));
        actions.add_action(&mk("row-move-down", Box::new(clone!(@strong me => move || me.ctx_move_row(false)))));
        actions.add_action(&mk("col-sum", Box::new(clone!(@strong me => move || me.ctx_sum_column()))));
        actions.add_action(&mk("col-rename", Box::new(clone!(@strong me => move || me.ctx_rename_column()))));
        actions.add_action(&mk("col-add", Box::new(clone!(@strong me => move || me.ctx_add_column()))));
        actions.add_action(&mk("col-delete", Box::new(clone!(@strong me => move || me.ctx_delete_column()))));
        me.root.insert_action_group("doc", Some(&actions));

        // ---------- conexões ----------
        let apply_filter = clone!(@strong me, @weak filter_entry => move || {
            me.apply_filter(&filter_entry.text());
        });
        filter_btn.connect_clicked(clone!(@strong apply_filter => move |_| apply_filter()));
        filter_entry.connect_activate(clone!(@strong apply_filter => move |_| apply_filter()));

        me.arm_filter_autocomplete(&filter_entry);

        // Teclado do documento: Esc fecha menus; o resto são os atalhos
        // configuráveis.
        let save_keys = gtk::EventControllerKey::new();
        save_keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let me_keys = me.clone();
        save_keys.connect_key_pressed(move |_, keyval, _code, state| {
            // Durante uma operação longa o teclado fica bloqueado: salvar ou
            // desfazer no meio dela deixaria o documento pela metade.
            // Esc fecha o menu de contexto (sem autohide ele não fecha sozinho
            // quando o foco do teclado continua na tabela).
            if keyval == gdk::Key::Escape && (me_keys.menu.is_visible() || me_keys.header_menu.is_visible()) {
                close_menu(me_keys.menu.upcast_ref());
                close_menu(me_keys.header_menu.upcast_ref());
                return glib::Propagation::Stop;
            }
            if me_keys.busy.get() {
                return glib::Propagation::Stop;
            }
            // Atalhos configuráveis (ver `shortcuts`). Os que agem sobre a
            // tabela, e os de tecla sem Ctrl/Alt, ficam com o campo de texto
            // quando há um em foco (célula em edição, filtro, localizar),
            // que tem seu próprio copiar, desfazer, apagar.
            if let Some(action) = crate::shortcuts::find(keyval, state) {
                let plain = !state.intersects(
                    gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::ALT_MASK | gdk::ModifierType::SUPER_MASK,
                );
                if (action.grid || plain) && me_keys.text_has_focus() {
                    return glib::Propagation::Proceed;
                }
                me_keys.run_shortcut(action.id);
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        me.root.add_controller(save_keys);

        undo_btn.connect_clicked(clone!(@strong me => move |_| me.do_undo()));
        me.selection.connect_selection_changed(clone!(@weak me => move |_, _, _| {
            let n = me.selection.selection().size();
            me.sel_label.set_text(&if n > 1 { format!("{n} {}", tr("rows_selected")) } else { String::new() });
        }));
        replace_toggle.connect_toggled(clone!(@strong me => move |t| {
            if t.is_active() {
                me.focus_replace_bar();
            }
        }));
        replace_close.connect_clicked(clone!(@strong me => move |_| me.replace_toggle.set_active(false)));
        replace_btn.connect_clicked(clone!(@strong me => move |_| me.replace_one()));
        replace_all_btn.connect_clicked(clone!(@strong me => move |_| me.replace_all()));
        replace_entry.connect_activate(clone!(@strong me => move |_| me.replace_one()));
        // Esc na barra de Substituir fecha a barra.
        let esc = gtk::EventControllerKey::new();
        esc.connect_key_pressed(clone!(@strong me => @default-return glib::Propagation::Proceed, move |_, keyval, _, _| {
            if keyval == gdk::Key::Escape {
                me.replace_toggle.set_active(false);
                me.find_entry.grab_focus();
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        }));
        replace_revealer.add_controller(esc);
        redo_btn.connect_clicked(clone!(@strong me => move |_| me.do_redo()));
        sql_btn.connect_clicked(clone!(@strong me => move |_| me.open_sql_dialog()));
        sum_btn.connect_clicked(clone!(@strong me => move |_| me.do_sum()));
        save_btn.connect_clicked(clone!(@strong me => move |_| me.do_save()));
        history_btn.connect_clicked(clone!(@strong me => move |_| me.show_history()));
        csv_btn.connect_clicked(clone!(@strong me => move |_| me.do_export_csv()));

        // Busca com debounce: cada tecla agenda a busca ~250 ms depois; teclas
        // subsequentes invalidam o agendamento anterior (via geração). Assim uma
        // varredura O(linhas×colunas) não roda a cada tecla numa tabela grande.
        find_entry.connect_search_changed(clone!(@strong me => move |e| {
            let text = e.text().to_string();
            if text.is_empty() {
                return; // apagar o texto não procura células vazias
            }
            let g = me.search_gen.get().wrapping_add(1);
            me.search_gen.set(g);
            let me2 = me.clone();
            glib::timeout_add_local_once(std::time::Duration::from_millis(250), move || {
                if me2.search_gen.get() == g {
                    me2.find_next(&text);
                }
            });
        }));
        // Enter busca imediatamente (sem esperar o debounce).
        find_entry.connect_activate(clone!(@strong me => move |e| {
            me.search_gen.set(me.search_gen.get().wrapping_add(1));
            me.find_next(&e.text());
        }));

        *me.weak_self.borrow_mut() = Rc::downgrade(&me);
        me.close_menus_on_outside_click();
        me.arm_drag_select();
        let weak = Rc::downgrade(&me);
        me.store.connect_items_changed(move |_, _, _, _| {
            if let Some(me) = weak.upgrade() {
                me.on_store_changed();
            }
        });

        // A carga inicial (completa) é disparada por quem cria a view
        // (`load_initial`/`load_blank`), fora da thread de UI e em blocos.
        me
    }

    /// Carrega o resultado inicial (já consultado em segundo plano) em blocos,
    /// chamando `on_done` quando a última linha entra no store.
    pub fn load_initial(self: &Rc<Self>, initial: QueryResult, on_done: impl Fn() + 'static) {
        *self.state.editable.borrow_mut() = true;
        *self.state.last_filter.borrow_mut() = String::new();
        let n = initial.rows.len();
        self.status.set_text(&format!("Total: {n}"));
        self.load_result_chunked(initial, true, Some(Box::new(on_done)));
    }

    /// Carga de um documento em branco/novo (tabela vazia ou pequena).
    pub fn load_blank(self: &Rc<Self>) {
        self.apply_filter("");
    }

    fn apply_filter(self: &Rc<Self>, where_clause: &str) {
        match self.state.dp.filter(Some(where_clause)) {
            Ok(res) => {
                *self.state.editable.borrow_mut() = true;
                *self.state.last_filter.borrow_mut() = where_clause.to_string();
                let n = res.rows.len();
                let extra = if where_clause.trim().is_empty() {
                    String::new()
                } else {
                    format!("  ·  filtro: {where_clause}")
                };
                self.status.set_text(&format!("Total: {n}{extra}"));
                self.load_result_chunked(res, true, None);
                self.refresh_history_buttons();
            }
            Err(e) => self.error(&format!("{}\n{e}", tr("filter_error"))),
        }
    }

    /// Arma o autocompletar de nomes de coluna no campo de filtro (WHERE).
    /// Enquanto o usuário digita, sugere colunas cujo nome começa com a
    /// palavra atual; Tab/Enter/clique completam a palavra em foco.
    fn arm_filter_autocomplete(self: &Rc<Self>, entry: &gtk::Entry) {
        let listbox = gtk::ListBox::new();
        listbox.set_selection_mode(gtk::SelectionMode::Single);
        listbox.add_css_class("filter-suggestions");

        let popover = gtk::Popover::builder()
            .autohide(false)          // mantém o foco no campo de texto
            .has_arrow(false)
            .position(gtk::PositionType::Bottom)
            .halign(gtk::Align::Start)
            .build();
        popover.set_child(Some(&listbox));
        popover.set_parent(entry);
        popover.add_css_class("filter-suggestions-popover");

        // Preenche a lista com as colunas que casam com o prefixo digitado.
        // Retorna true se há sugestões visíveis.
        let refresh = {
            let me = self.clone();
            let entry = entry.clone();
            let listbox = listbox.clone();
            let popover = popover.clone();
            move || -> bool {
                while let Some(row) = listbox.row_at_index(0) {
                    listbox.remove(&row);
                }
                let text = entry.text().to_string();
                let cursor = entry.position().max(0) as usize;
                let (start, _end, word) = current_word(&text, cursor);
                // só sugere ao digitar o começo de uma palavra (não no meio)
                let typed = word.chars().take(cursor.saturating_sub(start)).collect::<String>();
                if typed.trim().is_empty() {
                    popover.popdown();
                    return false;
                }
                let needle = typed.to_lowercase();
                let cols = me.state.columns.borrow();
                let mut shown = 0;
                for col in cols.iter() {
                    if me.state.dp.is_internal_col(col) {
                        continue;
                    }
                    if col.to_lowercase().starts_with(&needle) && col.to_lowercase() != needle {
                        let label = gtk::Label::builder()
                            .label(col)
                            .xalign(0.0)
                            .margin_start(8)
                            .margin_end(8)
                            .margin_top(3)
                            .margin_bottom(3)
                            .build();
                        let row = gtk::ListBoxRow::new();
                        row.set_child(Some(&label));
                        listbox.append(&row);
                        shown += 1;
                        if shown >= 8 {
                            break;
                        }
                    }
                }
                if shown == 0 {
                    popover.popdown();
                    return false;
                }
                if let Some(first) = listbox.row_at_index(0) {
                    listbox.select_row(Some(&first));
                }
                popover.popup();
                true
            }
        };

        // Completa a palavra atual com o texto da linha selecionada (ou a `row`
        // informada), acrescentando um espaço, e fecha o popover.
        let complete = {
            let entry = entry.clone();
            let listbox = listbox.clone();
            let popover = popover.clone();
            move |chosen: Option<gtk::ListBoxRow>| {
                let row = chosen.or_else(|| listbox.selected_row());
                let Some(row) = row else { return };
                let name = row
                    .child()
                    .and_then(|c| c.downcast::<gtk::Label>().ok())
                    .map(|l| l.text().to_string());
                let Some(name) = name else { return };

                let text = entry.text().to_string();
                let cursor = entry.position().max(0) as usize;
                let (start, end, _word) = current_word(&text, cursor);
                let insert = format!("{} ", quote_ident(&name));
                entry.delete_text(start as i32, end as i32);
                let mut pos = start as i32;
                entry.insert_text(&insert, &mut pos);
                entry.set_position(pos);
                popover.popdown();
            }
        };

        entry.connect_changed(clone!(@strong refresh => move |_| { refresh(); }));

        // Tab/Enter completam; setas navegam; Esc fecha.
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let listbox_k = listbox.clone();
        let popover_k = popover.clone();
        keys.connect_key_pressed(move |_, keyval, _code, _state| {
            let open = popover_k.is_visible() && listbox_k.row_at_index(0).is_some();
            match keyval {
                gdk::Key::Tab if open => {
                    complete(None);
                    glib::Propagation::Stop
                }
                gdk::Key::Return | gdk::Key::KP_Enter if open => {
                    complete(None);
                    glib::Propagation::Stop
                }
                gdk::Key::Escape if open => {
                    popover_k.popdown();
                    glib::Propagation::Stop
                }
                gdk::Key::Down if open => {
                    move_selection(&listbox_k, 1);
                    glib::Propagation::Stop
                }
                gdk::Key::Up if open => {
                    move_selection(&listbox_k, -1);
                    glib::Propagation::Stop
                }
                _ => glib::Propagation::Proceed,
            }
        });
        entry.add_controller(keys);

        // clique numa sugestão também completa
        let complete_click = {
            let listbox = listbox.clone();
            let entry = entry.clone();
            let popover = popover.clone();
            move |row: &gtk::ListBoxRow| {
                let name = row
                    .child()
                    .and_then(|c| c.downcast::<gtk::Label>().ok())
                    .map(|l| l.text().to_string());
                let Some(name) = name else { return };
                let text = entry.text().to_string();
                let cursor = entry.position().max(0) as usize;
                let (start, end, _word) = current_word(&text, cursor);
                let insert = format!("{} ", quote_ident(&name));
                entry.delete_text(start as i32, end as i32);
                let mut pos = start as i32;
                entry.insert_text(&insert, &mut pos);
                entry.set_position(pos);
                popover.popdown();
                entry.grab_focus();
                entry.set_position(pos);
                let _ = &listbox;
            }
        };
        listbox.connect_row_activated(move |_, row| complete_click(row));
    }

    fn run_sql(self: &Rc<Self>, sql: &str) {
        match self.state.dp.query(sql) {
            Ok(res) => {
                *self.state.editable.borrow_mut() = false;
                let n = res.rows.len();
                self.status
                    .set_text(&format!("Total: {n}  ·  SQL (somente leitura): {sql}"));
                self.load_result_chunked(res, false, None);
                self.refresh_history_buttons();
            }
            Err(e) => self.error(&format!("{}\n{e}", tr("sql_error"))),
        }
    }

    /// Substitui o conteúdo da tabela pelos dados de `res`, construindo os
    /// `RowObject` em BLOCOS via `idle_add_local` — assim o laço principal do
    /// GTK processa entrada/redesenho entre os blocos e o teclado não congela
    /// (crítico sob XWayland). Ao terminar chama `on_done` (usado na abertura
    /// para revelar a tabela só quando ela está pronta).
    fn load_result_chunked(
        self: &Rc<Self>,
        res: QueryResult,
        editable: bool,
        on_done: Option<Box<dyn Fn()>>,
    ) {
        // separa as colunas de controle das visíveis
        let rid_name = self.state.dp.rid_alias().to_string();
        let rid_idx = res.columns.iter().position(|c| *c == rid_name);
        let vis_idx: Vec<usize> = res
            .columns
            .iter()
            .enumerate()
            .filter(|(_, c)| !self.state.dp.is_internal_col(c))
            .map(|(i, _)| i)
            .collect();
        let vis_names: Vec<String> = vis_idx.iter().map(|&i| res.columns[i].clone()).collect();

        // Um resultado SQL traz a própria ordem (ORDER BY): começa sem
        // ordenação de cabeçalho. A visão editável mantém a ordenação ativa.
        *self.state.unsorted.borrow_mut() = None;
        if !editable {
            *self.state.sort.borrow_mut() = None;
        }
        *self.state.columns.borrow_mut() = vis_names.clone();
        self.rebuild_columns(&vis_names);

        // Nova geração: cancela qualquer carregador anterior ainda em voo e
        // limpa o store (libera os RowObjects antigos).
        let generation = self.state.generation.get().wrapping_add(1);
        self.state.generation.set(generation);
        self.state.loading.set(true);
        self.store.remove_all();

        // Invertemos uma vez (O(n)); depois consumimos com `pop()` (O(1) por
        // linha) para liberar memória progressivamente sem custo quadrático.
        let mut rows = res.rows;
        let sort_col = self.state.sort.borrow().as_ref().and_then(|(name, asc)| {
            vis_names.iter().position(|n| n == name).map(|p| (vis_idx[p], *asc))
        });
        if let Some((ci, asc)) = sort_col {
            let mut keyed: Vec<(SortKey, Vec<String>)> =
                rows.into_iter().map(|r| (sort_key(&r[ci]), r)).collect();
            keyed.sort_by(|a, b| cmp_keys(&a.0, &b.0, asc));
            rows = keyed.into_iter().map(|(_, r)| r).collect();
        }
        rows.reverse();

        let store = self.store.clone();
        let weak = Rc::downgrade(self);
        glib::idle_add_local(move || {
            const CHUNK: usize = 2000;
            let Some(me) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            if me.state.generation.get() != generation {
                return glib::ControlFlow::Break; // carga obsoleta → aborta
            }
            let mut chunk: Vec<RowObject> = Vec::with_capacity(CHUNK.min(rows.len()));
            for _ in 0..CHUNK {
                let Some(mut row) = rows.pop() else { break };
                let rowid = rid_idx
                    .and_then(|i| row.get(i))
                    .and_then(|s| s.parse::<i64>().ok())
                    .unwrap_or(-1);
                // move os valores visíveis para fora da linha (sem clonar o heap)
                let values: Vec<String> =
                    vis_idx.iter().map(|&i| std::mem::take(&mut row[i])).collect();
                chunk.push(RowObject::new(rowid, values, editable));
            }
            if !chunk.is_empty() {
                store.splice(store.n_items(), 0, &chunk);
            }
            if rows.is_empty() {
                me.state.loading.set(false);
                if let Some(cb) = &on_done {
                    cb();
                }
                glib::ControlFlow::Break
            } else {
                glib::ControlFlow::Continue
            }
        });
    }

    /// Libera os recursos pesados deste documento (chamado ao trocar de arquivo):
    /// cancela um carregamento em voo, esvazia o store (solta todos os
    /// `RowObject`) e remove as colunas. Isso garante que a tabela anterior não
    /// permaneça em memória mesmo que o shell da view seja mantido vivo por
    /// algum handler — evitando o acúmulo que levava ao OOM/"AppRun.wrapped".
    pub fn teardown(&self) {
        self.state.generation.set(self.state.generation.get().wrapping_add(1));
        self.state.loading.set(false);
        self.store.remove_all();
        let cols = self.colview.columns();
        for i in (0..cols.n_items()).rev() {
            if let Some(obj) = cols.item(i) {
                self.colview
                    .remove_column(&obj.downcast::<gtk::ColumnViewColumn>().unwrap());
            }
        }
    }

    fn rebuild_columns(self: &Rc<Self>, names: &[String]) {
        // remove colunas atuais
        let existing = self.colview.columns();
        for i in (0..existing.n_items()).rev() {
            if let Some(obj) = existing.item(i) {
                let col = obj.downcast::<gtk::ColumnViewColumn>().unwrap();
                self.colview.remove_column(&col);
            }
        }

        for (idx, name) in names.iter().enumerate() {
            let factory = gtk::SignalListItemFactory::new();
            // Capturas FRACAS: os closures das factories/gestos vivem dentro de
            // widgets que pertencem a esta view. Se capturassem `Rc` forte,
            // formariam um ciclo (view → colview → factory → closure → view) e a
            // tabela anterior nunca seria liberada ao abrir outro arquivo.
            let me = Rc::downgrade(self);
            let col_idx = idx;

            factory.connect_setup(move |_, list_item| {
                let list_item = list_item.downcast_ref::<gtk::ListItem>().unwrap();
                list_item.set_focusable(false); // O foco vai direto para a célula
                
                let label = gtk::EditableLabel::new("");
                label.set_margin_start(4);
                label.set_margin_end(4);
                label.set_max_width_chars(60);
                list_item.set_child(Some(&label));

                let me_key = me.clone();
                let li_key = list_item.clone();

                // Controlador de navegação por teclado
                let key_ctrl = gtk::EventControllerKey::new();
                key_ctrl.connect_key_pressed(move |ctrl, keyval, _code, _state| {
                    let Some(me_key) = me_key.upgrade() else {
                        return glib::Propagation::Proceed;
                    };
                    let lbl = ctrl.widget().unwrap().downcast::<gtk::EditableLabel>().unwrap();
                    if lbl.is_editing() {
                        // Tab/Shift+Tab: commita e move o foco manualmente. Sem isso,
                        // o EditableLabel devolve o foco a si mesmo ao parar de editar,
                        // brigando com o Tab e gerando um flick para a célula original.
                        let tab_dir = match keyval {
                            gdk::Key::Tab => Some(gtk::DirectionType::Right),
                            gdk::Key::ISO_Left_Tab => Some(gtk::DirectionType::Left),
                            _ => None,
                        };
                        if let Some(d) = tab_dir {
                            lbl.stop_editing(true);
                            if let Some(root) = lbl.root() {
                                root.upcast::<gtk::Widget>().child_focus(d);
                            }
                            return glib::Propagation::Stop;
                        }
                        // Up/Down enquanto edita: commita e navega verticalmente.
                        // Mesmo problema do Tab — sem tratar manualmente, o
                        // EditableLabel devolve o foco a si mesmo e gera o flick
                        // de volta para a célula que estava sendo editada.
                        let nav_dir = match keyval {
                            gdk::Key::Up => Some(gtk::DirectionType::Up),
                            gdk::Key::Down => Some(gtk::DirectionType::Down),
                            _ => None,
                        };
                        if let Some(d) = nav_dir {
                            lbl.stop_editing(true);
                            if let Some(root) = lbl.root() {
                                root.upcast::<gtk::Widget>().child_focus(d);
                            }
                            let pos = li_key.position();
                            if keyval == gdk::Key::Up && pos > 0 {
                                me_key.select_only(pos - 1);
                            } else if keyval == gdk::Key::Down
                                && pos + 1 < me_key.store.n_items()
                            {
                                me_key.select_only(pos + 1);
                            }
                            return glib::Propagation::Stop;
                        }
                        return glib::Propagation::Proceed;
                    }
                    // Shift+←/→: estende a seleção de células pela linha.
                    let shift = _state.contains(gdk::ModifierType::SHIFT_MASK);
                    if shift && matches!(keyval, gdk::Key::Left | gdk::Key::Right) {
                        let ncol = me_key.state.columns.borrow().len();
                        let cur = me_key.cur_cell.borrow().as_ref().map_or(col_idx, |(_, c)| *c);
                        let to = if keyval == gdk::Key::Left { cur.saturating_sub(1) } else { (cur + 1).min(ncol.saturating_sub(1)) };
                        let pos = li_key.position();
                        if let Some(w) = me_key.cell_widget(pos, to) {
                            w.grab_focus();
                        }
                        me_key.select_cells_to(pos, to);
                        return glib::Propagation::Stop;
                    }
                    let dir = match keyval {
                        gdk::Key::Left => Some(gtk::DirectionType::Left),
                        gdk::Key::Right => Some(gtk::DirectionType::Right),
                        gdk::Key::Up => Some(gtk::DirectionType::Up),
                        gdk::Key::Down => Some(gtk::DirectionType::Down),
                        _ => None,
                    };
                    if let Some(d) = dir {
                        if let Some(root) = lbl.root() {
                            let w = root.upcast::<gtk::Widget>();
                            w.child_focus(d);
                            
                            // Sincroniza o destaque da linha com o novo foco da
                            // célula; com Shift, estende a seleção a partir da âncora.
                            let pos = li_key.position();
                            let target = if keyval == gdk::Key::Up && pos > 0 {
                                Some(pos - 1)
                            } else if keyval == gdk::Key::Down && pos + 1 < me_key.store.n_items() {
                                Some(pos + 1)
                            } else {
                                None
                            };
                            if let Some(t) = target {
                                if shift {
                                    me_key.select_range_to(t);
                                    me_key.select_cells_to(t, col_idx);
                                } else {
                                    me_key.select_only(t);
                                    me_key.select_single_cell(t, col_idx);
                                }
                            } else if !shift && matches!(keyval, gdk::Key::Left | gdk::Key::Right) {
                                let ncol = me_key.state.columns.borrow().len();
                                let to = if keyval == gdk::Key::Left {
                                    col_idx.saturating_sub(1)
                                } else {
                                    (col_idx + 1).min(ncol.saturating_sub(1))
                                };
                                me_key.select_single_cell(pos, to);
                            }

                            return glib::Propagation::Stop;
                        }
                    }

                    // Digitou um caractere imprimível numa célula focada (sem editar):
                    // inicia a edição já substituindo o conteúdo pelo que foi digitado,
                    // sem exigir Enter. Ignora se houver Ctrl/Alt (atalhos).
                    if lbl.is_editable()
                        && !_state.intersects(gdk::ModifierType::CONTROL_MASK
                            | gdk::ModifierType::ALT_MASK)
                    {
                        if let Some(ch) = keyval.to_unicode() {
                            if !ch.is_control() {
                                lbl.start_editing();
                                lbl.set_text(&ch.to_string());
                                lbl.set_position(-1);
                                return glib::Propagation::Stop;
                            }
                        }
                    }
                    glib::Propagation::Proceed
                });
                label.add_controller(key_ctrl);

                // A célula que recebe o foco vira a célula atual.
                let me_focus = me.clone();
                let li_focus = list_item.clone();
                let focus = gtk::EventControllerFocus::new();
                focus.connect_enter(move |f| {
                    let Some(me) = me_focus.upgrade() else { return };
                    let Some(row) = li_focus.item().and_downcast::<RowObject>() else { return };
                    me.set_current_cell(&f.widget().unwrap(), row, col_idx);
                });
                label.add_controller(focus);

                if let Some(me) = me.upgrade() {
                    me.cell_items
                        .borrow_mut()
                        .insert(label.clone().upcast(), (list_item.downgrade(), col_idx));
                }

                // menu de contexto (botão direito) sensível à linha/coluna
                let me_ctx = me.clone();
                let li_ctx = list_item.clone();
                let gesture = gtk::GestureClick::new();
                gesture.set_button(gdk::BUTTON_SECONDARY);
                // Capture + claim: intercepta o clique antes do GtkText interno do
                // EditableLabel, evitando que apareça o menu padrão (copiar/colar).
                gesture.set_propagation_phase(gtk::PropagationPhase::Capture);
                gesture.connect_pressed(move |g, _, x, y| {
                    let Some(me_ctx) = me_ctx.upgrade() else { return };
                    g.set_state(gtk::EventSequenceState::Claimed);
                    if let Some(item) = li_ctx.item() {
                        if let Ok(row) = item.downcast::<RowObject>() {
                            me_ctx.ctx_rowid.set(row.rowid());
                        }
                    }
                    // Seleciona a linha clicada para deixar claro o alvo da ação.
                    // Se ela já faz parte de uma seleção, a seleção é mantida
                    // (Copiar/Excluir agem sobre todas as linhas selecionadas).
                    let pos = li_ctx.position();
                    if pos != gtk::INVALID_LIST_POSITION {
                        if me_ctx.selection.is_selected(pos) {
                            *me_ctx.cursor.borrow_mut() = me_ctx.store.item(pos).and_downcast();
                        } else {
                            me_ctx.select_only(pos);
                        }
                        // idem para as células: fora da seleção, só a clicada
                        let in_cells = me_ctx.cell_rects.borrow().iter().any(|r| r.contains(pos, col_idx));
                        if !in_cells {
                            me_ctx.select_single_cell(pos, col_idx);
                        }
                    }
                    me_ctx.ctx_col.set(col_idx);
                    if let Some(w) = g.widget() {
                        if let Some(p) = w.compute_point(
                            &me_ctx.colview,
                            &gtk::graphene::Point::new(x as f32, y as f32),
                        ) {
                            let rect = gdk::Rectangle::new(p.x() as i32, p.y() as i32, 1, 1);
                            me_ctx.menu.set_pointing_to(Some(&rect));
                        }
                    }
                    // Leva o foco (e o contorno de destaque) para a célula
                    // clicada: ao fechar o menu, o foco volta para ela, e não
                    // para a célula de outra linha que estava focada antes.
                    if let Some(w) = g.widget() {
                        w.grab_focus();
                    }
                    me_ctx.popup_ctx_menu();
                });
                label.add_controller(gesture);

                // Clique esquerdo em duas etapas: o 1º clique apenas seleciona a
                // linha (destaque visual) e foca a célula, sem editar; o 2º clique
                // (linha já selecionada) deixa o EditableLabel entrar em edição.
                let me_sel = me.clone();
                let li_sel = list_item.clone();
                let sel_gesture = gtk::GestureClick::new();
                sel_gesture.set_button(gdk::BUTTON_PRIMARY);
                sel_gesture.set_propagation_phase(gtk::PropagationPhase::Capture);
                sel_gesture.connect_pressed(move |g, _n, _x, _y| {
                    let Some(me_sel) = me_sel.upgrade() else { return };
                    let lbl = g.widget().unwrap().downcast::<gtk::EditableLabel>().unwrap();
                    if lbl.is_editing() {
                        return; // já editando: não interfere
                    }
                    let pos = li_sel.position();
                    if pos == gtk::INVALID_LIST_POSITION {
                        return;
                    }
                    let mods = g.current_event_state();
                    if mods.contains(gdk::ModifierType::CONTROL_MASK) {
                        // Ctrl+clique: marca/desmarca a linha e a célula, sem editar
                        me_sel.toggle_row(pos);
                        me_sel.toggle_cell(pos, col_idx);
                    } else if mods.contains(gdk::ModifierType::SHIFT_MASK) {
                        // Shift+clique: seleciona o intervalo desde a âncora
                        me_sel.select_range_to(pos);
                        me_sel.select_cells_to(pos, col_idx);
                    } else if me_sel.cursor_pos() != pos
                        || me_sel.selection.selection().size() != 1
                        || me_sel.selected_cell_count() > 1
                    {
                        // 1º clique: seleciona só esta linha e foca a célula,
                        // mas bloqueia a edição
                        me_sel.select_only(pos);
                        me_sel.select_single_cell(pos, col_idx);
                    } else {
                        // linha já selecionada sozinha: não reivindica →
                        // o EditableLabel entra em edição
                        return;
                    }
                    lbl.grab_focus();
                    g.set_state(gtk::EventSequenceState::Claimed);
                });
                label.add_controller(sel_gesture);

                // commit ao terminar edição
                let me2 = me.clone();
                let li = list_item.clone();
                label.connect_notify_local(Some("editing"), move |lbl, _| {
                    if lbl.is_editing() {
                        return;
                    }
                    let Some(item) = li.item() else { return };
                    let row = item.downcast::<RowObject>().unwrap();
                    let new_val = lbl.text().to_string();
                    if row.value(col_idx) == new_val {
                        return;
                    }
                    row.set_value(col_idx, new_val.clone());
                    let Some(me2) = me2.upgrade() else { return };
                    if *me2.state.editable.borrow() && row.rowid() >= 0 {
                        let col_name = me2.state.columns.borrow()[col_idx].clone();
                        if let Err(e) = me2.state.dp.update_cell(row.rowid(), &col_name, &new_val) {
                            me2.error(&format!("{}\n{e}", tr("cell_write_error")));
                        } else {
                            me2.mark_dirty();
                        }
                    }
                });
            });

            let me_bind = Rc::downgrade(self);
            factory.connect_bind(move |_, list_item| {
                let list_item = list_item.downcast_ref::<gtk::ListItem>().unwrap();
                let row = list_item.item().unwrap().downcast::<RowObject>().unwrap();
                let label = list_item
                    .child()
                    .unwrap()
                    .downcast::<gtk::EditableLabel>()
                    .unwrap();
                label.set_text(&row.value(col_idx));
                let editable = me_bind
                    .upgrade()
                    .map_or(false, |m| *m.state.editable.borrow())
                    && row.editable();
                label.set_editable(editable);
                // As células são recicladas ao rolar: o destaque segue a
                // linha/coluna atual, não o widget.
                if let (Some(me), Some(cell)) = (me_bind.upgrade(), label.parent()) {
                    let is_cur = me
                        .cur_cell
                        .borrow()
                        .as_ref()
                        .is_some_and(|(r, c)| *r == row && *c == col_idx);
                    if is_cur {
                        cell.add_css_class("cell-current");
                        *me.cur_widget.borrow_mut() = Some(cell.clone());
                    } else {
                        cell.remove_css_class("cell-current");
                    }
                    me.style_selected_cell(&cell, list_item.position(), col_idx);
                }
            });

            let column = gtk::ColumnViewColumn::new(Some(name), Some(factory));
            column.set_resizable(true);
            // As colunas dividem a largura da janela; com muitas colunas, a
            // tabela continua rolando na horizontal.
            column.set_expand(true);
            self.colview.append_column(&column);
        }

        self.update_sort_indicators();
        self.refresh_scope();
        // clique no cabeçalho ordena; duplo clique renomeia
        self.arm_headers();
    }

    /// Após (re)criar as colunas, localiza os títulos de cabeçalho e arma o
    /// clique (ordenar) e o botão direito (menu da coluna) de cada um.
    fn arm_headers(self: &Rc<Self>) {
        let me = Rc::downgrade(self);
        let colview = self.colview.clone();
        glib::idle_add_local_once(move || {
            let Some(me) = me.upgrade() else { return };
            let mut titles = Vec::new();
            collect_titles(colview.upcast_ref(), &mut titles);
            for (i, t) in titles.iter().enumerate() {
                me.arm_title(t, i);
            }
            *me.titles.borrow_mut() = titles;
        });
    }

    fn arm_title(self: &Rc<Self>, title: &gtk::Widget, col: usize) {
        // Ordena no "pressed": o cabeçalho do ColumnView tem um gesto de
        // arrastar (redimensionar/reordenar) que reivindica a sequência, e aí
        // o "released" nunca chega.
        let click = gtk::GestureClick::new();
        click.set_button(gdk::BUTTON_PRIMARY);
        let me = Rc::downgrade(self);
        click.connect_pressed(move |_, n_press, _, _| {
            if n_press != 1 {
                return;
            }
            if let Some(me) = me.upgrade() {
                me.cycle_sort(col);
            }
        });
        title.add_controller(click);

        // Botão direito no cabeçalho: menu da coluna. O popover é único e
        // preso à tabela — os títulos são recriados a cada recarga.
        let right = gtk::GestureClick::new();
        right.set_button(gdk::BUTTON_SECONDARY);
        let me = Rc::downgrade(self);
        right.connect_pressed(move |g, _, x, y| {
            let Some(me) = me.upgrade() else { return };
            g.set_state(gtk::EventSequenceState::Claimed);
            me.ctx_col.set(col);
            if let Some(p) = g.widget().and_then(|w| {
                w.compute_point(&me.colview, &gtk::graphene::Point::new(x as f32, y as f32))
            }) {
                let rect = gdk::Rectangle::new(p.x() as i32, p.y() as i32, 1, 1);
                me.header_menu.set_pointing_to(Some(&rect));
            }
            open_menu(me.header_menu.upcast_ref());
        });
        title.add_controller(right);
    }

    /// Clique no cabeçalho: crescente → decrescente → ordem do documento.
    fn cycle_sort(self: &Rc<Self>, col: usize) {
        if self.state.loading.get() {
            return;
        }
        let Some(name) = self.state.columns.borrow().get(col).cloned() else { return };
        let next = match self.state.sort.borrow().clone() {
            Some((n, true)) if n == name => Some((name, false)),
            Some((n, false)) if n == name => None,
            _ => Some((name, true)),
        };
        *self.state.sort.borrow_mut() = next;
        self.update_sort_indicators();
        self.sort_store();
    }

    /// Mostra ▲/▼ no título da coluna ordenada.
    fn update_sort_indicators(&self) {
        let sort = self.state.sort.borrow().clone();
        let names = self.state.columns.borrow();
        let cols = self.colview.columns();
        for i in 0..cols.n_items() {
            let (Some(obj), Some(name)) = (cols.item(i), names.get(i as usize)) else {
                continue;
            };
            let title = match &sort {
                Some((n, asc)) if n == name => format!("{name} {}", if *asc { "▲" } else { "▼" }),
                _ => name.clone(),
            };
            obj.downcast::<gtk::ColumnViewColumn>().unwrap().set_title(Some(&title));
        }
    }

    /// Reordena as linhas já carregadas conforme `state.sort`, numa única
    /// troca do conteúdo do store — sem recarregar a tabela.
    fn sort_store(&self) {
        let n = self.store.n_items();
        let rows: Vec<RowObject> = (0..n)
            .filter_map(|i| self.store.item(i))
            .map(|o| o.downcast::<RowObject>().unwrap())
            .collect();
        let editable = *self.state.editable.borrow();
        let spec = self.state.sort.borrow().as_ref().and_then(|(name, asc)| {
            self.state.columns.borrow().iter().position(|c| c == name).map(|c| (c, *asc))
        });
        let sorted: Vec<RowObject> = match spec {
            Some((col, asc)) => {
                if !editable && self.state.unsorted.borrow().is_none() {
                    *self.state.unsorted.borrow_mut() = Some(rows.clone());
                }
                let mut keyed: Vec<(SortKey, RowObject)> =
                    rows.into_iter().map(|r| (sort_key(&r.value(col)), r)).collect();
                keyed.sort_by(|a, b| cmp_keys(&a.0, &b.0, asc));
                keyed.into_iter().map(|(_, r)| r).collect()
            }
            None if editable => match self.state.dp.row_orders() {
                Ok(ord) => {
                    let mut rows = rows;
                    let key = |r: &RowObject| ord.get(&r.rowid()).copied().unwrap_or(f64::MAX);
                    rows.sort_by(|a, b| key(a).total_cmp(&key(b)));
                    rows
                }
                Err(e) => {
                    self.error(&format!("{}\n{e}", tr("error")));
                    return;
                }
            },
            None => self.state.unsorted.borrow_mut().take().unwrap_or(rows),
        };
        let selected = self.selected_rows();
        self.store.splice(0, n, &sorted);
        self.restore_selection(&selected);
    }

    fn is_sorted(&self) -> bool {
        self.state.sort.borrow().is_some()
    }

    /// Abre um campo de edição inline ancorado no título da coluna.
    fn open_rename_popover(self: &Rc<Self>, anchor: &gtk::Widget, old_name: &str) {
        let pop = gtk::Popover::new();
        pop.set_autohide(true);
        pop.set_parent(anchor);
        let entry = gtk::Entry::builder().text(old_name).build();
        pop.set_child(Some(&entry));

        let me = self.clone();
        let old = old_name.to_string();
        let popc = pop.clone();
        entry.connect_activate(move |e| {
            let new = e.text().to_string();
            popc.popdown();
            if new.trim().is_empty() || new == old {
                return;
            }
            match me.state.dp.rename_column(&old, &new) {
                Ok(()) => {
                    me.mark_dirty();
                    me.show_change(Change::ColumnRenamed { from: old.clone(), to: new.trim().to_string() });
                }
                Err(err) => me.error(&format!("{}\n{err}", tr("column_op_error"))),
            }
        });
        pop.connect_closed(|p| p.unparent());
        pop.popup();
        entry.grab_focus();
    }

    /// Critério atual de Localizar, com as opções da barra de Substituir.
    fn matcher(&self, needle: &str) -> Matcher {
        Matcher::new(needle, self.case_chk.is_active(), self.whole_chk.is_active())
    }

    /// Colunas onde procurar: todas, ou só a escolhida na barra de Substituir.
    fn scope_cols(&self) -> Vec<usize> {
        let ncol = self.state.columns.borrow().len();
        match self.scope_dd.selected() {
            0 | gtk::INVALID_LIST_POSITION => (0..ncol).collect(),
            i => {
                let name = self.scope_list.string(i).map(|s| s.to_string());
                let cols = self.state.columns.borrow();
                cols.iter().position(|c| Some(c) == name.as_ref()).into_iter().collect()
            }
        }
    }

    /// Mantém a lista de escopo igual às colunas visíveis, preservando a
    /// coluna escolhida se ela ainda existir.
    fn refresh_scope(&self) {
        let chosen = match self.scope_dd.selected() {
            0 | gtk::INVALID_LIST_POSITION => None,
            i => self.scope_list.string(i).map(|s| s.to_string()),
        };
        let cols = self.state.columns.borrow().clone();
        let names: Vec<&str> = cols.iter().map(|c| c.as_str()).collect();
        self.scope_list.splice(1, self.scope_list.n_items().saturating_sub(1), &names);
        let sel = chosen
            .and_then(|c| cols.iter().position(|n| *n == c))
            .map_or(0, |p| p as u32 + 1);
        self.scope_dd.set_selected(sel);
    }

    fn toggle_replace_bar(&self) {
        self.replace_toggle.set_active(!self.replace_toggle.is_active());
    }

    /// Ao abrir a barra: foca Localizar se está vazio, senão Substituir por.
    fn focus_replace_bar(&self) {
        if self.find_entry.text().is_empty() {
            self.find_entry.grab_focus();
        } else {
            self.replace_entry.grab_focus();
        }
    }

    fn find_next(&self, needle: &str) {
        let m = self.matcher(needle);
        if m.is_empty() {
            return;
        }
        let n = self.store.n_items();
        if n == 0 {
            return;
        }
        let cols = self.scope_cols();
        let start = self.cursor_pos();
        let start = if start == gtk::INVALID_LIST_POSITION { 0 } else { start + 1 };
        for off in 0..n {
            let pos = (start + off) % n;
            if let Some(obj) = self.store.item(pos) {
                let row = obj.downcast::<RowObject>().unwrap();
                if cols.iter().any(|&c| m.matches(&row.value(c))) {
                    self.select_only(pos);
                    self.colview
                        .scroll_to(pos, None, gtk::ListScrollFlags::FOCUS, None);
                    return;
                }
            }
        }
        self.status.set_text(tr("find_no_match"));
    }

    /// Substituições de uma linha: (coluna, valor novo) de cada célula do
    /// escopo que casa com `m`.
    fn row_replacements(&self, row: &RowObject, m: &Matcher, with: &str, cols: &[usize]) -> Vec<(usize, String)> {
        cols.iter()
            .filter_map(|&c| m.replace(&row.value(c), with).map(|v| (c, v)))
            .collect()
    }

    /// Substituir só vale na visão editável e com a tabela carregada.
    fn can_replace(&self) -> bool {
        if self.busy.get() {
            return false;
        }
        if !*self.state.editable.borrow() {
            self.status.set_text(tr("replace_readonly"));
            return false;
        }
        !self.state.loading.get()
    }

    /// Substitui na linha selecionada (se ela casa) e segue para a próxima.
    fn replace_one(self: &Rc<Self>) {
        if !self.can_replace() {
            return;
        }
        self.commit_pending_edit();
        let needle = self.find_entry.text().to_string();
        let m = self.matcher(&needle);
        if m.is_empty() {
            self.find_entry.grab_focus();
            return;
        }
        let with = self.replace_entry.text().to_string();
        let cols = self.scope_cols();
        let pos = self.cursor_pos();
        if let Some(row) = self.store.item(pos).and_downcast::<RowObject>() {
            let reps = self.row_replacements(&row, &m, &with, &cols);
            if !reps.is_empty() {
                self.apply_replacements(vec![(pos, row, reps)]);
            }
        }
        self.find_next(&needle);
    }

    /// Substitui em todas as linhas visíveis (respeita o filtro), num único
    /// passo de desfazer. Roda em blocos, com a camada de andamento visível,
    /// para a janela não congelar numa tabela grande.
    fn replace_all(self: &Rc<Self>) {
        if !self.can_replace() {
            return;
        }
        self.commit_pending_edit();
        let m = self.matcher(&self.find_entry.text());
        if m.is_empty() {
            self.find_entry.grab_focus();
            return;
        }
        let with = self.replace_entry.text().to_string();
        let cols = self.scope_cols();
        let names = self.state.columns.borrow().clone();
        let total = self.store.n_items();
        if let Err(e) = self.state.dp.batch_begin() {
            self.error(&format!("{}\n{e}", tr("cell_write_error")));
            return;
        }
        self.begin_busy(tr("replacing"));

        const CHUNK: u32 = 4000;
        let weak = Rc::downgrade(self);
        let mut next = 0u32;
        // Linhas alteradas: os RowObjects só mudam depois de o lote ser
        // confirmado, para uma falha no meio não deixar a grade divergente.
        let mut changed: Vec<(u32, RowObject, Vec<(usize, String)>)> = Vec::new();
        glib::idle_add_local(move || {
            let Some(me) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            let end = (next + CHUNK).min(total);
            let mut edits = Vec::new();
            for pos in next..end {
                if let Some(row) = me.store.item(pos).and_downcast::<RowObject>() {
                    let reps = me.row_replacements(&row, &m, &with, &cols);
                    if !reps.is_empty() {
                        edits.extend(
                            reps.iter().map(|(c, v)| (row.rowid(), names[*c].clone(), v.clone())),
                        );
                        changed.push((pos, row, reps));
                    }
                }
            }
            if let Err(e) = me.state.dp.batch_update(&edits) {
                let _ = me.state.dp.batch_end(false);
                me.end_busy();
                me.error(&format!("{}\n{e}", tr("cell_write_error")));
                return glib::ControlFlow::Break;
            }
            next = end;
            me.set_busy_progress(next, total);
            if next < total {
                return glib::ControlFlow::Continue;
            }
            match me.state.dp.batch_end(true) {
                Ok(n) => {
                    me.show_replacements(std::mem::take(&mut changed));
                    if n > 0 {
                        me.mark_dirty();
                    }
                    me.end_busy();
                    me.status.set_text(&format!("{n} {}", tr("cells_replaced")));
                }
                Err(e) => {
                    me.end_busy();
                    me.error(&format!("{}\n{e}", tr("cell_write_error")));
                }
            }
            glib::ControlFlow::Break
        });
    }

    /// Reflete na grade substituições já gravadas: troca os valores nos
    /// objetos das linhas e atualiza as células que estão na tela.
    fn show_replacements(&self, changed: Vec<(u32, RowObject, Vec<(usize, String)>)>) {
        for (_, row, reps) in changed {
            for (c, v) in reps {
                row.set_value(c, v);
            }
        }
        self.refresh_visible_cells();
    }

    /// Reescreve o texto das células visíveis a partir dos objetos das
    /// linhas. Avisar o store com `items_changed` não basta: quando o objeto
    /// é o mesmo, o GTK reaproveita o widget sem refazer o bind, e a célula
    /// continuaria mostrando o valor antigo. As linhas fora da tela pegam o
    /// valor novo no bind, ao aparecerem.
    fn refresh_visible_cells(&self) {
        let cells: Vec<(gtk::Widget, RowObject, usize)> = self
            .cell_items
            .borrow()
            .iter()
            .filter_map(|(w, (li, col))| {
                let row = li.upgrade()?.item()?.downcast::<RowObject>().ok()?;
                Some((w.clone(), row, *col))
            })
            .collect();
        for (w, row, col) in cells {
            let Ok(label) = w.downcast::<gtk::EditableLabel>() else { continue };
            let v = row.value(col);
            if !label.is_editing() && label.text() != v {
                label.set_text(&v);
            }
        }
    }

    fn begin_busy(&self, text: &str) {
        self.busy.set(true);
        self.busy_label.set_text(text);
        self.busy_bar.set_fraction(0.0);
        self.busy_bar.set_text(None);
        self.busy_spinner.start();
        self.busy_shade.set_visible(true);
        self.toolbar.set_sensitive(false);
        self.replace_revealer.set_sensitive(false);
    }

    fn set_busy_progress(&self, done: u32, total: u32) {
        let frac = if total == 0 { 1.0 } else { done as f64 / total as f64 };
        self.busy_bar.set_fraction(frac);
        self.busy_bar
            .set_text(Some(&format!("{done} {} {total} {}", tr("of"), tr("rows"))));
    }

    fn end_busy(&self) {
        self.busy.set(false);
        self.busy_spinner.stop();
        self.busy_shade.set_visible(false);
        self.toolbar.set_sensitive(true);
        self.replace_revealer.set_sensitive(true);
    }

    /// Grava as substituições no banco (um passo de desfazer) e atualiza só as
    /// linhas afetadas na grade. Devolve quantas células mudaram.
    fn apply_replacements(&self, changed: Vec<(u32, RowObject, Vec<(usize, String)>)>) -> usize {
        if changed.is_empty() {
            return 0;
        }
        let names = self.state.columns.borrow().clone();
        let edits: Vec<(i64, String, String)> = changed
            .iter()
            .flat_map(|(_, row, reps)| {
                reps.iter().map(|(c, v)| (row.rowid(), names[*c].clone(), v.clone()))
            })
            .collect();
        match self.state.dp.update_cells(&edits) {
            Ok(n) => {
                self.show_replacements(changed);
                if n > 0 {
                    self.mark_dirty();
                }
                n
            }
            Err(e) => {
                self.error(&format!("{}\n{e}", tr("cell_write_error")));
                0
            }
        }
    }

    fn do_sum(self: &Rc<Self>) {
        let names = self.state.columns.borrow().clone();
        if names.is_empty() {
            return;
        }
        let strs: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
        let dropdown = gtk::DropDown::from_strings(&strs);

        let me = self.clone();
        let dropdown2 = dropdown.clone();
        dialog::form(
            self.window().as_ref(),
            tr("sum_column_title"),
            tr("sum_column_body"),
            &dropdown,
            tr("sum"),
            move || {
                let sel = dropdown2.selected();
                let Some(col) = names.get(sel as usize).cloned() else { return };
                let filter = me.state.last_filter.borrow().clone();
                match me.state.dp.sum_column(&col, Some(&filter)) {
                    Ok((total, count)) => {
                        let body = format!(
                            "{}: {}\n({} {})",
                            tr("total"),
                            fmt_br(total),
                            count,
                            tr("numeric_values")
                        );
                        me.info(&format!("{} {col}", tr("sum")), &body);
                    }
                    Err(e) => me.error(&format!("{}\n{e}", tr("sum_failed"))),
                }
            },
        );
    }

    fn open_sql_dialog(self: &Rc<Self>) {
        let entry = gtk::Entry::builder()
            .text("SELECT * FROM dados")
            .hexpand(true)
            .build();
        let me = self.clone();
        let entry2 = entry.clone();
        dialog::form(
            self.window().as_ref(),
            tr("sql_title"),
            tr("sql_body"),
            &entry,
            tr("run"),
            move || me.run_sql(&entry2.text()),
        );
    }

    fn do_save(self: &Rc<Self>) {
        self.save_then(|_ok| {});
    }

    /// `true` se há edições não salvas em disco.
    pub fn is_dirty(&self) -> bool {
        *self.state.dirty.borrow()
    }

    /// Grava diretamente em `path`, atualizando estado (destino conhecido e
    /// flag de "sujo"). Retorna `true` em sucesso.
    fn write_to(self: &Rc<Self>, path: &std::path::Path) -> bool {
        // guarda a versão que está em disco antes de sobrescrevê-la
        crate::backup::save_version(path);
        match self.state.dp.save(path) {
            Ok(()) => {
                crate::recent::add(path);
                // salvo: a cópia de recuperação não é mais necessária
                self.recovery.remove();
                self.state.backed_rev.set(self.state.rev.get());
                *self.state.dirty.borrow_mut() = false;
                *self.state.saved_path.borrow_mut() = Some(path.to_path_buf());
                let when = glib::DateTime::now_local()
                    .and_then(|d| d.format("%d/%m/%Y %H:%M:%S"))
                    .map(|s| s.to_string())
                    .unwrap_or_default();
                self.status.set_text(&format!(
                    "{}: {}  ·  {}",
                    tr("save_tooltip"),
                    path.display(),
                    when
                ));
                true
            }
            Err(e) => {
                self.error(&format!("{}\n{e}", tr("save_error")));
                false
            }
        }
    }

    /// Salva. Se o documento já tem um destino em disco, sobrescreve direto
    /// (sem abrir o seletor); caso contrário, abre "Salvar como". `after` é
    /// chamado quando termina, com `true` se gravou.
    pub fn save_then(self: &Rc<Self>, after: impl FnOnce(bool) + 'static) {
        if self.busy.get() {
            after(false);
            return;
        }
        self.commit_pending_edit();
        let known = self.state.saved_path.borrow().clone();
        if let Some(path) = known {
            let ok = self.write_to(&path);
            after(ok);
        } else {
            self.save_as(after);
        }
    }

    /// Abre sempre o seletor de salvamento (Salvar como…).
    pub fn save_as(self: &Rc<Self>, after: impl FnOnce(bool) + 'static) {
        self.commit_pending_edit();
        let dialog = gtk::FileDialog::builder()
            .title(tr("save_as_xml"))
            .initial_name(
                self.state
                    .saved_path
                    .borrow()
                    .as_ref()
                    .unwrap_or(&self.state.path)
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| "saida.xml".into()),
            )
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

    fn do_export_csv(self: &Rc<Self>) {
        // exporta a visão atual
        let where_or_all = "";
        let res = match self.state.dp.filter(Some(where_or_all)) {
            Ok(r) => r,
            Err(e) => {
                self.error(&format!("{}\n{e}", tr("csv_export_error")));
                return;
            }
        };
        // preserva o nome original do arquivo, trocando a extensão para .csv
        let initial = self
            .state
            .path
            .file_stem()
            .map(|s| format!("{}.csv", s.to_string_lossy()))
            .unwrap_or_else(|| "export.csv".into());
        let dialog = gtk::FileDialog::builder()
            .title(tr("export_csv_title"))
            .initial_name(initial)
            .build();
        let me = self.clone();
        dialog.save(self.window().as_ref(), gio::Cancellable::NONE, move |r| {
            if let Ok(file) = r {
                if let Some(path) = file.path() {
                    match me.state.dp.export_csv(&path, &res) {
                        Ok(()) => me.status.set_text(&format!("CSV: {}", path.display())),
                        Err(e) => me.error(&format!("{}\n{e}", tr("csv_export_error"))),
                    }
                }
            }
        });
    }

    fn window(&self) -> Option<gtk::Window> {
        self.root.root().and_then(|r| r.downcast::<gtk::Window>().ok())
    }

    /// Confirma a edição de célula em andamento (se houver) antes de salvar.
    /// Sem isso, Ctrl+S/botão salvam com o valor antigo, pois o commit só
    /// ocorre quando o EditableLabel perde o foco.
    fn commit_pending_edit(&self) {
        let Some(win) = self.window() else { return };
        let Some(focus) = GtkWindowExt::focus(&win) else { return };
        let mut w = Some(focus);
        while let Some(cur) = w {
            if let Ok(lbl) = cur.clone().downcast::<gtk::EditableLabel>() {
                if lbl.is_editing() {
                    lbl.stop_editing(true); // dispara o commit (notify "editing")
                }
                break;
            }
            w = cur.parent();
        }
    }

    fn info(&self, heading: &str, body: &str) {
        dialog::info(self.window().as_ref(), heading, body);
    }

    fn error(&self, body: &str) {
        dialog::error(self.window().as_ref(), body);
    }

    /// Marca o documento como alterado e atualiza os botões de desfazer/refazer.
    /// Grava a cópia de recuperação, se houve alteração desde a última. O XML
    /// é montado aqui (rápido); a escrita em disco vai para outra thread.
    fn autosave(self: &Rc<Self>) {
        let rev = self.state.rev.get();
        if !self.is_dirty()
            || rev == self.state.backed_rev.get()
            || self.busy.get()
            || self.state.loading.get()
            || self.backup_writing.get()
        {
            return;
        }
        let Ok(xml) = self.state.dp.to_xml() else { return };
        let (data, meta) = self.recovery.paths();
        let original = self.state.saved_path.borrow().clone();
        let name = original
            .as_deref()
            .unwrap_or(&self.state.path)
            .file_name()
            .map_or_else(|| "novo.xml".into(), |s| s.to_string_lossy().into_owned());
        // De tempos em tempos a cópia também entra no histórico de versões,
        // para dar para voltar a um ponto da edição mesmo sem ter salvo.
        let snapshot = original
            .clone()
            .filter(|_| self.last_snapshot.get().elapsed() >= SNAPSHOT_EVERY);
        if snapshot.is_some() {
            self.last_snapshot.set(std::time::Instant::now());
        }
        self.backup_writing.set(true);
        let handle = gio::spawn_blocking(move || {
            if let Some(path) = &snapshot {
                let _ = crate::backup::save_snapshot(path, xml.as_bytes(), crate::backup::Kind::Editing);
            }
            crate::backup::Recovery::write(&data, &meta, xml.as_bytes(), original.as_deref(), &name)
                .is_ok()
        });
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let ok = handle.await.unwrap_or(false);
            let Some(me) = weak.upgrade() else { return };
            me.backup_writing.set(false);
            if ok {
                me.state.backed_rev.set(rev);
            }
            // salvo enquanto a cópia era gravada: ela não serve mais
            if !me.is_dirty() {
                me.recovery.remove();
            } else if me.state.rev.get() != rev {
                // houve alteração durante a gravação: grava de novo
                me.schedule_backup();
            }
        });
    }

    /// Apaga a cópia de recuperação (documento fechado sem pendências ou
    /// alterações descartadas pelo usuário).
    pub fn discard_backup(&self) {
        self.recovery.remove();
    }

    /// Documento aberto a partir de uma cópia de recuperação ou de uma versão
    /// do histórico: fica marcado como alterado, para o usuário salvar.
    pub fn mark_recovered(&self, message: &str) {
        self.mark_dirty();
        self.status.set_text(message);
    }

    /// Define quem troca o documento da janela por uma versão restaurada.
    pub fn set_restore_handler(&self, handler: Rc<dyn Fn(crate::backup::Version)>) {
        *self.restore_handler.borrow_mut() = Some(handler);
    }

    /// Abre o histórico de versões do arquivo, comparando cada uma com o
    /// documento como está agora (inclusive alterações não salvas).
    fn show_history(self: &Rc<Self>) {
        if self.busy.get() {
            return;
        }
        let Some(path) = self.state.saved_path.borrow().clone() else {
            self.info(tr("history_title"), tr("history_unsaved_doc"));
            return;
        };
        let versions = crate::backup::versions_of(&path);
        if versions.is_empty() {
            self.info(tr("history_title"), tr("history_empty"));
            return;
        }
        self.commit_pending_edit();
        let rows = match self.state.dp.filter(None) {
            Ok(r) => r.rows,
            Err(e) => {
                self.error(&e.to_string());
                return;
            }
        };
        let current = crate::diff::Table { names: self.state.dp.field_names(), rows };
        let name = path.file_name().map_or_else(String::new, |s| s.to_string_lossy().into_owned());
        let weak = Rc::downgrade(self);
        crate::history::show(
            self.window().as_ref(),
            &name,
            versions,
            current,
            Rc::new(move |v| {
                if let Some(me) = weak.upgrade() {
                    me.restore_version(v);
                }
            }),
        );
    }

    /// Troca o documento pela versão `v`. Alterações não salvas não se
    /// perdem: antes, o estado atual entra no histórico.
    fn restore_version(self: &Rc<Self>, v: crate::backup::Version) {
        let Some(handler) = self.restore_handler.borrow().clone() else { return };
        if self.is_dirty() {
            let saved = self.state.dp.to_xml().map_err(|e| e.to_string()).and_then(|xml| {
                crate::backup::save_snapshot(&v.original, xml.as_bytes(), crate::backup::Kind::BeforeRestore)
                    .map_err(|e| e.to_string())
            });
            if let Err(e) = saved {
                self.error(&format!("{}\n{e}", tr("history_snapshot_error")));
                return;
            }
        }
        self.discard_backup();
        handler(v);
    }

    fn mark_dirty(&self) {
        self.state.rev.set(self.state.rev.get() + 1);
        *self.state.dirty.borrow_mut() = true;
        self.refresh_history_buttons();
        self.schedule_backup();
    }

    /// Agenda a cópia de recuperação para logo depois da última alteração
    /// (cada nova alteração adia a gravação em `BACKUP_DELAY`).
    fn schedule_backup(&self) {
        const BACKUP_DELAY: std::time::Duration = std::time::Duration::from_millis(800);
        let g = self.backup_gen.get().wrapping_add(1);
        self.backup_gen.set(g);
        let weak = self.weak_self.borrow().clone();
        glib::timeout_add_local_once(BACKUP_DELAY, move || {
            let Some(me) = weak.upgrade() else { return };
            if me.backup_gen.get() != g {
                return; // outra alteração veio depois: ela agenda a sua
            }
            if me.busy.get() || me.state.loading.get() {
                me.schedule_backup(); // tenta de novo quando a operação acabar
            } else {
                me.autosave();
            }
        });
    }

    /// Desfazer/refazer só valem na visão editável: o resultado de uma
    /// consulta SQL livre não corresponde às linhas da tabela.
    fn refresh_history_buttons(&self) {
        let editable = *self.state.editable.borrow();
        self.undo_btn.set_sensitive(editable && self.state.dp.can_undo());
        self.redo_btn.set_sensitive(editable && self.state.dp.can_redo());
    }

    /// `true` se o foco está num campo de texto (célula em edição, filtro,
    /// localizar), que tem seu próprio desfazer.
    /// Executa a ação de um atalho de teclado (ids em `shortcuts::ACTIONS`).
    fn run_shortcut(self: &Rc<Self>, id: &str) {
        // ações de linha usam a linha do cursor, como o menu de contexto usa
        // a linha clicada
        let cursor = self.cursor.borrow().as_ref().map(|r| r.rowid());
        if id.starts_with("row_") || id.starts_with("move_") {
            let Some(rid) = cursor else { return };
            self.ctx_rowid.set(rid);
        }
        match id {
            "save" => self.do_save(),
            "save_as" => self.save_as(|_ok| {}),
            "undo" => self.do_undo(),
            "redo" => self.do_redo(),
            "find" => {
                self.find_entry.grab_focus();
            }
            "replace" => self.toggle_replace_bar(),
            "history" => self.show_history(),
            "sql" => self.open_sql_dialog(),
            "select_all" => {
                self.selection.select_all();
                self.select_all_cells();
            }
            "copy" => {
                self.copy_rows();
            }
            "cut" => self.cut_rows(),
            "paste" => self.paste_rows(cursor, false),
            "clear_cells" => self.clear_selected_cells(),
            "delete_rows" => self.delete_selected_rows(),
            "row_below" => self.ctx_insert_row(false),
            "row_above" => self.ctx_insert_row(true),
            "row_clone" => self.ctx_clone_row(false),
            "move_up" => self.ctx_move_row(true),
            "move_down" => self.ctx_move_row(false),
            _ => {}
        }
    }

    fn text_has_focus(&self) -> bool {
        self.window()
            .and_then(|w| GtkWindowExt::focus(&w))
            .is_some_and(|f| f.is::<gtk::Text>())
    }

    fn do_undo(self: &Rc<Self>) {
        self.history_step(false);
    }

    fn do_redo(self: &Rc<Self>) {
        self.history_step(true);
    }

    fn history_step(self: &Rc<Self>, forward: bool) {
        if !*self.state.editable.borrow() || self.state.loading.get() || self.busy.get() {
            return;
        }
        self.commit_pending_edit();
        let res = if forward { self.state.dp.redo() } else { self.state.dp.undo() };
        match res {
            Ok(None) => {}
            Ok(Some(change)) => {
                self.mark_dirty();
                self.show_change(change);
            }
            Err(e) => self.error(&format!("{}\n{e}", tr("error"))),
        }
    }

    /// Reflete na grade o que o desfazer/refazer mudou, mexendo só nas linhas
    /// e colunas afetadas — sem recarregar a tabela.
    fn show_change(self: &Rc<Self>, change: Change) {
        match change {
            Change::Cell { rowid, column, value } => {
                let col = self.state.columns.borrow().iter().position(|c| *c == column);
                let (Some(col), Some(pos)) = (col, self.row_position(rowid)) else {
                    return; // linha ou coluna fora da visão atual
                };
                if let Some(obj) = self.store.item(pos) {
                    obj.downcast::<RowObject>().unwrap().set_value(col, value);
                    self.refresh_visible_cells();
                }
                self.select_rowid(rowid);
            }
            Change::Cells(cells) => {
                let mut pos_of = std::collections::HashMap::new();
                for i in 0..self.store.n_items() {
                    if let Some(obj) = self.store.item(i) {
                        pos_of.insert(obj.downcast::<RowObject>().unwrap().rowid(), i);
                    }
                }
                let cols = self.state.columns.borrow().clone();
                let mut changed: Vec<(u32, RowObject, Vec<(usize, String)>)> = Vec::new();
                for (rowid, column, value) in cells {
                    let (Some(&pos), Some(c)) =
                        (pos_of.get(&rowid), cols.iter().position(|n| *n == column))
                    else {
                        continue; // fora da visão atual
                    };
                    if let Some(row) = self.store.item(pos).and_downcast::<RowObject>() {
                        changed.push((pos, row, vec![(c, value)]));
                    }
                }
                self.show_replacements(changed);
            }
            Change::Batch(changes) => {
                let mut added = Vec::new();
                let mut removed = std::collections::HashSet::new();
                let mut cells = Vec::new();
                for c in changes {
                    match c {
                        Change::RowAdded { rowid } => added.push(rowid),
                        Change::RowRemoved { rowid } => {
                            removed.insert(rowid);
                        }
                        Change::Cell { rowid, column, value } => cells.push((rowid, column, value)),
                        other => self.show_change(other),
                    }
                }
                if !removed.is_empty() {
                    self.show_rows_removed(&removed);
                }
                if !added.is_empty() {
                    self.show_rows_added(&added);
                }
                if !cells.is_empty() {
                    self.show_change(Change::Cells(cells));
                }
            }
            Change::RowAdded { rowid } => {
                self.show_row_added(rowid);
                if self.is_sorted() {
                    self.sort_store();
                    self.select_rowid(rowid);
                }
            }
            Change::RowRemoved { rowid } => {
                if let Some(pos) = self.row_position(rowid) {
                    self.store.remove(pos);
                    self.refresh_status();
                }
            }
            Change::RowsSwapped { a, b } => {
                // Se uma das duas está fora do filtro, a ordem visível não muda.
                let (Some(pa), Some(pb)) = (self.row_position(a), self.row_position(b)) else {
                    return;
                };
                let (lo, hi) = (pa.min(pb), pa.max(pb));
                let (Some(o_lo), Some(o_hi)) = (self.store.item(lo), self.store.item(hi)) else {
                    return;
                };
                self.store.splice(hi, 1, &[o_lo]);
                self.store.splice(lo, 1, &[o_hi]);
                if self.is_sorted() {
                    self.sort_store();
                }
                self.select_rowid(a);
            }
            Change::ColumnAdded { index, name, values } => {
                let cnt = self.store.n_items();
                for i in 0..cnt {
                    if let Some(obj) = self.store.item(i) {
                        let row = obj.downcast::<RowObject>().unwrap();
                        let v = values.get(&row.rowid()).cloned().unwrap_or_default();
                        row.insert_value(index, v);
                    }
                }
                {
                    let mut cols = self.state.columns.borrow_mut();
                    let at = index.min(cols.len());
                    cols.insert(at, name);
                }
                let names = self.state.columns.borrow().clone();
                self.rebuild_columns(&names);
            }
            Change::ColumnRemoved { name } => {
                let col = self.state.columns.borrow().iter().position(|c| *c == name);
                let Some(col) = col else { return };
                let cnt = self.store.n_items();
                for i in 0..cnt {
                    if let Some(obj) = self.store.item(i) {
                        obj.downcast::<RowObject>().unwrap().remove_value(col);
                    }
                }
                self.state.columns.borrow_mut().remove(col);
                let names = self.state.columns.borrow().clone();
                self.rebuild_columns(&names);
            }
            Change::ColumnRenamed { from, to } => {
                if let Some(c) = self.state.columns.borrow_mut().iter_mut().find(|c| **c == from) {
                    *c = to;
                }
                let names = self.state.columns.borrow().clone();
                self.rebuild_columns(&names);
            }
        }
    }

    /// Recoloca na grade uma linha restaurada, na posição que ela ocupa na
    /// ordem do documento — se ela passar pelo filtro atual.
    fn show_row_added(self: &Rc<Self>, rowid: i64) {
        let filter = self.state.last_filter.borrow().clone();
        let values = match self.state.dp.visible_row(rowid, &filter) {
            Ok(Some(v)) => v,
            Ok(None) => return, // fora do filtro
            Err(e) => {
                self.error(&format!("{}\n{e}", tr("error")));
                return;
            }
        };
        // posição de cada linha visível, para achar a próxima vizinha
        let mut pos_of = std::collections::HashMap::new();
        for i in 0..self.store.n_items() {
            if let Some(obj) = self.store.item(i) {
                pos_of.insert(obj.downcast::<RowObject>().unwrap().rowid(), i);
            }
        }
        let at = match self.state.dp.next_row_where(rowid, |id| pos_of.contains_key(&id)) {
            Ok(Some(next)) => pos_of[&next],
            Ok(None) => self.store.n_items(),
            Err(e) => {
                self.error(&format!("{}\n{e}", tr("error")));
                return;
            }
        };
        self.store.insert(at, &RowObject::new(rowid, values, true));
        self.refresh_status();
        self.select_rowid(rowid);
    }

    /// Seleciona e rola até a linha `rowid`, se ela estiver na visão.
    fn select_rowid(&self, rowid: i64) {
        if let Some(pos) = self.row_position(rowid) {
            self.select_only(pos);
            self.colview.scroll_to(pos, None, gtk::ListScrollFlags::FOCUS, None);
        }
    }

    /// Localiza a posição de um `rowid` dentro do store (varredura linear barata).
    fn row_position(&self, rowid: i64) -> Option<u32> {
        let n = self.store.n_items();
        for i in 0..n {
            if let Some(obj) = self.store.item(i) {
                if obj.downcast::<RowObject>().unwrap().rowid() == rowid {
                    return Some(i);
                }
            }
        }
        None
    }

    /// Atualiza o rodapé com a contagem atual do store (sem re-consultar o banco).
    fn refresh_status(&self) {
        let n = self.store.n_items();
        let f = self.state.last_filter.borrow();
        let extra = if f.trim().is_empty() {
            String::new()
        } else {
            format!("  ·  filtro: {f}")
        };
        self.status.set_text(&format!("Total: {n}{extra}"));
    }

    // ---------- ações do menu de contexto ----------

    fn ctx_insert_row(self: &Rc<Self>, above: bool) {
        if !*self.state.editable.borrow() || self.state.loading.get() {
            return; // visão somente leitura (SQL livre) ou carga em andamento
        }
        let rid = self.ctx_rowid.get();
        if rid < 0 {
            return;
        }
        match self.state.dp.insert_row_relative(rid, above) {
            Ok(new_rid) => {
                self.mark_dirty();
                // insere apenas a nova linha no store (sem re-consultar 50k linhas)
                let ncol = self.state.columns.borrow().len();
                let obj = RowObject::new(new_rid, vec![String::new(); ncol], true);
                let pos = self.row_position(rid).unwrap_or(self.store.n_items());
                let at = if above { pos } else { pos + 1 };
                self.store.insert(at, &obj);
                self.refresh_status();
                self.select_only(at);
                self.colview.scroll_to(at, None, gtk::ListScrollFlags::FOCUS, None);
            }
            Err(e) => self.error(&format!("{}\n{e}", tr("error"))),
        }
    }

    /// Exclui a linha clicada — ou, se ela faz parte de uma seleção, todas
    /// as linhas selecionadas, num único passo de desfazer.
    fn ctx_delete_row(self: &Rc<Self>) {
        if !*self.state.editable.borrow() || self.state.loading.get() {
            return;
        }
        let rid = self.ctx_rowid.get();
        if rid < 0 {
            return;
        }
        let mut ids: Vec<i64> = self.selected_rows().iter().map(|r| r.rowid()).collect();
        if !ids.contains(&rid) {
            ids = vec![rid];
        }
        self.delete_rowids(&ids);
    }

    fn delete_rowids(self: &Rc<Self>, ids: &[i64]) {
        let ids: Vec<i64> = ids.iter().copied().filter(|&r| r >= 0).collect();
        if ids.is_empty() {
            return;
        }
        match self.state.dp.delete_rows(&ids) {
            Ok(()) => {
                self.mark_dirty();
                self.show_rows_removed(&ids.into_iter().collect());
            }
            Err(e) => self.error(&format!("{}\n{e}", tr("error"))),
        }
    }

    /// Célula sob o ponto `(x, y)` da tabela: posição da linha no store e o
    /// widget da célula.
    fn cell_at(&self, x: f64, y: f64) -> Option<(u32, usize, gtk::Widget)> {
        let mut w = self.colview.pick(x, y, gtk::PickFlags::DEFAULT);
        let items = self.cell_items.borrow();
        while let Some(cur) = w {
            if let Some((li, col)) = items.get(&cur) {
                let pos = li.upgrade().map_or(gtk::INVALID_LIST_POSITION, |li| li.position());
                return (pos != gtk::INVALID_LIST_POSITION).then_some((pos, *col, cur));
            }
            w = cur.parent();
        }
        None
    }

    /// Linha (posição no store) sob o ponto `(x, y)` da tabela.
    fn row_at(&self, x: f64, y: f64) -> Option<u32> {
        self.cell_at(x, y).map(|(pos, _, _)| pos)
    }

    /// Widget visível da célula (linha `pos`, coluna `col`), se houver.
    fn cell_widget(&self, pos: u32, col: usize) -> Option<gtk::Widget> {
        self.cell_items.borrow().iter().find_map(|(w, (li, c))| {
            (*c == col && li.upgrade().is_some_and(|li| li.position() == pos)).then(|| w.clone())
        })
    }

    /// Arrastar com o botão esquerdo seleciona as linhas do ponto de partida
    /// até a linha sob o ponteiro. Com o ponteiro parado perto da borda de
    /// cima ou de baixo, a tabela continua rolando e a seleção acompanha.
    /// Enquanto o ponteiro fica na linha de origem nada muda — aí o arrasto
    /// continua sendo do texto da célula em edição, se houver uma.
    fn arm_drag_select(self: &Rc<Self>) {
        let drag = gtk::GestureDrag::new();
        drag.set_button(gdk::BUTTON_PRIMARY);
        drag.set_propagation_phase(gtk::PropagationPhase::Capture);
        let st = Rc::new(DragSelect::default());

        let weak = Rc::downgrade(self);
        let s1 = st.clone();
        drag.connect_drag_begin(move |_, x, y| {
            s1.active.set(false);
            let cell = weak.upgrade().and_then(|me| me.cell_at(x, y));
            s1.start.set(cell.as_ref().map(|(pos, _, _)| *pos));
            s1.start_col.set(cell.map_or(0, |(_, col, _)| col));
            s1.last.set((x, y));
        });

        let weak = Rc::downgrade(self);
        let s2 = st.clone();
        drag.connect_drag_update(move |g, dx, dy| {
            let Some(me) = weak.upgrade() else { return };
            let Some((sx, sy)) = g.start_point() else { return };
            s2.last.set((sx + dx, sy + dy));
            if !s2.active.get() {
                let Some(from) = s2.start.get() else { return };
                let (x, y) = s2.last.get();
                match me.row_at(x, y) {
                    Some(to) if to != from => {}
                    _ => return, // ainda na linha de origem
                }
                // Passou para outra linha: o arrasto é de seleção. Toma o
                // evento para si (o texto da célula não é mais arrastado) e
                // encerra uma edição que tenha começado no clique.
                s2.active.set(true);
                g.set_state(gtk::EventSequenceState::Claimed);
                me.commit_pending_edit();
                *me.anchor.borrow_mut() = me.store.item(from).and_downcast();
                me.anchor_col.set(s2.start_col.get());
            }
            me.drag_select_step(&s2);
            // Perto da borda, um temporizador continua rolando mesmo com o
            // ponteiro parado.
            if !s2.ticking.get() {
                s2.ticking.set(true);
                let weak = Rc::downgrade(&me);
                let s3 = s2.clone();
                glib::timeout_add_local(std::time::Duration::from_millis(40), move || {
                    let Some(me) = weak.upgrade() else {
                        return glib::ControlFlow::Break;
                    };
                    if !s3.active.get() || !me.drag_select_step(&s3) {
                        s3.ticking.set(false);
                        return glib::ControlFlow::Break;
                    }
                    glib::ControlFlow::Continue
                });
            }
        });

        drag.connect_drag_end(move |_, _, _| st.active.set(false));
        self.colview.add_controller(drag);
    }

    /// Um passo da seleção por arrasto: rola se o ponteiro está perto de uma
    /// borda e seleciona da linha de origem até a linha sob o ponteiro.
    /// Devolve `true` se rolou (o temporizador deve continuar).
    fn drag_select_step(&self, st: &DragSelect) -> bool {
        let Some(from) = st.start.get() else { return false };
        let (x, y) = st.last.get();
        const HEADER: f64 = 30.0;
        const EDGE: f64 = 24.0;
        let h = f64::from(self.colview.height());
        let mut scrolled = false;
        if let Some(adj) = self.colview.vadjustment() {
            // quanto mais longe da borda, mais rápido
            let speed = |d: f64| (d / EDGE).clamp(0.3, 4.0) * adj.step_increment().max(12.0);
            let before = adj.value();
            if y < HEADER + EDGE {
                adj.set_value(before - speed(HEADER + EDGE - y));
            } else if y > h - EDGE {
                adj.set_value(before + speed(y - (h - EDGE)));
            }
            scrolled = adj.value() != before;
        }
        let probe = y.clamp(HEADER + 2.0, (h - 2.0).max(HEADER + 2.0));
        if let Some((to, col, cell)) = self.cell_at(x, probe) {
            let (lo, hi) = (from.min(to), from.max(to));
            self.selection.select_range(lo, hi - lo + 1, true);
            self.set_cell_rects(vec![CellRect::new(from, to, st.start_col.get(), col)]);
            *self.cursor.borrow_mut() = self.store.item(to).and_downcast();
            // O contorno (e o foco do teclado) acompanha a célula sob o
            // ponteiro, junto com o destaque das linhas.
            if !cell.has_focus() {
                cell.grab_focus();
            }
        }
        scrolled
    }

    /// Fecha os menus de contexto quando há um clique fora deles, sem
    /// consumir o clique: ele segue para onde foi dado. Assim o botão direito
    /// em outra célula já abre o menu dela, e o esquerdo já seleciona.
    fn close_menus_on_outside_click(self: &Rc<Self>) {
        let attached = Rc::new(Cell::new(false));
        let weak = Rc::downgrade(self);
        // O controlador vai na janela (pega também cliques no cabeçalho dela),
        // que só existe depois que a view é colocada nela.
        self.root.connect_map(move |root| {
            if attached.get() {
                return;
            }
            let Some(win) = root.root() else { return };
            attached.set(true);
            let gesture = gtk::GestureClick::new();
            gesture.set_button(0); // qualquer botão
            gesture.set_propagation_phase(gtk::PropagationPhase::Capture);
            let weak = weak.clone();
            gesture.connect_pressed(move |g, _, _, _| {
                let Some(me) = weak.upgrade() else { return };
                if !me.menu.is_visible() && !me.header_menu.is_visible() {
                    return;
                }
                // Cliques dentro do menu (e dos submenus) chegam de outra
                // superfície; só os da superfície da janela estão fora dele.
                let clicked = g.current_event().and_then(|e| e.surface());
                let window = g.widget().and_then(|w| w.native()).and_then(|n| n.surface());
                if clicked.is_some() && clicked == window {
                    close_menu(me.menu.upcast_ref());
                    close_menu(me.header_menu.upcast_ref());
                }
            });
            win.add_controller(gesture);
        });
    }

    /// Marca a célula de `label` como a atual: só ela leva o contorno.
    fn set_current_cell(&self, label: &gtk::Widget, row: RowObject, col: usize) {
        if let Some(old) = self.cur_widget.borrow_mut().take() {
            old.remove_css_class("cell-current");
        }
        if let Some(cell) = label.parent() {
            cell.add_css_class("cell-current");
            *self.cur_widget.borrow_mut() = Some(cell);
        }
        *self.cur_cell.borrow_mut() = Some((row, col));
    }

    // ---------- seleção de células ----------

    fn set_cell_rects(&self, rects: Vec<CellRect>) {
        *self.cell_rects.borrow_mut() = rects;
        self.refresh_cell_classes();
    }

    /// Total de células selecionadas.
    fn selected_cell_count(&self) -> u64 {
        self.cell_rects.borrow().iter().map(|r| r.cells()).sum()
    }

    /// Seleciona só a célula (`pos`, `col`), que vira a âncora do Shift.
    fn select_single_cell(&self, pos: u32, col: usize) {
        self.anchor_col.set(col);
        self.set_cell_rects(vec![CellRect::new(pos, pos, col, col)]);
    }

    /// Shift: retângulo da célula âncora até (`pos`, `col`).
    fn select_cells_to(&self, pos: u32, col: usize) {
        let anchor = self.anchor.borrow().as_ref().and_then(|r| self.store.find(r)).unwrap_or(pos);
        self.set_cell_rects(vec![CellRect::new(anchor, pos, self.anchor_col.get(), col)]);
    }

    /// Ctrl: acrescenta ou tira a célula da seleção.
    fn toggle_cell(&self, pos: u32, col: usize) {
        let mut rects = self.cell_rects.borrow().clone();
        let single = CellRect::new(pos, pos, col, col);
        if let Some(i) = rects.iter().position(|r| *r == single) {
            rects.remove(i);
        } else {
            rects.push(single);
        }
        self.anchor_col.set(col);
        self.set_cell_rects(rects);
    }

    fn select_all_cells(&self) {
        let (n, ncol) = (self.store.n_items(), self.state.columns.borrow().len());
        if n > 0 && ncol > 0 {
            self.set_cell_rects(vec![CellRect::new(0, n - 1, 0, ncol - 1)]);
        }
    }

    /// Destaca (ou não) uma célula conforme a seleção. Uma célula sozinha
    /// não ganha o destaque: o contorno da célula atual já a indica.
    fn style_selected_cell(&self, cell: &gtk::Widget, pos: u32, col: usize) {
        let rects = self.cell_rects.borrow();
        let multi = rects.len() > 1 || rects.first().is_some_and(|r| r.cells() > 1);
        if multi && rects.iter().any(|r| r.contains(pos, col)) {
            cell.add_css_class("cell-selected");
        } else {
            cell.remove_css_class("cell-selected");
        }
    }

    /// Reaplica o destaque nas células visíveis (as outras ganham ao serem
    /// mostradas, no bind).
    fn refresh_cell_classes(&self) {
        let items: Vec<(gtk::Widget, u32, usize)> = self
            .cell_items
            .borrow()
            .iter()
            .filter_map(|(w, (li, col))| li.upgrade().map(|li| (w.clone(), li.position(), *col)))
            .collect();
        for (w, pos, col) in items {
            if let Some(cell) = w.parent() {
                self.style_selected_cell(&cell, pos, col);
            }
        }
    }

    /// Linhas entraram, saíram ou mudaram de ordem: as posições guardadas
    /// na seleção de células não valem mais, então ela é desfeita (uma
    /// limpeza com Backspace não pode cair em outras células).
    fn on_store_changed(&self) {
        if self.cell_rects.borrow().is_empty() {
            return;
        }
        self.cell_rects.borrow_mut().clear();
        self.refresh_cell_classes();
    }

    /// Backspace: apaga o conteúdo das células selecionadas (ou da célula
    /// atual), num único passo de desfazer.
    fn clear_selected_cells(self: &Rc<Self>) {
        if !self.can_replace() {
            return;
        }
        let mut rects = self.cell_rects.borrow().clone();
        if rects.is_empty() {
            let cur = self.cur_cell.borrow().clone();
            let Some((row, col)) = cur else { return };
            let Some(pos) = self.store.find(&row) else { return };
            rects.push(CellRect::new(pos, pos, col, col));
        }
        let ncol = self.state.columns.borrow().len();
        let mut by_row: std::collections::BTreeMap<u32, std::collections::BTreeSet<usize>> =
            Default::default();
        for r in &rects {
            for pos in r.r0..=r.r1.min(self.store.n_items().saturating_sub(1)) {
                by_row.entry(pos).or_default().extend((r.c0..=r.c1).filter(|&c| c < ncol));
            }
        }
        let changed: Vec<(u32, RowObject, Vec<(usize, String)>)> = by_row
            .into_iter()
            .filter_map(|(pos, cols)| {
                let row = self.store.item(pos).and_downcast::<RowObject>()?;
                let reps: Vec<(usize, String)> = cols
                    .into_iter()
                    .filter(|&c| !row.value(c).is_empty())
                    .map(|c| (c, String::new()))
                    .collect();
                (!reps.is_empty()).then_some((pos, row, reps))
            })
            .collect();
        let n = self.apply_replacements(changed);
        self.status.set_text(&format!("{n} {}", tr("cells_cleared")));
    }

    /// Delete: exclui as linhas selecionadas (ou a linha atual).
    fn delete_selected_rows(self: &Rc<Self>) {
        if !self.can_replace() {
            return;
        }
        let mut ids: Vec<i64> = self.selected_rows().iter().map(|r| r.rowid()).collect();
        if ids.is_empty() {
            if let Some(r) = self.cursor.borrow().as_ref() {
                ids.push(r.rowid());
            }
        }
        let n = ids.len();
        self.delete_rowids(&ids);
        self.status.set_text(&format!("{n} {}", tr("rows_deleted")));
    }

    // ---------- seleção ----------

    /// Seleciona só a linha `pos`, que vira o cursor e a âncora do Shift.
    fn select_only(&self, pos: u32) {
        self.selection.select_item(pos, true);
        let row = self.store.item(pos).and_downcast::<RowObject>();
        *self.anchor.borrow_mut() = row.clone();
        *self.cursor.borrow_mut() = row;
    }

    /// Shift: seleciona da âncora até `pos` (a âncora não muda).
    fn select_range_to(&self, pos: u32) {
        let anchor = self.anchor.borrow().as_ref().and_then(|r| self.store.find(r));
        let Some(anchor) = anchor else {
            self.select_only(pos);
            return;
        };
        let (lo, hi) = (anchor.min(pos), anchor.max(pos));
        self.selection.select_range(lo, hi - lo + 1, true);
        *self.cursor.borrow_mut() = self.store.item(pos).and_downcast();
    }

    /// Ctrl: marca ou desmarca a linha `pos`, sem mexer no resto.
    fn toggle_row(&self, pos: u32) {
        if self.selection.is_selected(pos) {
            self.selection.unselect_item(pos);
        } else {
            self.selection.select_item(pos, false);
        }
        let row = self.store.item(pos).and_downcast::<RowObject>();
        *self.anchor.borrow_mut() = row.clone();
        *self.cursor.borrow_mut() = row;
    }

    /// Posição do cursor na grade (`INVALID_LIST_POSITION` se não há).
    fn cursor_pos(&self) -> u32 {
        self.cursor
            .borrow()
            .as_ref()
            .and_then(|r| self.store.find(r))
            .unwrap_or(gtk::INVALID_LIST_POSITION)
    }

    /// Linhas selecionadas, na ordem da grade.
    fn selected_rows(&self) -> Vec<RowObject> {
        let set = self.selection.selection();
        (0..set.size())
            .filter_map(|i| self.store.item(set.nth(i as u32)).and_downcast::<RowObject>())
            .collect()
    }

    /// Seleciona as linhas `rows` (onde estiverem agora) e rola até a
    /// primeira; ela vira o cursor e a âncora.
    fn select_rows(&self, rows: &[RowObject]) {
        let wanted: std::collections::HashSet<&RowObject> = rows.iter().collect();
        let set = gtk::Bitset::new_empty();
        let mut first = None;
        for i in 0..self.store.n_items() {
            if let Some(r) = self.store.item(i).and_downcast::<RowObject>() {
                if wanted.contains(&r) {
                    set.add(i);
                    first.get_or_insert(i);
                }
            }
        }
        let n = self.store.n_items();
        self.selection.set_selection(&set, &gtk::Bitset::new_range(0, n));
        if let Some(p) = first {
            let row = self.store.item(p).and_downcast::<RowObject>();
            *self.anchor.borrow_mut() = row.clone();
            *self.cursor.borrow_mut() = row;
            self.colview.scroll_to(p, None, gtk::ListScrollFlags::FOCUS, None);
        }
    }

    /// Depois de reordenar o store: seleciona de novo as mesmas linhas e
    /// mantém o cursor à vista, sem mudar cursor e âncora.
    fn restore_selection(&self, rows: &[RowObject]) {
        let (cursor, anchor) = (self.cursor.borrow().clone(), self.anchor.borrow().clone());
        self.select_rows(rows);
        *self.cursor.borrow_mut() = cursor;
        *self.anchor.borrow_mut() = anchor;
        let pos = self.cursor_pos();
        if pos != gtk::INVALID_LIST_POSITION {
            self.colview.scroll_to(pos, None, gtk::ListScrollFlags::FOCUS, None);
        }
    }

    // ---------- copiar / colar ----------

    /// Copia as linhas selecionadas para a área de transferência como XML
    /// (`<ROW .../>`, uma por linha de texto). Devolve quantas copiou.
    fn copy_rows(&self) -> usize {
        let rows = self.selected_rows();
        if rows.is_empty() {
            return 0;
        }
        let names = self.state.columns.borrow().clone();
        let values: Vec<Vec<String>> =
            rows.iter().map(|r| (0..names.len()).map(|c| r.value(c)).collect()).collect();
        self.root.clipboard().set_text(&rowclip::rows_to_xml(&names, &values));
        self.status.set_text(&format!("{} {}", rows.len(), tr("rows_copied")));
        rows.len()
    }

    /// Recortar: copia e exclui as linhas selecionadas.
    fn cut_rows(self: &Rc<Self>) {
        if !*self.state.editable.borrow() || self.state.loading.get() {
            self.status.set_text(tr("paste_readonly"));
            return;
        }
        let ids: Vec<i64> = self.selected_rows().iter().map(|r| r.rowid()).collect();
        if self.copy_rows() > 0 {
            self.delete_rowids(&ids);
            self.status.set_text(&format!("{} {}", ids.len(), tr("rows_cut")));
        }
    }

    /// Cola as linhas da área de transferência acima/abaixo da linha
    /// `ref_rowid` (ou no fim, sem referência).
    fn paste_rows(self: &Rc<Self>, ref_rowid: Option<i64>, above: bool) {
        if !*self.state.editable.borrow() || self.state.loading.get() || self.busy.get() {
            self.status.set_text(tr("paste_readonly"));
            return;
        }
        let weak = Rc::downgrade(self);
        self.root.clipboard().read_text_async(gio::Cancellable::NONE, move |res| {
            let Some(me) = weak.upgrade() else { return };
            let text = res.ok().flatten().map(|t| t.to_string()).unwrap_or_default();
            match rowclip::parse_rows(&text) {
                Some(rows) => me.insert_pasted(ref_rowid.filter(|r| *r >= 0), above, &rows),
                None => me.status.set_text(tr("paste_nothing")),
            }
        });
    }

    fn insert_pasted(self: &Rc<Self>, ref_rowid: Option<i64>, above: bool, rows: &[Vec<(String, String)>]) {
        let (created, ignored) = match self.state.dp.insert_rows(ref_rowid, above, rows) {
            Ok(r) => r,
            Err(e) => {
                self.error(&format!("{}\n{e}", tr("error")));
                return;
            }
        };
        let objs: Vec<RowObject> =
            created.into_iter().map(|(rid, vals)| RowObject::new(rid, vals, true)).collect();
        let at = ref_rowid
            .and_then(|r| self.row_position(r))
            .map(|p| if above { p } else { p + 1 })
            .unwrap_or(self.store.n_items());
        self.store.splice(at, 0, &objs);
        if self.is_sorted() {
            self.sort_store();
        }
        self.select_rows(&objs);
        self.mark_dirty();
        let mut msg = format!("{} {}", objs.len(), tr("rows_pasted"));
        if !ignored.is_empty() {
            msg.push_str(&format!("  ·  {}: {}", tr("paste_ignored"), ignored.join(", ")));
        }
        self.status.set_text(&msg);
    }

    /// Abre o menu de contexto das células. Antes, confere se a área de
    /// transferência tem linhas para mostrar (ou não) o submenu Colar.
    fn popup_ctx_menu(self: &Rc<Self>) {
        if !*self.state.editable.borrow() {
            self.paste_section.remove_all();
            open_menu(self.menu.upcast_ref());
            return;
        }
        let weak = Rc::downgrade(self);
        self.root.clipboard().read_text_async(gio::Cancellable::NONE, move |res| {
            let Some(me) = weak.upgrade() else { return };
            let has_rows = res.ok().flatten().is_some_and(|t| t.contains("<ROW"));
            me.paste_section.remove_all();
            if has_rows {
                let sub = gio::Menu::new();
                sub.append(Some(tr("ctx_paste_above")), Some("doc.paste-above"));
                sub.append(Some(tr("ctx_paste_below")), Some("doc.paste-below"));
                me.paste_section.append_submenu(Some(tr("ctx_paste")), &sub);
            }
            open_menu(me.menu.upcast_ref());
        });
    }

    /// Tira da grade as linhas `ids`. Muitas de uma vez viram uma única troca
    /// do conteúdo do store.
    fn show_rows_removed(&self, ids: &std::collections::HashSet<i64>) {
        if ids.len() <= 20 {
            for id in ids {
                if let Some(pos) = self.row_position(*id) {
                    self.store.remove(pos);
                }
            }
        } else {
            let n = self.store.n_items();
            let kept: Vec<RowObject> = (0..n)
                .filter_map(|i| self.store.item(i).and_downcast::<RowObject>())
                .filter(|r| !ids.contains(&r.rowid()))
                .collect();
            self.store.splice(0, n, &kept);
        }
        self.refresh_status();
    }

    /// Recoloca na grade várias linhas restauradas (desfazer exclusão,
    /// refazer colagem), cada uma na sua posição do documento.
    fn show_rows_added(self: &Rc<Self>, ids: &[i64]) {
        let filter = self.state.last_filter.borrow().clone();
        let vis = match self.state.dp.visible_rows(ids, &filter) {
            Ok(v) => v,
            Err(e) => {
                self.error(&format!("{}\n{e}", tr("error")));
                return;
            }
        };
        if vis.is_empty() {
            return;
        }
        let new: Vec<RowObject> =
            vis.into_iter().map(|(rid, vals)| RowObject::new(rid, vals, true)).collect();
        let n = self.store.n_items();
        if self.is_sorted() {
            self.store.splice(n, 0, &new);
            self.sort_store();
        } else {
            // Intercala pelas posições do documento numa única troca.
            let ord = match self.state.dp.row_orders() {
                Ok(o) => o,
                Err(e) => {
                    self.error(&format!("{}\n{e}", tr("error")));
                    return;
                }
            };
            let mut all: Vec<RowObject> =
                (0..n).filter_map(|i| self.store.item(i).and_downcast::<RowObject>()).collect();
            all.extend(new.iter().cloned());
            let key = |r: &RowObject| ord.get(&r.rowid()).copied().unwrap_or(f64::MAX);
            all.sort_by(|a, b| key(a).total_cmp(&key(b)));
            self.store.splice(0, n, &all);
        }
        self.select_rows(&new);
        self.refresh_status();
    }

    fn ctx_clone_row(self: &Rc<Self>, above: bool) {
        if !*self.state.editable.borrow() || self.state.loading.get() {
            return;
        }
        let rid = self.ctx_rowid.get();
        if rid < 0 {
            return;
        }
        match self.state.dp.clone_row(rid, above) {
            Ok((new_rid, values)) => {
                self.mark_dirty();
                let obj = RowObject::new(new_rid, values, true);
                let pos = self.row_position(rid).unwrap_or(self.store.n_items());
                let at = if above { pos } else { pos + 1 };
                self.store.insert(at, &obj);
                self.refresh_status();
                self.select_only(at);
                self.colview.scroll_to(at, None, gtk::ListScrollFlags::FOCUS, None);
            }
            Err(e) => self.error(&format!("{}\n{e}", tr("error"))),
        }
    }

    fn ctx_move_row(self: &Rc<Self>, up: bool) {
        if !*self.state.editable.borrow() || self.state.loading.get() {
            return;
        }
        // Com a grade ordenada por uma coluna, a vizinha na tela não é a
        // vizinha no documento: mover trocaria a linha com outra, invisível.
        if self.is_sorted() {
            self.status.set_text(tr("move_while_sorted"));
            return;
        }
        let rid = self.ctx_rowid.get();
        if rid < 0 {
            return;
        }
        match self.state.dp.move_row(rid, up) {
            Ok(Some(_)) => {
                self.mark_dirty();
                let Some(pos) = self.row_position(rid) else {
                    return;
                };
                // reordena o store para refletir a troca (sem re-consultar)
                let target = if up { pos.wrapping_sub(1) } else { pos + 1 };
                if let Some(obj) = self.store.item(pos) {
                    self.store.remove(pos);
                    self.store.insert(target, &obj);
                    self.select_only(target);
                    self.colview
                        .scroll_to(target, None, gtk::ListScrollFlags::FOCUS, None);
                }
            }
            Ok(None) => {} // já está no topo/fundo
            Err(e) => self.error(&format!("{}\n{e}", tr("error"))),
        }
    }

    /// Abre o campo de renomear sobre o título da coluna do menu de contexto.
    fn ctx_rename_column(self: &Rc<Self>) {
        // Numa visão somente-leitura (resultado de SQL livre) as colunas
        // exibidas podem não corresponder às da tabela.
        if !*self.state.editable.borrow() || self.state.loading.get() {
            return;
        }
        let col = self.ctx_col.get();
        let name = self.state.columns.borrow().get(col).cloned();
        let anchor = self.titles.borrow().get(col).cloned();
        if let (Some(name), Some(anchor)) = (name, anchor) {
            self.open_rename_popover(&anchor, &name);
        }
    }

    fn ctx_add_column(self: &Rc<Self>) {
        if !*self.state.editable.borrow() || self.state.loading.get() {
            return;
        }
        // nome padrão único; o usuário renomeia depois (botão direito → Renomear coluna)
        let existing = self.state.columns.borrow().clone();
        let mut n = existing.len() + 1;
        let mut name = format!("coluna{n}");
        while existing.iter().any(|c| c == &name) {
            n += 1;
            name = format!("coluna{n}");
        }
        match self.state.dp.add_column(&name) {
            Ok(()) => {
                self.mark_dirty();
                // acrescenta um valor vazio em cada RowObject já carregado (sem re-query)
                let cnt = self.store.n_items();
                for i in 0..cnt {
                    if let Some(obj) = self.store.item(i) {
                        obj.downcast::<RowObject>().unwrap().push_value(String::new());
                    }
                }
                self.state.columns.borrow_mut().push(name);
                let names = self.state.columns.borrow().clone();
                self.rebuild_columns(&names);
            }
            Err(e) => self.error(&format!("{}\n{e}", tr("column_op_error"))),
        }
    }

    fn ctx_delete_column(self: &Rc<Self>) {
        if !*self.state.editable.borrow() || self.state.loading.get() {
            return;
        }
        let col = self.ctx_col.get();
        let name = self.state.columns.borrow().get(col).cloned();
        let Some(name) = name else { return };
        match self.state.dp.drop_column(&name) {
            Ok(()) => {
                self.mark_dirty();
                // remove o valor da coluna em cada RowObject já carregado (sem re-query)
                let cnt = self.store.n_items();
                for i in 0..cnt {
                    if let Some(obj) = self.store.item(i) {
                        obj.downcast::<RowObject>().unwrap().remove_value(col);
                    }
                }
                self.state.columns.borrow_mut().remove(col);
                let names = self.state.columns.borrow().clone();
                self.rebuild_columns(&names);
            }
            Err(e) => self.error(&format!("{}\n{e}", tr("column_op_error"))),
        }
    }

    /// Soma diretamente a coluna sob o cursor (item do menu de contexto).
    fn ctx_sum_column(self: &Rc<Self>) {
        let col = self.ctx_col.get();
        let name = self.state.columns.borrow().get(col).cloned();
        let Some(name) = name else { return };
        let filter = self.state.last_filter.borrow().clone();
        match self.state.dp.sum_column(&name, Some(&filter)) {
            Ok((total, count)) => {
                let body = format!(
                    "{}: {}\n({} {})",
                    tr("total"),
                    fmt_br(total),
                    count,
                    tr("numeric_values")
                );
                self.info(&format!("{} {name}", tr("sum")), &body);
            }
            Err(e) => self.error(&format!("{}\n{e}", tr("sum_failed"))),
        }
    }
}

/// Retângulo de células selecionadas: linhas `r0..=r1` × colunas `c0..=c1`
/// (posições na grade).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct CellRect {
    r0: u32,
    r1: u32,
    c0: usize,
    c1: usize,
}

impl CellRect {
    fn new(ra: u32, rb: u32, ca: usize, cb: usize) -> Self {
        Self { r0: ra.min(rb), r1: ra.max(rb), c0: ca.min(cb), c1: ca.max(cb) }
    }

    fn contains(&self, row: u32, col: usize) -> bool {
        (self.r0..=self.r1).contains(&row) && (self.c0..=self.c1).contains(&col)
    }

    fn cells(&self) -> u64 {
        u64::from(self.r1 - self.r0 + 1) * (self.c1 - self.c0 + 1) as u64
    }
}

/// Estado da seleção por arrasto (ver `DocumentView::arm_drag_select`).
#[derive(Default)]
struct DragSelect {
    /// Linha onde o arrasto começou (`None` fora das linhas: cabeçalho etc.).
    start: Cell<Option<u32>>,
    /// Coluna onde o arrasto começou.
    start_col: Cell<usize>,
    /// O arrasto já virou seleção de linhas.
    active: Cell<bool>,
    /// Última posição do ponteiro, na tabela.
    last: Cell<(f64, f64)>,
    /// Há um temporizador de rolagem rodando.
    ticking: Cell<bool>,
}

/// Popovers de submenu dentro de `w`. No modo NESTED o PopoverMenu cria um
/// popover por submenu, como filho do item que o abre.
fn sub_popovers(w: &gtk::Widget, out: &mut Vec<gtk::Popover>) {
    let mut child = w.first_child();
    while let Some(c) = child {
        if let Ok(p) = c.clone().downcast::<gtk::Popover>() {
            out.push(p);
        }
        sub_popovers(&c, out);
        child = c.next_sibling();
    }
}

/// Abre um menu de contexto. Os submenus também ficam sem autohide: com ele,
/// um submenu aberto engoliria o clique em outra célula (ver
/// `close_menus_on_outside_click`). Refeito a cada abertura porque o
/// submenu Colar é recriado conforme a área de transferência.
fn open_menu(menu: &gtk::Popover) {
    let mut subs = Vec::new();
    sub_popovers(menu.upcast_ref(), &mut subs);
    for p in subs {
        p.set_autohide(false);
    }
    menu.popup();
}

/// Fecha um menu de contexto e os submenus que estiverem abertos.
fn close_menu(menu: &gtk::Popover) {
    let mut subs = Vec::new();
    sub_popovers(menu.upcast_ref(), &mut subs);
    for p in subs {
        p.popdown();
    }
    menu.popdown();
}

/// Coleta, na ordem das colunas, os widgets de título do cabeçalho
/// (`GtkColumnViewTitle`).
fn collect_titles(w: &gtk::Widget, out: &mut Vec<gtk::Widget>) {
    if w.type_().name() == "GtkColumnViewTitle" {
        out.push(w.clone());
        return;
    }
    let mut child = w.first_child();
    while let Some(c) = child {
        collect_titles(&c, out);
        child = c.next_sibling();
    }
}

/// Formata número no padrão pt-BR: 1.234.567,89
fn fmt_br(n: f64) -> String {
    let neg = n < 0.0;
    let s = format!("{:.2}", n.abs());
    let (int_part, dec_part) = s.split_once('.').unwrap_or((s.as_str(), "00"));
    let mut grouped = String::new();
    let bytes = int_part.as_bytes();
    let len = bytes.len();
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 && (len - i) % 3 == 0 {
            grouped.push('.');
        }
        grouped.push(*b as char);
    }
    format!("{}{},{}", if neg { "-" } else { "" }, grouped, dec_part)
}

/// Um caractere faz parte de um identificador de coluna (letras, dígitos, `_`).
fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Dado o texto e a posição do cursor (em caracteres), devolve
/// (início, fim, palavra) da palavra que contém/termina no cursor.
/// Os índices são em caracteres.
fn current_word(text: &str, cursor: usize) -> (usize, usize, String) {
    let chars: Vec<char> = text.chars().collect();
    let cursor = cursor.min(chars.len());
    let mut start = cursor;
    while start > 0 && is_ident_char(chars[start - 1]) {
        start -= 1;
    }
    let mut end = cursor;
    while end < chars.len() && is_ident_char(chars[end]) {
        end += 1;
    }
    let word: String = chars[start..end].iter().collect();
    (start, end, word)
}

/// Coloca aspas duplas no nome da coluna se ele não for um identificador
/// SQL simples (ou contiver caracteres fora de `[A-Za-z0-9_]` / iniciar com dígito).
fn quote_ident(name: &str) -> String {
    let simple = !name.is_empty()
        && name.chars().next().map(|c| c.is_alphabetic() || c == '_').unwrap_or(false)
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if simple {
        name.to_string()
    } else {
        format!("\"{}\"", name.replace('"', "\"\""))
    }
}

/// Move a seleção da lista de sugestões em `delta` linhas, com clamp.
fn move_selection(listbox: &gtk::ListBox, delta: i32) {
    let cur = listbox.selected_row().map(|r| r.index()).unwrap_or(0);
    let mut idx = cur + delta;
    if idx < 0 {
        idx = 0;
    }
    // encontra o maior índice válido
    let mut last = 0;
    while listbox.row_at_index(last + 1).is_some() {
        last += 1;
    }
    if idx > last {
        idx = last;
    }
    if let Some(row) = listbox.row_at_index(idx) {
        listbox.select_row(Some(&row));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retangulo_de_celulas_normaliza_os_cantos() {
        let r = CellRect::new(7, 3, 2, 0);
        assert_eq!((r.r0, r.r1, r.c0, r.c1), (3, 7, 0, 2));
        assert!(r.contains(3, 0) && r.contains(7, 2) && r.contains(5, 1));
        assert!(!r.contains(8, 1) && !r.contains(5, 3));
        assert_eq!(r.cells(), 15);
    }
}
