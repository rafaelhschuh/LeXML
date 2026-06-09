use crate::xmldoc::{XmlDoc, QueryResult};
use crate::row_object::RowObject;
use adw::prelude::*;
use gtk::glib::{self, clone};
use gtk::gio;
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

/// Estado compartilhado de um documento aberto.
struct State {
    dp: XmlDoc,
    columns: RefCell<Vec<String>>, // colunas visíveis (sem _rid)
    editable: RefCell<bool>,
    dirty: RefCell<bool>,
    path: PathBuf,
}

pub struct DocumentView {
    pub root: gtk::Box,
    state: Rc<State>,
    store: gio::ListStore,
    selection: gtk::SingleSelection,
    colview: gtk::ColumnView,
    col_dropdown: gtk::DropDown,
    status: gtk::Label,
    find_entry: gtk::SearchEntry,
}

impl DocumentView {
    pub fn new(dp: XmlDoc, path: PathBuf) -> Rc<Self> {
        let state = Rc::new(State {
            dp,
            columns: RefCell::new(Vec::new()),
            editable: RefCell::new(true),
            dirty: RefCell::new(false),
            path,
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
            .placeholder_text("Localizar…")
            .width_chars(18)
            .build();
        bar.append(&find_entry);

        let filter_entry = gtk::Entry::builder()
            .placeholder_text("Filtrar (SQL WHERE) — ex: valor <> '0.00'")
            .hexpand(true)
            .build();
        bar.append(&filter_entry);

        let filter_btn = gtk::Button::from_icon_name("system-search-symbolic");
        filter_btn.set_tooltip_text(Some("Aplicar filtro (WHERE)"));
        bar.append(&filter_btn);

        let sql_btn = gtk::Button::with_label("SQL…");
        sql_btn.set_tooltip_text(Some("Consulta SQL completa (somente leitura)"));
        bar.append(&sql_btn);

        let col_dropdown = gtk::DropDown::from_strings(&[]);
        col_dropdown.set_tooltip_text(Some("Coluna para somar"));
        bar.append(&col_dropdown);

        let sum_btn = gtk::Button::with_label("Σ Somar");
        sum_btn.set_tooltip_text(Some("Somar a coluna selecionada"));
        bar.append(&sum_btn);

        let save_btn = gtk::Button::from_icon_name("document-save-symbolic");
        save_btn.set_tooltip_text(Some("Salvar como .xml"));
        bar.append(&save_btn);

        let csv_btn = gtk::Button::from_icon_name("x-office-spreadsheet-symbolic");
        csv_btn.set_tooltip_text(Some("Exportar CSV"));
        bar.append(&csv_btn);

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

        let me = Rc::new(Self {
            root,
            state,
            store,
            selection,
            colview,
            col_dropdown,
            status,
            find_entry: find_entry.clone(),
        });

        // ---------- conexões ----------
        let apply_filter = clone!(@strong me, @weak filter_entry => move || {
            me.apply_filter(&filter_entry.text());
        });
        filter_btn.connect_clicked(clone!(@strong apply_filter => move |_| apply_filter()));
        filter_entry.connect_activate(clone!(@strong apply_filter => move |_| apply_filter()));

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
                self.load_result(&res, true);
                let n = res.rows.len();
                let extra = if where_clause.trim().is_empty() {
                    String::new()
                } else {
                    format!("  ·  filtro: {where_clause}")
                };
                self.status.set_text(&format!("Total: {n}{extra}"));
            }
            Err(e) => self.error(&format!("Erro no filtro SQL:\n{e}")),
        }
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
            Err(e) => self.error(&format!("Erro SQL:\n{e}")),
        }
    }

    fn load_result(self: &Rc<Self>, res: &QueryResult, editable: bool) {
        // separa _rid das colunas visíveis
        let rid_idx = res.columns.iter().position(|c| c == "_rid");
        let visible: Vec<(usize, String)> = res
            .columns
            .iter()
            .enumerate()
            .filter(|(i, _)| Some(*i) != rid_idx)
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

        // dropdown de soma
        let strs: Vec<&str> = vis_names.iter().map(|s| s.as_str()).collect();
        self.col_dropdown
            .set_model(Some(&gtk::StringList::new(&strs)));
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
                let label = gtk::EditableLabel::new("");
                label.set_margin_start(4);
                label.set_margin_end(4);
                label.set_max_width_chars(60);
                list_item.set_child(Some(&label));

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
                            me2.error(&format!("Erro ao gravar célula:\n{e}"));
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

    fn do_sum(&self) {
        let model = match self.col_dropdown.model() {
            Some(m) => m,
            None => return,
        };
        let sel = self.col_dropdown.selected();
        if sel == gtk::INVALID_LIST_POSITION {
            return;
        }
        let col = model
            .item(sel)
            .and_then(|o| o.downcast::<gtk::StringObject>().ok())
            .map(|s| s.string().to_string());
        let Some(col) = col else { return };
        match self.state.dp.sum_column(&col) {
            Ok((total, count)) => {
                let body = format!("Total: {}\n({} valor(es) numérico(s))", fmt_br(total), count);
                self.info(&format!("Soma de {col}"), &body);
            }
            Err(e) => self.error(&format!("Não foi possível somar:\n{e}")),
        }
    }

    fn open_sql_dialog(self: &Rc<Self>) {
        let win = self.window();
        let dialog = adw::MessageDialog::new(
            win.as_ref(),
            Some("Consulta SQL"),
            Some("Tabela: dados — resultado somente leitura."),
        );
        let entry = gtk::Entry::builder()
            .text("SELECT * FROM dados")
            .hexpand(true)
            .build();
        dialog.set_extra_child(Some(&entry));
        dialog.add_responses(&[("cancel", "Cancelar"), ("run", "Executar")]);
        dialog.set_default_response(Some("run"));
        dialog.set_response_appearance("run", adw::ResponseAppearance::Suggested);
        let me = self.clone();
        dialog.connect_response(None, move |_, resp| {
            if resp == "run" {
                me.run_sql(&entry.text());
            }
        });
        dialog.present();
    }

    fn do_save(self: &Rc<Self>) {
        self.save_then(|_ok| {});
    }

    /// `true` se há edições não salvas em disco.
    pub fn is_dirty(&self) -> bool {
        *self.state.dirty.borrow()
    }

    /// Abre o seletor de salvamento. `after` é chamado quando o diálogo fecha,
    /// com `true` se o arquivo foi gravado com sucesso (e `false` se o usuário
    /// cancelou ou houve erro) — usado para encadear o fechamento da janela.
    pub fn save_then(self: &Rc<Self>, after: impl FnOnce(bool) + 'static) {
        let dialog = gtk::FileDialog::builder()
            .title("Salvar como .xml")
            .initial_name(
                self.state
                    .path
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
                    match me.state.dp.save(&path) {
                        Ok(()) => {
                            *me.state.dirty.borrow_mut() = false;
                            me.status.set_text(&format!("Salvo em {}", path.display()));
                            ok = true;
                        }
                        Err(e) => me.error(&format!("Erro ao salvar:\n{e}")),
                    }
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
                self.error(&format!("Erro ao montar CSV:\n{e}"));
                return;
            }
        };
        let dialog = gtk::FileDialog::builder()
            .title("Exportar CSV")
            .initial_name("export.csv")
            .build();
        let me = self.clone();
        dialog.save(self.window().as_ref(), gio::Cancellable::NONE, move |r| {
            if let Ok(file) = r {
                if let Some(path) = file.path() {
                    match me.state.dp.export_csv(&path, &res) {
                        Ok(()) => me.status.set_text(&format!("CSV exportado: {}", path.display())),
                        Err(e) => me.error(&format!("Erro ao exportar CSV:\n{e}")),
                    }
                }
            }
        });
    }

    fn window(&self) -> Option<gtk::Window> {
        self.root.root().and_then(|r| r.downcast::<gtk::Window>().ok())
    }

    fn info(&self, heading: &str, body: &str) {
        let d = adw::MessageDialog::new(self.window().as_ref(), Some(heading), Some(body));
        d.add_response("ok", "OK");
        d.present();
    }

    fn error(&self, body: &str) {
        self.info("Erro", body);
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
