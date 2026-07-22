use crate::xmldoc::{XmlDoc, QueryResult};
use crate::row_object::RowObject;
use crate::dialog;
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
    columns: RefCell<Vec<String>>, // colunas visíveis (sem _rid)
    editable: RefCell<bool>,
    dirty: RefCell<bool>,
    path: PathBuf,
    saved_path: RefCell<Option<PathBuf>>, // caminho em disco já conhecido (Salvar sobrescreve)
    last_filter: RefCell<String>, // último WHERE aplicado (para recarregar)
}

pub struct DocumentView {
    pub root: gtk::Box,
    state: Rc<State>,
    store: gio::ListStore,
    selection: gtk::SingleSelection,
    colview: gtk::ColumnView,
    status: gtk::Label,
    find_entry: gtk::SearchEntry,
    menu: gtk::PopoverMenu,
    ctx_rowid: Cell<i64>,
    ctx_col: Cell<usize>,
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

        let find_entry = gtk::SearchEntry::builder()
            .placeholder_text(tr("find_placeholder"))
            .width_chars(18)
            .build();
        bar.append(&find_entry);

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

        let scroller = gtk::ScrolledWindow::builder().vexpand(true).build();
        scroller.set_child(Some(&colview));
        root.append(&scroller);

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
        let sec_rows = gio::Menu::new();
        sec_rows.append(Some(tr("ctx_row_above")), Some("doc.row-above"));
        sec_rows.append(Some(tr("ctx_row_below")), Some("doc.row-below"));
        sec_rows.append(Some(tr("ctx_row_delete")), Some("doc.row-delete"));
        menu_model.append_section(None, &sec_rows);
        let sec_cols = gio::Menu::new();
        sec_cols.append(Some(tr("ctx_col_sum")), Some("doc.col-sum"));
        sec_cols.append(Some(tr("ctx_col_add")), Some("doc.col-add"));
        sec_cols.append(Some(tr("ctx_col_delete")), Some("doc.col-delete"));
        menu_model.append_section(None, &sec_cols);
        let menu = gtk::PopoverMenu::from_model(Some(&menu_model));
        menu.set_parent(&colview);
        menu.set_has_arrow(false);
        menu.set_halign(gtk::Align::Start);

