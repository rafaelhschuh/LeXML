use crate::xmldoc::{Change, XmlDoc, QueryResult};
use crate::row_object::RowObject;
use crate::dialog;
use crate::replace::Matcher;
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
    // Ordenação da grade pelo cabeçalho: (coluna, crescente). `None` é a
    // ordem do documento. Só muda a exibição — o arquivo salvo mantém a
    // ordem das linhas.
    sort: RefCell<Option<(String, bool)>>,
    // Ordem original de um resultado SQL (somente leitura), guardada ao
    // ordená-lo, para o terceiro clique poder voltar a ela.
    unsorted: RefCell<Option<Vec<RowObject>>>,
}

pub struct DocumentView {
    pub root: gtk::Box,
    state: Rc<State>,
    store: gio::ListStore,
    selection: gtk::SingleSelection,
    colview: gtk::ColumnView,
    status: gtk::Label,
    find_entry: gtk::SearchEntry,
    toolbar: gtk::Box,
    replace_revealer: gtk::Revealer,
    // Camada de "trabalhando…" sobre a tabela durante operações longas.
    busy: Cell<bool>,
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
        let selection = gtk::SingleSelection::new(Some(store.clone()));
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
        root.append(&status);

        // ---------- menu de contexto (botão direito) ----------
        let menu_model = gio::Menu::new();

        // Submenu "Linha": inserir / clonar / mover / excluir
        let rows_menu = gio::Menu::new();
        let sec_insert = gio::Menu::new();
        sec_insert.append(Some(tr("ctx_row_above")), Some("doc.row-above"));
        sec_insert.append(Some(tr("ctx_row_below")), Some("doc.row-below"));
        rows_menu.append_section(None, &sec_insert);
        let sec_clone = gio::Menu::new();
        sec_clone.append(Some(tr("ctx_row_clone_above")), Some("doc.row-clone-above"));
        sec_clone.append(Some(tr("ctx_row_clone_below")), Some("doc.row-clone-below"));
        rows_menu.append_section(None, &sec_clone);
        let sec_move = gio::Menu::new();
        sec_move.append(Some(tr("ctx_row_move_up")), Some("doc.row-move-up"));
        sec_move.append(Some(tr("ctx_row_move_down")), Some("doc.row-move-down"));
        rows_menu.append_section(None, &sec_move);
        let sec_row_del = gio::Menu::new();
        sec_row_del.append(Some(tr("ctx_row_delete")), Some("doc.row-delete"));
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
        sec_quick.append(Some(tr("ctx_col_sum")), Some("doc.col-sum"));
        menu_model.append_section(None, &sec_quick);

        // NESTED: cada submenu abre em seu próprio popover, dimensionado ao
        // conteúdo (evita o scroll do modo deslizante, que não recalcula altura).
        let menu = gtk::PopoverMenu::from_model_full(&menu_model, gtk::PopoverMenuFlags::NESTED);
        menu.set_parent(&colview);
        menu.set_has_arrow(false);
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
        header_menu.set_halign(gtk::Align::Start);