        let me = Rc::new(Self {
            root,
            state,
            store,
            selection,
            colview,
            status,
            find_entry: find_entry.clone(),
            menu,
            ctx_rowid: Cell::new(-1),
            ctx_col: Cell::new(0),
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
        actions.add_action(&mk("col-sum", Box::new(clone!(@strong me => move || me.ctx_sum_column()))));
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

        sql_btn.connect_clicked(clone!(@strong me => move |_| me.open_sql_dialog()));
        sum_btn.connect_clicked(clone!(@strong me => move |_| me.do_sum()));
        save_btn.connect_clicked(clone!(@strong me => move |_| me.do_save()));
        csv_btn.connect_clicked(clone!(@strong me => move |_| me.do_export_csv()));

        find_entry.connect_search_changed(clone!(@strong me => move |e| {
            me.find_next(&e.text());
        }));
        find_entry.connect_activate(clone!(@strong me => move |e| {
            me.find_next(&e.text());
        }));

        // carga inicial (tabela completa)
        me.apply_filter("");
        me
    }

    fn apply_filter(self: &Rc<Self>, where_clause: &str) {
        match self.state.dp.filter(Some(where_clause)) {
            Ok(res) => {
                *self.state.editable.borrow_mut() = true;
                *self.state.last_filter.borrow_mut() = where_clause.to_string();
                self.load_result(&res, true);
                let n = res.rows.len();
                let extra = if where_clause.trim().is_empty() {
                    String::new()
                } else {
                    format!("  ·  filtro: {where_clause}")
                };
                self.status.set_text(&format!("Total: {n}{extra}"));
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
                    if col == "_rid" || col == "__ord" {
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
                self.load_result(&res, false);
                let n = res.rows.len();
                self.status
                    .set_text(&format!("Total: {n}  ·  SQL (somente leitura): {sql}"));
            }
            Err(e) => self.error(&format!("{}\n{e}", tr("sql_error"))),
        }
    }

    fn load_result(self: &Rc<Self>, res: &QueryResult, editable: bool) {
        // separa _rid das colunas visíveis
        let rid_idx = res.columns.iter().position(|c| c == "_rid");
        let visible: Vec<(usize, String)> = res
            .columns
            .iter()
            .enumerate()
            .filter(|(_, c)| *c != "_rid" && *c != "__ord")
            .map(|(i, c)| (i, c.clone()))
            .collect();
        let vis_names: Vec<String> = visible.iter().map(|(_, c)| c.clone()).collect();

        *self.state.columns.borrow_mut() = vis_names.clone();
        self.rebuild_columns(&vis_names);

        let objs: Vec<RowObject> = res
            .rows
            .iter()
            .map(|row| {
                let rowid = rid_idx
                    .and_then(|i| row.get(i))
                    .and_then(|s| s.parse::<i64>().ok())
                    .unwrap_or(-1);
                let values: Vec<String> = visible.iter().map(|(i, _)| row[*i].clone()).collect();
                RowObject::new(rowid, values, editable)
            })
            .collect();
        // Uma única operação em lote em vez de 50k append (evita custo quadrático).
        self.store.splice(0, self.store.n_items(), &objs);
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
            let me = self.clone();
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
                    g.set_state(gtk::EventSequenceState::Claimed);
                    if let Some(item) = li_ctx.item() {
                        if let Ok(row) = item.downcast::<RowObject>() {
                            me_ctx.ctx_rowid.set(row.rowid());
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
                    me_ctx.menu.popup();
                });
                label.add_controller(gesture);

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
                    if *me2.state.editable.borrow() && row.rowid() >= 0 {
                        let col_name = me2.state.columns.borrow()[col_idx].clone();
                        if let Err(e) = me2.state.dp.update_cell(row.rowid(), &col_name, &new_val) {
                            me2.error(&format!("{}\n{e}", tr("cell_write_error")));
                        } else {
                            *me2.state.dirty.borrow_mut() = true;
                        }
                    }
                });
            });

            let me_bind = self.clone();
            factory.connect_bind(move |_, list_item| {
                let list_item = list_item.downcast_ref::<gtk::ListItem>().unwrap();
                let row = list_item.item().unwrap().downcast::<RowObject>().unwrap();
                let label = list_item
                    .child()
                    .unwrap()
                    .downcast::<gtk::EditableLabel>()
                    .unwrap();
                label.set_text(&row.value(col_idx));
                label.set_editable(*me_bind.state.editable.borrow() && row.editable());
            });

            let column = gtk::ColumnViewColumn::new(Some(name), Some(factory));
            column.set_resizable(true);
            self.colview.append_column(&column);
        }

        // arma rename inline ao dar duplo-clique no cabeçalho de cada coluna
        self.arm_header_rename();
    }

    /// Após (re)criar as colunas, percorre a árvore de widgets para localizar os
    /// títulos de cabeçalho e arma o rename inline (duplo-clique → campo editável).
    fn arm_header_rename(self: &Rc<Self>) {
        let me = self.clone();
        let colview = self.colview.clone();
        glib::idle_add_local_once(move || {
            arm_titles(&colview.clone().upcast::<gtk::Widget>(), &me);
        });
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
                    *me.state.dirty.borrow_mut() = true;
                    me.reload();
                }
                Err(err) => me.error(&format!("{}\n{err}", tr("column_op_error"))),
            }
        });
        pop.connect_closed(|p| p.unparent());
        pop.popup();
        entry.grab_focus();
    }

    fn find_next(&self, needle: &str) {
        let needle = needle.to_lowercase();
        if needle.is_empty() {
            return;
        }
        let n = self.store.n_items();
        if n == 0 {
            return;
        }
        let start = self.selection.selected();
        let start = if start == gtk::INVALID_LIST_POSITION { 0 } else { start + 1 };
        for off in 0..n {
            let pos = (start + off) % n;
            if let Some(obj) = self.store.item(pos) {
                let row = obj.downcast::<RowObject>().unwrap();
                let ncol = self.state.columns.borrow().len();
                let hit = (0..ncol).any(|c| row.value(c).to_lowercase().contains(&needle));
                if hit {
                    self.selection.set_selected(pos);
                    self.colview
                        .scroll_to(pos, None, gtk::ListScrollFlags::FOCUS, None);
                    return;
                }
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

    /// Recarrega a visão atual (mantendo o último filtro) após mudança estrutural.
    fn reload(self: &Rc<Self>) {
        let where_clause = self.state.last_filter.borrow().clone();
        self.apply_filter(&where_clause);
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
        if !*self.state.editable.borrow() {
            return; // visão somente leitura (SQL livre)
        }
        let rid = self.ctx_rowid.get();
        if rid < 0 {
            return;
        }
        match self.state.dp.insert_row_relative(rid, above) {
            Ok(new_rid) => {
                *self.state.dirty.borrow_mut() = true;
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
        if !*self.state.editable.borrow() {
            return;
        }
        let rid = self.ctx_rowid.get();
        if rid < 0 {
            return;
        }
        match self.state.dp.delete_row(rid) {
            Ok(()) => {
                *self.state.dirty.borrow_mut() = true;
                // remove apenas a linha do store (sem re-consultar)
                if let Some(pos) = self.row_position(rid) {
                    self.store.remove(pos);
                }
                self.refresh_status();
            }
            Err(e) => self.error(&format!("{}\n{e}", tr("error"))),
        }
    }

    fn ctx_add_column(self: &Rc<Self>) {
        if !*self.state.editable.borrow() {
            return;
        }
        // nome padrão único; o usuário renomeia inline depois (duplo-clique no cabeçalho)
        let existing = self.state.columns.borrow().clone();
        let mut n = existing.len() + 1;
        let mut name = format!("coluna{n}");
        while existing.iter().any(|c| c == &name) {
            n += 1;
            name = format!("coluna{n}");
        }
        match self.state.dp.add_column(&name) {
            Ok(()) => {
                *self.state.dirty.borrow_mut() = true;
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
        if !*self.state.editable.borrow() {
            return;
        }
        let col = self.ctx_col.get();
        let name = self.state.columns.borrow().get(col).cloned();
        let Some(name) = name else { return };
        match self.state.dp.drop_column(&name) {
            Ok(()) => {
                *self.state.dirty.borrow_mut() = true;
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

/// Percorre recursivamente os widgets de cabeçalho (`GtkColumnViewTitle`) e
/// arma o duplo-clique para edição inline do nome da coluna.
fn arm_titles(w: &gtk::Widget, me: &Rc<DocumentView>) {
    if w.type_().name() == "GtkColumnViewTitle" {
        let title = w.clone();
        let me2 = me.clone();
        let gesture = gtk::GestureClick::new();
        gesture.set_button(gdk::BUTTON_PRIMARY);
        gesture.connect_pressed(move |_, n_press, _, _| {
            if n_press != 2 {
                return;
            }
            let name = label_text(&title).unwrap_or_default();
            me2.open_rename_popover(&title, &name);
        });
        w.add_controller(gesture);
    }
    let mut child = w.first_child();
    while let Some(c) = child {
        arm_titles(&c, me);
        child = c.next_sibling();
    }
}

/// Retorna o texto do primeiro `GtkLabel` descendente (o nome exibido da coluna).
fn label_text(w: &gtk::Widget) -> Option<String> {
    if let Ok(lbl) = w.clone().downcast::<gtk::Label>() {
        return Some(lbl.text().to_string());
    }
    let mut child = w.first_child();
    while let Some(c) = child {
        if let Some(t) = label_text(&c) {
            return Some(t);
        }
        child = c.next_sibling();
    }
    None
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