        let me = Rc::new(Self {
            root,
            state,
            store,
            selection,
            colview,
            status,
            find_entry: find_entry.clone(),
            toolbar: bar.clone(),
            replace_revealer: replace_revealer.clone(),
            busy: Cell::new(false),
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

        // Ctrl+S salva (sobrescreve o arquivo atual); Ctrl+Shift+S abre "Salvar como".
        // Ctrl+Z desfaz; Ctrl+Shift+Z e Ctrl+Y refazem.
        let save_keys = gtk::EventControllerKey::new();
        save_keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let me_keys = me.clone();
        save_keys.connect_key_pressed(move |_, keyval, _code, state| {
            // Durante uma operação longa o teclado fica bloqueado: salvar ou
            // desfazer no meio dela deixaria o documento pela metade.
            if me_keys.busy.get() {
                return glib::Propagation::Stop;
            }
            let ctrl = state.contains(gdk::ModifierType::CONTROL_MASK);
            let shift = state.contains(gdk::ModifierType::SHIFT_MASK);
            let undo = ctrl && !shift && matches!(keyval, gdk::Key::z | gdk::Key::Z);
            let redo = ctrl
                && (matches!(keyval, gdk::Key::y | gdk::Key::Y)
                    || (shift && matches!(keyval, gdk::Key::z | gdk::Key::Z)));
            // Num campo de texto (célula em edição, filtro, localizar) o atalho
            // fica com o próprio campo, que tem seu desfazer de texto.
            if (undo || redo) && !me_keys.text_has_focus() {
                if undo {
                    me_keys.do_undo();
                } else {
                    me_keys.do_redo();
                }
                return glib::Propagation::Stop;
            }
            // Ctrl+F vai para Localizar; Ctrl+H abre/fecha a barra de Substituir.
            if ctrl && matches!(keyval, gdk::Key::f | gdk::Key::F) {
                me_keys.find_entry.grab_focus();
                return glib::Propagation::Stop;
            }
            if ctrl && matches!(keyval, gdk::Key::h | gdk::Key::H) {
                me_keys.toggle_replace_bar();
                return glib::Propagation::Stop;
            }
            if ctrl && matches!(keyval, gdk::Key::s | gdk::Key::S) {
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

        undo_btn.connect_clicked(clone!(@strong me => move |_| me.do_undo()));
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
                                me_key.selection.set_selected(pos - 1);
                            } else if keyval == gdk::Key::Down
                                && pos + 1 < me_key.store.n_items()
                            {
                                me_key.selection.set_selected(pos + 1);
                            }
                            return glib::Propagation::Stop;
                        }
                        return glib::Propagation::Proceed;
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
                            
                            // Sincroniza o destaque da linha com o novo foco da célula
                            let pos = li_key.position();
                            if keyval == gdk::Key::Up && pos > 0 {
                                me_key.selection.set_selected(pos - 1);
                            } else if keyval == gdk::Key::Down && pos + 1 < me_key.store.n_items() {
                                me_key.selection.set_selected(pos + 1);
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
                    // seleciona a linha clicada para deixar claro o alvo da ação
                    let pos = li_ctx.position();
                    if pos != gtk::INVALID_LIST_POSITION {
                        me_ctx.selection.set_selected(pos);
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
                    me_ctx.menu.popup();
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
                    if me_sel.selection.selected() != pos {
                        // 1º clique: seleciona e foca a célula, mas bloqueia a edição
                        me_sel.selection.set_selected(pos);
                        lbl.grab_focus();
                        g.set_state(gtk::EventSequenceState::Claimed);
                    }
                    // linha já selecionada: não reivindica → EditableLabel edita
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
            });

            let column = gtk::ColumnViewColumn::new(Some(name), Some(factory));
            column.set_resizable(true);
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
            me.header_menu.popup();
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
        let selected = self.selection.selected_item().and_downcast::<RowObject>();
        self.store.splice(0, n, &sorted);
        if let Some(sel) = selected {
            if let Some(pos) = sorted.iter().position(|r| *r == sel) {
                self.selection.set_selected(pos as u32);
                self.colview.scroll_to(pos as u32, None, gtk::ListScrollFlags::FOCUS, None);
            }
        }
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
        let start = self.selection.selected();
        let start = if start == gtk::INVALID_LIST_POSITION { 0 } else { start + 1 };
        for off in 0..n {
            let pos = (start + off) % n;
            if let Some(obj) = self.store.item(pos) {
                let row = obj.downcast::<RowObject>().unwrap();
                if cols.iter().any(|&c| m.matches(&row.value(c))) {
                    self.selection.set_selected(pos);
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
        let pos = self.selection.selected();
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

    /// Reflete na grade substituições já gravadas. Muitas linhas viram um
    /// único aviso de mudança, em vez de um por linha.
    fn show_replacements(&self, changed: Vec<(u32, RowObject, Vec<(usize, String)>)>) {
        // Conta linhas distintas: o desfazer traz uma entrada por célula.
        let rows: std::collections::HashSet<u32> = changed.iter().map(|(p, _, _)| *p).collect();
        let many = rows.len() > 500;
        for (pos, row, reps) in changed {
            for (c, v) in reps {
                row.set_value(c, v);
            }
            if !many {
                self.store.items_changed(pos, 1, 1);
            }
        }
        if many {
            let sel = self.selection.selected();
            let n = self.store.n_items();
            self.store.items_changed(0, n, n);
            if sel != gtk::INVALID_LIST_POSITION {
                self.selection.set_selected(sel);
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
                for (pos, row, reps) in changed {
                    for (c, v) in reps {
                        row.set_value(c, v);
                    }
                    self.store.items_changed(pos, 1, 1);
                }
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
        match self.state.dp.save(path) {
            Ok(()) => {
                crate::recent::add(path);
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
    fn mark_dirty(&self) {
        *self.state.dirty.borrow_mut() = true;
        self.refresh_history_buttons();
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
                    // força o rebind da linha para a célula mostrar o valor
                    self.store.items_changed(pos, 1, 1);
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
            self.selection.set_selected(pos);
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
                self.selection.set_selected(at);
                self.colview.scroll_to(at, None, gtk::ListScrollFlags::FOCUS, None);
            }
            Err(e) => self.error(&format!("{}\n{e}", tr("error"))),
        }
    }

    fn ctx_delete_row(self: &Rc<Self>) {
        if !*self.state.editable.borrow() || self.state.loading.get() {
            return;
        }
        let rid = self.ctx_rowid.get();
        if rid < 0 {
            return;
        }
        match self.state.dp.delete_row(rid) {
            Ok(()) => {
                self.mark_dirty();
                // remove apenas a linha do store (sem re-consultar)
                if let Some(pos) = self.row_position(rid) {
                    self.store.remove(pos);
                }
                self.refresh_status();
            }
            Err(e) => self.error(&format!("{}\n{e}", tr("error"))),
        }
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
                self.selection.set_selected(at);
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
                    self.selection.set_selected(target);
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
