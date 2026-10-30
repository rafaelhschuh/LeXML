use anyhow::{anyhow, bail, Context, Result};
use quick_xml::events::Event;
use quick_xml::reader::Reader;
use rusqlite::types::Value;
use rusqlite::{Connection, OptionalExtension};
use std::cell::RefCell;
use std::io::{Read, Write};
use std::path::Path;

/// Quanto do início do arquivo é lido como texto cru para recuperar a moldura
/// do documento (`<DATAPACKET>` e `<PARAMS>`). A seção METADATA fica sempre no
/// começo, então 1 MiB é folgado — e o teto impede que um arquivo enorme seja
/// carregado inteiro na memória só para isso.
const HEADER_SCAN_BYTES: u64 = 1 << 20;

/// Um <FIELD> do METADATA, com todos os atributos crus preservados para reescrita fiel.
pub struct Field {
    pub name: String,
    pub attrs: Vec<(String, String)>,
}

pub struct XmlDoc {
    pub fields: RefCell<Vec<Field>>,
    /// Tag de abertura `<DATAPACKET ...>` do arquivo original, preservada para
    /// reescrita fiel (Version e demais atributos).
    datapacket_open: String,
    /// Seção `<PARAMS.../>` (ou `<PARAMS>...</PARAMS>`) do original, preservada
    /// literalmente — o app não interpreta seu conteúdo, apenas o repassa.
    params_xml: String,
    db: Connection,
    /// Chave primária interna (equivale ao rowid). O nome é escolhido para não
    /// colidir com nenhum campo do arquivo: um XML pode legitimamente trazer
    /// uma coluna chamada `__rid`, `__ord` ou até `rowid`.
    rid_col: String,
    /// Coluna interna de ordenação das linhas.
    ord_col: String,
    /// Apelido do `rid_col` nos resultados de `filter()`, também livre de colisão.
    rid_alias: String,
}

/// Cria um Field padrão (string, largura 40) com o nome dado.
fn default_field(name: &str) -> Field {
    Field {
        name: name.to_string(),
        attrs: vec![
            ("attrname".to_string(), name.to_string()),
            ("fieldtype".to_string(), "string".to_string()),
            ("WIDTH".to_string(), "40".to_string()),
        ],
    }
}

/// Resultado de uma consulta: nomes das colunas + linhas (valores como String, "" para NULL).
pub struct QueryResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

impl XmlDoc {
    pub fn open(path: &Path) -> Result<Self> {
        // Lê só o começo do arquivo para preservar a moldura do documento
        // (tag <DATAPACKET> e seção <PARAMS>) sem depender do modelo de eventos.
        let head = read_header(path)?;
        let datapacket_open =
            extract_open_tag(&head, "DATAPACKET").unwrap_or_else(|| DEFAULT_DATAPACKET_OPEN.into());
        let params_xml = extract_params(&head).unwrap_or_else(|| DEFAULT_PARAMS.into());
        drop(head);

        let mut reader = Reader::from_file(path)
            .with_context(|| format!("abrindo {}", path.display()))?;
        reader.config_mut().trim_text(false);

        let mut fields: Vec<Field> = Vec::new();
        let mut field_names: Vec<String> = Vec::new();
        let mut in_fields = false;

        let db = open_db()?;

        let mut rid_col = String::new();
        let mut ord_col = String::new();
        let mut table_ready = false;
        let mut buf = Vec::new();
        let mut next_ord = 1.0_f64;

        // As linhas são inseridas à medida que o parser as produz: nada de
        // acumular o arquivo inteiro na memória antes do primeiro INSERT.
        let tx = db.unchecked_transaction()?;
        let mut stmt: Option<rusqlite::Statement> = None;

        loop {
            match reader.read_event_into(&mut buf) {
                Err(e) => return Err(anyhow!("erro de parsing XML: {e}")),
                Ok(Event::Eof) => break,
                Ok(Event::Start(ref e)) if e.name().as_ref() == b"FIELDS" => {
                    in_fields = true;
                }
                Ok(Event::End(ref e)) if e.name().as_ref() == b"FIELDS" => {
                    in_fields = false;
                }
                Ok(ev @ (Event::Empty(_) | Event::Start(_))) => {
                    let e = match &ev {
                        Event::Empty(e) | Event::Start(e) => e,
                        _ => unreachable!(),
                    };
                    let tag = e.name();
                    if in_fields && tag.as_ref() == b"FIELD" {
                        let mut attrs = Vec::new();
                        let mut name = String::new();
                        for a in e.attributes().flatten() {
                            let key = String::from_utf8_lossy(a.key.as_ref()).to_string();
                            // Chaves que não são nomes XML válidos seriam
                            // reescritas cruas na gravação e quebrariam o
                            // arquivo de saída; descarta-as.
                            if !is_valid_xml_name(&key) {
                                continue;
                            }
                            let val = a.unescape_value()?.to_string();
                            if key == "attrname" {
                                name = val.clone();
                            }
                            attrs.push((key, val));
                        }
                        if !name.is_empty() {
                            // O nome vira nome de atributo na gravação. Se não
                            // for um Name XML válido, gravar produziria um
                            // arquivo malformado (ou permitiria injetar
                            // atributos/elementos), então o documento é
                            // recusado em vez de corrompido depois.
                            if !is_valid_xml_name(&name) {
                                bail!(
                                    "campo com nome inválido para XML: {:?} \
                                     (só letras, dígitos, '_', '-', '.' e ':', começando por letra ou '_')",
                                    name
                                );
                            }
                            if field_names.iter().any(|n| n.eq_ignore_ascii_case(&name)) {
                                bail!("campo duplicado no METADATA: {name:?}");
                            }
                            field_names.push(name.clone());
                            fields.push(Field { name, attrs });
                        }
                    } else if tag.as_ref() == b"ROW" {
                        if !table_ready {
                            rid_col = unique_name("__rid", &field_names);
                            ord_col = unique_name("__ord", &field_names);
                            create_table(&tx, &rid_col, &field_names, &ord_col)?;
                            stmt = Some(tx.prepare(&build_insert(&field_names, &ord_col))?);
                            table_ready = true;
                        }
                        let mut vals: Vec<Value> = vec![Value::Null; field_names.len()];
                        for a in e.attributes().flatten() {
                            let key = String::from_utf8_lossy(a.key.as_ref());
                            if let Some(idx) = field_names.iter().position(|n| n == key.as_ref()) {
                                vals[idx] = Value::Text(a.unescape_value()?.to_string());
                            }
                        }
                        vals.push(Value::Real(next_ord));
                        next_ord += 1.0;
                        if let Some(s) = stmt.as_mut() {
                            s.execute(rusqlite::params_from_iter(vals))?;
                        }
                    }
                }
                _ => {}
            }
            buf.clear();
        }

        drop(stmt);
        if field_names.is_empty() {
            return Err(anyhow!("XML não reconhecido: faltam as seções METADATA/FIELDS"));
        }
        if !table_ready {
            rid_col = unique_name("__rid", &field_names);
            ord_col = unique_name("__ord", &field_names);
            create_table(&tx, &rid_col, &field_names, &ord_col)?;
        }
        tx.commit()?;

        let rid_alias = unique_name("_rid", &field_names);
        Ok(Self {
            fields: RefCell::new(fields),
            datapacket_open,
            params_xml,
            db,
            rid_col,
            ord_col,
            rid_alias,
        })
    }

    /// Cria um documento em branco com uma coluna inicial e nenhuma linha.
    pub fn new_empty() -> Result<Self> {
        let db = open_db()?;
        let names = vec!["coluna1".to_string()];
        create_table(&db, "__rid", &names, "__ord")?;
        Ok(Self {
            fields: RefCell::new(vec![default_field("coluna1")]),
            datapacket_open: DEFAULT_DATAPACKET_OPEN.into(),
            params_xml: DEFAULT_PARAMS.into(),
            db,
            rid_col: "__rid".into(),
            ord_col: "__ord".into(),
            rid_alias: "_rid".into(),
        })
    }

    /// Nome da coluna de rowid nos resultados de `filter()`.
    pub fn rid_alias(&self) -> &str {
        &self.rid_alias
    }

    /// `true` se `name` é uma coluna de controle interna, que não deve ser
    /// exibida nem exportada.
    pub fn is_internal_col(&self, name: &str) -> bool {
        name == self.rid_alias
            || name.eq_ignore_ascii_case(&self.rid_col)
            || name.eq_ignore_ascii_case(&self.ord_col)
    }

    fn q_rid(&self) -> String {
        quote_ident(&self.rid_col)
    }

    fn q_ord(&self) -> String {
        quote_ident(&self.ord_col)
    }

    pub fn field_names(&self) -> Vec<String> {
        self.fields.borrow().iter().map(|f| f.name.clone()).collect()
    }

    /// Adiciona uma nova coluna (TEXT) ao final.
    pub fn add_column(&self, name: &str) -> Result<()> {
        let name = name.trim();
        check_column_name(name)?;
        if self.name_taken(name) {
            bail!("já existe uma coluna com esse nome");
        }
        self.db
            .execute(&format!("ALTER TABLE dados ADD COLUMN {} TEXT", quote_ident(name)), [])?;
        self.fields.borrow_mut().push(default_field(name));
        Ok(())
    }

    /// Renomeia uma coluna existente.
    pub fn rename_column(&self, old: &str, new: &str) -> Result<()> {
        let new = new.trim();
        if old == new {
            return Ok(());
        }
        check_column_name(new)?;
        if self.name_taken(new) {
            bail!("já existe uma coluna com esse nome");
        }
        self.db.execute(
            &format!(
                "ALTER TABLE dados RENAME COLUMN {} TO {}",
                quote_ident(old),
                quote_ident(new)
            ),
            [],
        )?;
        if let Some(f) = self.fields.borrow_mut().iter_mut().find(|f| f.name == old) {
            f.name = new.to_string();
            for (k, v) in f.attrs.iter_mut() {
                if k == "attrname" {
                    *v = new.to_string();
                }
            }
        }
        Ok(())
    }

    /// `true` se o nome já é usado por um campo ou por uma coluna interna
    /// (comparação sem distinguir maiúsculas, como o SQLite faz).
    fn name_taken(&self, name: &str) -> bool {
        name.eq_ignore_ascii_case(&self.rid_col)
            || name.eq_ignore_ascii_case(&self.ord_col)
            || self
                .fields
                .borrow()
                .iter()
                .any(|f| f.name.eq_ignore_ascii_case(name))
    }

    /// Exclui uma coluna (mantém ao menos uma).
    pub fn drop_column(&self, name: &str) -> Result<()> {
        if self.fields.borrow().len() <= 1 {
            bail!("o documento precisa de ao menos uma coluna");
        }
        self.db
            .execute(&format!("ALTER TABLE dados DROP COLUMN {}", quote_ident(name)), [])?;
        self.fields.borrow_mut().retain(|f| f.name != name);
        Ok(())
    }

    /// Insere uma linha em branco com a ordem dada. Retorna o rowid criado.
    fn insert_blank(&self, ord: f64) -> Result<i64> {
        let names = self.field_names();
        if names.is_empty() {
            bail!("documento sem colunas");
        }
        let mut cols: Vec<String> = names.iter().map(|n| quote_ident(n)).collect();
        cols.push(self.q_ord());
        let mut vals: Vec<Value> = names.iter().map(|_| Value::Text(String::new())).collect();
        vals.push(Value::Real(ord));
        let ph: Vec<String> = (1..=cols.len()).map(|i| format!("?{i}")).collect();
        self.db.execute(
            &format!("INSERT INTO dados ({}) VALUES ({})", cols.join(", "), ph.join(", ")),
            rusqlite::params_from_iter(vals),
        )?;
        Ok(self.db.last_insert_rowid())
    }

    /// Insere uma linha em branco no final. Retorna o rowid criado.
    pub fn add_row(&self) -> Result<i64> {
        let max: f64 = self.db.query_row(
            &format!("SELECT COALESCE(MAX({}), 0.0) FROM dados", self.q_ord()),
            [],
            |r| r.get(0),
        )?;
        self.insert_blank(max + 1.0)
    }

    /// Insere uma linha em branco acima (ou abaixo) da linha de `ref_rowid`.
    /// Retorna o rowid criado.
    pub fn insert_row_relative(&self, ref_rowid: i64, above: bool) -> Result<i64> {
        let ref_ord = self.ord_of(ref_rowid)?;
        let new_ord = self.ord_between(ref_ord, above)?;
        self.insert_blank(new_ord)
    }

    fn ord_of(&self, rowid: i64) -> Result<f64> {
        let sql = format!("SELECT {} FROM dados WHERE {} = ?1", self.q_ord(), self.q_rid());
        Ok(self.db.query_row(&sql, [rowid], |r| r.get(0))?)
    }

    /// Exclui a linha de `rowid`.
    pub fn delete_row(&self, rowid: i64) -> Result<()> {
        self.db.execute(
            &format!("DELETE FROM dados WHERE {} = ?1", self.q_rid()),
            [rowid],
        )?;
        Ok(())
    }

    /// Calcula um `__ord` para inserir imediatamente acima (ou abaixo) da linha
    /// `ref_ord`, respeitando a vizinha existente.
    fn ord_between(&self, ref_ord: f64, above: bool) -> Result<f64> {
        let ord = self.q_ord();
        let neighbor_sql = if above {
            format!("SELECT MAX({ord}) FROM dados WHERE {ord} < ?1")
        } else {
            format!("SELECT MIN({ord}) FROM dados WHERE {ord} > ?1")
        };
        let neighbor: Option<f64> = self.db.query_row(&neighbor_sql, [ref_ord], |r| r.get(0))?;
        Ok(match neighbor {
            Some(n) => (n + ref_ord) / 2.0,
            None if above => ref_ord - 1.0,
            None => ref_ord + 1.0,
        })
    }

    /// Clona a linha `ref_rowid`, inserindo a cópia imediatamente acima (ou
    /// abaixo). Retorna o rowid da nova linha e os valores copiados, na ordem
    /// de `field_names()`.
    pub fn clone_row(&self, ref_rowid: i64, above: bool) -> Result<(i64, Vec<String>)> {
        let ref_ord = self.ord_of(ref_rowid)?;
        let new_ord = self.ord_between(ref_ord, above)?;

        let names = self.field_names();
        if names.is_empty() {
            bail!("documento sem colunas");
        }
        // lê os valores da linha de origem
        let sel = names.iter().map(|n| quote_ident(n)).collect::<Vec<_>>().join(", ");
        let vals: Vec<String> = {
            let mut stmt = self.db.prepare(&format!(
                "SELECT {sel} FROM dados WHERE {} = ?1",
                self.q_rid()
            ))?;
            stmt.query_row([ref_rowid], |r| {
                let mut v = Vec::with_capacity(names.len());
                for i in 0..names.len() {
                    let val: Value = r.get(i)?;
                    v.push(value_to_string(&val));
                }
                Ok(v)
            })?
        };

        // insere a cópia com os mesmos valores
        let mut cols: Vec<String> = names.iter().map(|n| quote_ident(n)).collect();
        cols.push(self.q_ord());
        let mut params: Vec<Value> = vals.iter().map(|v| Value::Text(v.clone())).collect();
        params.push(Value::Real(new_ord));
        let ph: Vec<String> = (1..=cols.len()).map(|i| format!("?{i}")).collect();
        self.db.execute(
            &format!(
                "INSERT INTO dados ({}) VALUES ({})",
                cols.join(", "),
                ph.join(", ")
            ),
            rusqlite::params_from_iter(params),
        )?;
        Ok((self.db.last_insert_rowid(), vals))
    }

    /// Move a linha `rowid` uma posição para cima (ou para baixo), trocando a
    /// ordem com a vizinha. Retorna o rowid da linha vizinha que trocou de
    /// lugar, ou `None` se `rowid` já estava no topo/fundo.
    pub fn move_row(&self, rowid: i64, up: bool) -> Result<Option<i64>> {
        let ord_v = self.ord_of(rowid)?;
        let (rid, ord) = (self.q_rid(), self.q_ord());
        let neighbor_sql = if up {
            format!("SELECT {rid}, {ord} FROM dados WHERE {ord} < ?1 ORDER BY {ord} DESC LIMIT 1")
        } else {
            format!("SELECT {rid}, {ord} FROM dados WHERE {ord} > ?1 ORDER BY {ord} ASC LIMIT 1")
        };
        let neighbor: Option<(i64, f64)> = self
            .db
            .query_row(&neighbor_sql, [ord_v], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?;
        let Some((nb_rowid, nb_ord)) = neighbor else {
            return Ok(None);
        };
        // troca os __ord das duas linhas
        let upd = format!("UPDATE dados SET {ord} = ?1 WHERE {rid} = ?2");
        self.db.execute(&upd, rusqlite::params![nb_ord, rowid])?;
        self.db.execute(&upd, rusqlite::params![ord_v, nb_rowid])?;
        Ok(Some(nb_rowid))
    }

    pub fn row_count(&self) -> i64 {
        self.db
            .query_row("SELECT COUNT(*) FROM dados", [], |r| r.get(0))
            .unwrap_or(0)
    }

    /// Visão editável: inclui o rowid interno sob o apelido `rid_alias()`.
    /// As colunas são listadas explicitamente (nada de `SELECT *`) para que
    /// nenhuma coluna de controle vaze para a tabela exibida.
    pub fn filter(&self, where_clause: Option<&str>) -> Result<QueryResult> {
        let mut sel = vec![format!("{} AS {}", self.q_rid(), quote_ident(&self.rid_alias))];
        sel.extend(self.field_names().iter().map(|n| quote_ident(n)));
        let mut sql = format!("SELECT {} FROM dados", sel.join(", "));
        if let Some(w) = where_clause {
            if !w.trim().is_empty() {
                sql.push_str(" WHERE ");
                sql.push_str(w);
            }
        }
        sql.push_str(&format!(" ORDER BY {}", self.q_ord()));
        self.run_readonly(&sql)
    }

    /// SQL arbitrário digitado pelo usuário — rigorosamente somente leitura.
    pub fn query(&self, sql: &str) -> Result<QueryResult> {
        self.run_readonly(sql)
    }

    /// Executa `sql` com três travas independentes contra escrita:
    /// 1. só aceita comandos que começam por SELECT/WITH/VALUES/EXPLAIN;
    /// 2. `PRAGMA query_only` bloqueia qualquer alteração em arquivo de banco;
    /// 3. `sqlite3_stmt_readonly` recusa o que ainda assim escreveria.
    /// (ATTACH já é impossível: o limite de bancos anexados é zero — veja
    /// `open_db`.) Sem isso, a caixa rotulada "somente leitura" permitiria
    /// apagar os dados ou gravar arquivos arbitrários no disco.
    fn run_readonly(&self, sql: &str) -> Result<QueryResult> {
        ensure_select_only(sql)?;
        let _guard = ReadOnlyGuard::new(&self.db)?;
        self.run_checked(sql)
    }

    fn run_checked(&self, sql: &str) -> Result<QueryResult> {
        let stmt = self.db.prepare(sql)?;
        if !stmt.readonly() {
            bail!("apenas consultas de leitura são permitidas");
        }
        self.collect(stmt)
    }

    fn collect(&self, mut stmt: rusqlite::Statement) -> Result<QueryResult> {
        let ncol = stmt.column_count();
        let columns: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();
        let mut rows = Vec::new();
        let mut q = stmt.query([])?;
        while let Some(r) = q.next()? {
            let mut row = Vec::with_capacity(ncol);
            for i in 0..ncol {
                let v: Value = r.get(i)?;
                row.push(value_to_string(&v));
            }
            rows.push(row);
        }
        Ok(QueryResult { columns, rows })
    }

    pub fn update_cell(&self, rowid: i64, column: &str, value: &str) -> Result<()> {
        let sql = format!(
            "UPDATE dados SET {} = ?1 WHERE {} = ?2",
            quote_ident(column),
            self.q_rid()
        );
        self.db.execute(&sql, rusqlite::params![value, rowid])?;
        Ok(())
    }

    /// Soma valores numéricos de uma coluna. Retorna (total, qtd_numéricos).
    pub fn sum_column(&self, column: &str, where_clause: Option<&str>) -> Result<(f64, usize)> {
        let mut sql = format!("SELECT {} FROM dados", quote_ident(column));
        if let Some(w) = where_clause {
            if !w.trim().is_empty() {
                sql.push_str(" WHERE ");
                sql.push_str(w);
            }
        }
        let res = self.run_readonly(&sql)?;
        let mut total = 0.0;
        let mut count = 0;
        for row in &res.rows {
            if let Some(n) = row.first().and_then(|s| parse_number(s)) {
                total += n;
                count += 1;
            }
        }
        Ok((total, count))
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let names = self.field_names();
        // Última barreira: nomes de campo viram nomes de atributo e não podem
        // ser escapados. Um nome inválido aqui significaria gravar XML quebrado.
        for n in &names {
            if !is_valid_xml_name(n) {
                bail!("campo com nome inválido para XML: {n:?}");
            }
        }
        let mut out = String::with_capacity(1024 * 1024);
        out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>  ");
        out.push_str(&self.datapacket_open);
        out.push_str("<METADATA><FIELDS>");
        for f in self.fields.borrow().iter() {
            out.push_str("<FIELD");
            for (k, v) in &f.attrs {
                if !is_valid_xml_name(k) {
                    continue;
                }
                out.push(' ');
                out.push_str(k);
                out.push_str("=\"");
                xml_escape_attr(v, &mut out);
                out.push('"');
            }
            out.push_str("/>");
        }
        out.push_str("</FIELDS>");
        out.push_str(&self.params_xml);
        out.push_str("</METADATA><ROWDATA>");

        let cols = names.iter().map(|n| quote_ident(n)).collect::<Vec<_>>().join(", ");
        let sql = format!("SELECT {cols} FROM dados ORDER BY {}", self.q_ord());
        let mut stmt = self.db.prepare(&sql)?;
        let mut q = stmt.query([])?;
        while let Some(r) = q.next()? {
            out.push_str("<ROW");
            for (i, name) in names.iter().enumerate() {
                let v: Value = r.get(i)?;
                let s = value_to_string(&v);
                if s.is_empty() {
                    continue; // XmlDoc omite atributos nulos/vazios
                }
                out.push(' ');
                out.push_str(name);
                out.push_str("=\"");
                xml_escape_attr(&s, &mut out);
                out.push('"');
            }
            out.push_str("/>");
        }
        out.push_str("</ROWDATA></DATAPACKET>");

        write_atomic(path, out.as_bytes())
    }

    pub fn export_csv(&self, path: &Path, result: &QueryResult) -> Result<()> {
        let mut out = String::new();
        // cabeçalho (pula as colunas de controle)
        let visible: Vec<usize> = result
            .columns
            .iter()
            .enumerate()
            .filter(|(_, c)| !self.is_internal_col(c))
            .map(|(i, _)| i)
            .collect();
        let header: Vec<String> = visible
            .iter()
            .map(|&i| csv_field(&result.columns[i]))
            .collect();
        out.push_str(&header.join(";"));
        out.push('\n');
        for row in &result.rows {
            let line: Vec<String> = visible.iter().map(|&i| csv_field(&row[i])).collect();
            out.push_str(&line.join(";"));
            out.push('\n');
        }
        write_atomic(path, out.as_bytes())
    }
}

/// Liga `PRAGMA query_only` e o desliga ao sair de escopo — inclusive se a
/// consulta falhar ou entrar em pânico. Sem isso, uma saída inesperada
/// deixaria a conexão travada e as edições da tabela parariam de gravar.
struct ReadOnlyGuard<'a>(&'a Connection);

impl<'a> ReadOnlyGuard<'a> {
    fn new(db: &'a Connection) -> Result<Self> {
        db.execute_batch("PRAGMA query_only=ON;")?;
        Ok(Self(db))
    }
}

impl Drop for ReadOnlyGuard<'_> {
    fn drop(&mut self) {
        let _ = self.0.execute_batch("PRAGMA query_only=OFF;");
    }
}

/// Abre o banco em memória usado como motor da tabela.
fn open_db() -> Result<Connection> {
    let db = Connection::open_in_memory()?;
    // Acelera carga em massa.
    db.execute_batch("PRAGMA journal_mode=OFF; PRAGMA synchronous=OFF;")?;
    // O app nunca precisa de ATTACH. Zerar o limite de bancos anexados impede
    // que uma consulta do usuário crie ou sobrescreva arquivos fora do
    // documento (`ATTACH '/caminho/x.db'` + `CREATE TABLE x.y AS SELECT ...`).
    db.set_limit(rusqlite::limits::Limit::SQLITE_LIMIT_ATTACHED, 0);
    Ok(db)
}

/// Recusa qualquer comando que não comece por uma palavra-chave de leitura.
fn ensure_select_only(sql: &str) -> Result<()> {
    // despreza comentários e espaços à esquerda
    let mut s = sql.trim_start();
    loop {
        if let Some(rest) = s.strip_prefix("--") {
            s = rest.find('\n').map(|i| &rest[i + 1..]).unwrap_or("").trim_start();
        } else if let Some(rest) = s.strip_prefix("/*") {
            let Some(i) = rest.find("*/") else {
                bail!("comentário SQL não terminado");
            };
            s = rest[i + 2..].trim_start();
        } else {
            break;
        }
    }
    let word: String = s
        .chars()
        .take_while(|c| c.is_ascii_alphabetic())
        .collect::<String>()
        .to_ascii_uppercase();
    if matches!(word.as_str(), "SELECT" | "WITH" | "VALUES" | "EXPLAIN") {
        Ok(())
    } else {
        bail!("apenas SELECT/WITH/VALUES são permitidos (consulta somente leitura)")
    }
}

/// Lê o início do arquivo como texto, limitado a `HEADER_SCAN_BYTES`.
/// Bytes inválidos em UTF-8 cortam a leitura em vez de virarem lixo: a moldura
/// extraída daqui é copiada literalmente para o arquivo salvo.
fn read_header(path: &Path) -> Result<String> {
    let f = std::fs::File::open(path).with_context(|| format!("abrindo {}", path.display()))?;
    let mut buf = Vec::new();
    f.take(HEADER_SCAN_BYTES).read_to_end(&mut buf)?;
    Ok(match std::str::from_utf8(&buf) {
        Ok(s) => s.to_string(),
        Err(e) => String::from_utf8_lossy(&buf[..e.valid_up_to()]).into_owned(),
    })
}

/// Grava `bytes` em `path` de forma atômica: escreve num temporário no mesmo
/// diretório e só então renomeia por cima. Uma falha no meio da escrita (disco
/// cheio, queda de energia, erro de E/S) deixa o arquivo original intacto, em
/// vez de truncado — `File::create` apaga o conteúdo antes da primeira escrita.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);

    let dir = path.parent().filter(|p| !p.as_os_str().is_empty());
    let dir = dir.unwrap_or_else(|| Path::new("."));
    let base = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "saida".into());
    let tmp = dir.join(format!(
        ".{base}.tmp-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));

    let write = || -> Result<()> {
        let mut f = std::fs::File::create(&tmp)
            .with_context(|| format!("gravando {}", tmp.display()))?;
        f.write_all(bytes)?;
        f.sync_all()?;
        Ok(())
    };
    if let Err(e) = write() {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }

    // Preserva as permissões do arquivo que será substituído.
    #[cfg(unix)]
    if let Ok(meta) = std::fs::metadata(path) {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(
            &tmp,
            std::fs::Permissions::from_mode(meta.permissions().mode()),
        );
    }

    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(anyhow::Error::from(e))
            .with_context(|| format!("gravando {}", path.display()));
    }
    Ok(())
}

/// Coloca um nome de coluna entre aspas duplas, duplicando aspas internas, para
/// uso seguro em SQL (nomes podem conter espaços, aspas ou caracteres especiais
/// vindos dos atributos do XML ou de um rename do usuário).
fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// Deriva de `base` um nome que não colida com nenhum de `taken`.
/// O SQLite não distingue maiúsculas em identificadores, então a comparação
/// também não distingue.
fn unique_name(base: &str, taken: &[String]) -> String {
    let mut name = base.to_string();
    let mut n = 1;
    while taken.iter().any(|t| t.eq_ignore_ascii_case(&name)) {
        name = format!("{base}_{n}");
        n += 1;
    }
    name
}

fn is_name_start(c: char) -> bool {
    c == ':'
        || c == '_'
        || c.is_ascii_alphabetic()
        || matches!(c as u32,
            0xC0..=0xD6 | 0xD8..=0xF6 | 0xF8..=0x2FF | 0x370..=0x37D | 0x37F..=0x1FFF
            | 0x200C..=0x200D | 0x2070..=0x218F | 0x2C00..=0x2FEF | 0x3001..=0xD7FF
            | 0xF900..=0xFDCF | 0xFDF0..=0xFFFD | 0x10000..=0xEFFFF)
}

fn is_name_char(c: char) -> bool {
    is_name_start(c)
        || c == '-'
        || c == '.'
        || c.is_ascii_digit()
        || c as u32 == 0xB7
        || matches!(c as u32, 0x300..=0x36F | 0x203F..=0x2040)
}

/// `true` se `s` é um Name XML válido. Nomes de campo viram nomes de atributo
/// na gravação, onde não existe escape possível: validar na entrada é o que
/// impede um `attrname` como `a" evil="1` de injetar atributos no arquivo.
fn is_valid_xml_name(s: &str) -> bool {
    let mut it = s.chars();
    match it.next() {
        Some(c) if is_name_start(c) => {}
        _ => return false,
    }
    it.all(is_name_char)
}

/// Valida um nome de coluna digitado pelo usuário.
fn check_column_name(name: &str) -> Result<()> {
    if name.is_empty() {
        bail!("nome de coluna vazio");
    }
    if !is_valid_xml_name(name) {
        bail!(
            "nome inválido para XML: use apenas letras, dígitos, '_', '-', '.' e ':', \
             começando por letra ou '_'"
        );
    }
    Ok(())
}

const DEFAULT_DATAPACKET_OPEN: &str = "<DATAPACKET Version=\"2.0\">";
const DEFAULT_PARAMS: &str = "<PARAMS/>";

/// Procura `needle` (que começa com `<`) em `raw`, pulando comentários, CDATA e
/// instruções de processamento — assim um `<PARAMS>` escondido num comentário
/// não é confundido com a seção real e copiado para o arquivo salvo.
fn find_markup(raw: &str, needle: &str, from: usize) -> Option<usize> {
    let b = raw.as_bytes();
    let mut i = from;
    while i < b.len() {
        if b[i] == b'<' {
            // `b[i] == b'<'` garante fronteira de caractere para fatiar.
            let rest = &raw[i..];
            if let Some(r) = rest.strip_prefix("<!--") {
                i += 4 + r.find("-->")? + 3;
                continue;
            }
            if let Some(r) = rest.strip_prefix("<![CDATA[") {
                i += 9 + r.find("]]>")? + 3;
                continue;
            }
            if let Some(r) = rest.strip_prefix("<?") {
                i += 2 + r.find("?>")? + 2;
                continue;
            }
            if rest.starts_with(needle) {
                return Some(i);
            }
        }
        i += 1;
    }
    None
}

/// Índice do `>` que fecha a tag aberta em `start`, ignorando `>` dentro de
/// valores de atributo entre aspas.
fn find_tag_end(raw: &str, start: usize) -> Option<usize> {
    let b = raw.as_bytes();
    let mut quote: Option<u8> = None;
    for (i, &c) in b.iter().enumerate().skip(start) {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None if c == b'"' || c == b'\'' => quote = Some(c),
            None if c == b'>' => return Some(i),
            None => {}
        }
    }
    None
}

/// `true` se o caractere após o nome da tag a encerra (evita casar `<PARAMSX`).
fn tag_name_ends(raw: &str, after: usize) -> bool {
    matches!(raw[after..].chars().next(), Some(c) if c.is_whitespace() || c == '>' || c == '/')
}

/// Extrai a tag de abertura `<TAG ...>` (inclusive o `>`) do texto cru.
/// Retorna `None` se a tag não existir.
fn extract_open_tag(raw: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}");
    let mut from = 0;
    loop {
        let start = find_markup(raw, &open, from)?;
        if tag_name_ends(raw, start + open.len()) {
            let end = find_tag_end(raw, start)?;
            return Some(raw[start..=end].to_string());
        }
        from = start + 1;
    }
}

/// Extrai a seção `<PARAMS.../>` ou `<PARAMS>...</PARAMS>` literal do texto cru,
/// procurando apenas dentro de `<METADATA>` e fora de comentários/CDATA.
fn extract_params(raw: &str) -> Option<String> {
    let meta = find_markup(raw, "<METADATA", 0).unwrap_or(0);
    let mut from = meta;
    let start = loop {
        let s = find_markup(raw, "<PARAMS", from)?;
        if tag_name_ends(raw, s + "<PARAMS".len()) {
            break s;
        }
        from = s + 1;
    };
    let open_end = find_tag_end(raw, start)?;
    // Auto-fechada: `<PARAMS .../>`
    if raw.as_bytes().get(open_end.wrapping_sub(1)) == Some(&b'/') {
        return Some(raw[start..=open_end].to_string());
    }
    // Com conteúdo: fecha em `</PARAMS>`
    let close = find_markup(raw, "</PARAMS", open_end)?;
    let close_end = find_tag_end(raw, close)? + 1;
    Some(raw[start..close_end].to_string())
}

fn create_table(conn: &Connection, rid: &str, names: &[String], ord: &str) -> Result<()> {
    // Chave primária explícita: é o rowid da tabela, mas com um nome que o app
    // controla. Assim um campo do arquivo chamado `rowid` não o encobre.
    let mut cols = vec![format!("{} INTEGER PRIMARY KEY", quote_ident(rid))];
    cols.extend(names.iter().map(|n| format!("{} TEXT", quote_ident(n))));
    // coluna interna de ordenação (não exibida, não salva nos FIELDS)
    cols.push(format!("{} REAL", quote_ident(ord)));
    conn.execute(&format!("CREATE TABLE dados ({})", cols.join(", ")), [])?;
    Ok(())
}

fn build_insert(names: &[String], ord: &str) -> String {
    let mut cols: Vec<String> = names.iter().map(|n| quote_ident(n)).collect();
    cols.push(quote_ident(ord));
    let ph = (1..=cols.len())
        .map(|i| format!("?{i}"))
        .collect::<Vec<_>>()
        .join(", ");
    format!("INSERT INTO dados ({}) VALUES ({ph})", cols.join(", "))
}

fn value_to_string(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::Integer(i) => i.to_string(),
        Value::Real(r) => r.to_string(),
        Value::Text(t) => t.clone(),
        Value::Blob(_) => String::new(),
    }
}

/// Aceita "1234.56" (ponto decimal) e BR "1.234,56".
fn parse_number(s: &str) -> Option<f64> {
    let t = s.trim();
    if t.is_empty() {
        return None;
    }
    if let Ok(n) = t.parse::<f64>() {
        return Some(n);
    }
    // formato BR: remove pontos de milhar, troca vírgula por ponto
    let br: String = t.chars().filter(|c| *c != '.').collect::<String>().replace(',', ".");
    br.parse::<f64>().ok()
}

fn xml_escape_attr(s: &str, out: &mut String) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            // Caracteres de controle devem virar referências numéricas: dentro
            // de um atributo, \n/\r/\t literais são normalizados para espaço na
            // releitura (perda de dados — ex.: históricos com várias linhas).
            '\n' => out.push_str("&#10;"),
            '\r' => out.push_str("&#13;"),
            '\t' => out.push_str("&#9;"),
            _ => out.push(c),
        }
    }
}

fn csv_field(s: &str) -> String {
    if s.contains(';') || s.contains('"') || s.contains('\n') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Caminho temporário exclusivo desta execução e desta chamada. Os testes
    /// rodam em paralelo na mesma thread pool; um nome fixo fazia um teste ler
    /// o arquivo enquanto outro o reescrevia.
    fn tmp_path(nome: impl std::fmt::Display) -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        std::env::temp_dir().join(format!(
            "lexml_{}_{}_{nome}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ))
    }

    /// Gera um XML mínimo no formato esperado (METADATA/FIELDS + ROWDATA/ROW).
    fn sample_xml() -> PathBuf {
        let xml = concat!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>",
            "<DATAPACKET Version=\"2.0\"><METADATA><FIELDS>",
            "<FIELD attrname=\"id\" fieldtype=\"string\" WIDTH=\"10\"/>",
            "<FIELD attrname=\"nome\" fieldtype=\"string\" WIDTH=\"40\"/>",
            "<FIELD attrname=\"valor\" fieldtype=\"string\" WIDTH=\"20\"/>",
            "</FIELDS><PARAMS/></METADATA><ROWDATA>",
            "<ROW id=\"1\" nome=\"Alpha\" valor=\"10.50\"/>",
            "<ROW id=\"2\" nome=\"Beta\" valor=\"4.25\"/>",
            "<ROW id=\"3\" nome=\"Gama\"/>",
            "</ROWDATA></DATAPACKET>",
        );
        let p = tmp_path("lexml_sample.xml");
        std::fs::write(&p, xml).unwrap();
        p
    }

    #[test]
    fn parse_filter_sum_roundtrip() {
        let dp = XmlDoc::open(&sample_xml()).unwrap();
        assert_eq!(dp.row_count(), 3);
        assert_eq!(dp.field_names(), vec!["id", "nome", "valor"]);

        // filtro WHERE traz a coluna _rid (visão editável)
        let f = dp.filter(Some("valor <> ''")).unwrap();
        assert_eq!(f.rows.len(), 2);
        assert_eq!(f.columns[0], "_rid");

        // soma da coluna numérica
        let (total, count) = dp.sum_column("valor", None).unwrap();
        assert_eq!(count, 2);
        assert!((total - 14.75).abs() < 0.001, "total={total}");

        // round-trip: salvar e reabrir preserva contagem e campos
        let tmp = tmp_path("lexml_roundtrip.xml");
        dp.save(&tmp).unwrap();
        let dp2 = XmlDoc::open(&tmp).unwrap();
        assert_eq!(dp2.row_count(), 3);
        assert_eq!(dp2.field_names(), dp.field_names());
    }

    /// Quebras de linha dentro de um atributo (escritas como &#010;/&#013; no
    /// formato original) devem sobreviver ao round-trip. Sem escapar os
    /// caracteres de controle, a releitura os normalizaria para espaço.
    #[test]
    fn roundtrip_preserva_quebras_de_linha() {
        let xml = concat!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>",
            "<DATAPACKET Version=\"2.0\"><METADATA><FIELDS>",
            "<FIELD attrname=\"id\" fieldtype=\"string\" WIDTH=\"10\"/>",
            "<FIELD attrname=\"hist\" fieldtype=\"string\" WIDTH=\"40\"/>",
            "</FIELDS><PARAMS/></METADATA><ROWDATA>",
            "<ROW id=\"1\" hist=\"linha1&#010;&#013;linha2\"/>",
            "</ROWDATA></DATAPACKET>",
        );
        let p = tmp_path("lexml_ctrl.xml");
        std::fs::write(&p, xml).unwrap();

        let hist_of = |res: &QueryResult| -> String {
            let idx = res.columns.iter().position(|c| c == "hist").unwrap();
            res.rows[0][idx].clone()
        };

        let dp = XmlDoc::open(&p).unwrap();
        let antes = dp.filter(None).unwrap();
        let hist_antes = hist_of(&antes);
        assert!(hist_antes.contains('\n'), "parser deveria ter LF: {hist_antes:?}");

        let tmp = tmp_path("lexml_ctrl_out.xml");
        dp.save(&tmp).unwrap();
        let dp2 = XmlDoc::open(&tmp).unwrap();
        let depois = dp2.filter(None).unwrap();
        let hist_depois = hist_of(&depois);
        assert_eq!(hist_antes, hist_depois, "quebras de linha perdidas no round-trip");
    }

    /// A tag <DATAPACKET> (Version) e a seção <PARAMS> com conteúdo devem
    /// sobreviver ao salvar/reabrir.
    #[test]
    fn roundtrip_preserva_datapacket_e_params() {
        let xml = concat!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>",
            "<DATAPACKET Version=\"3.1\"><METADATA><FIELDS>",
            "<FIELD attrname=\"id\" fieldtype=\"string\" WIDTH=\"10\"/>",
            "</FIELDS>",
            "<PARAMS><PARAM Name=\"LCID\" fieldtype=\"i4\" VALUE=\"1046\"/></PARAMS>",
            "</METADATA><ROWDATA>",
            "<ROW id=\"1\"/>",
            "</ROWDATA></DATAPACKET>",
        );
        let p = tmp_path("lexml_frame.xml");
        std::fs::write(&p, xml).unwrap();

        let dp = XmlDoc::open(&p).unwrap();
        let tmp = tmp_path("lexml_frame_out.xml");
        dp.save(&tmp).unwrap();
        let out = std::fs::read_to_string(&tmp).unwrap();

        assert!(out.contains("<DATAPACKET Version=\"3.1\">"), "Version perdida: {out}");
        assert!(
            out.contains("<PARAMS><PARAM Name=\"LCID\" fieldtype=\"i4\" VALUE=\"1046\"/></PARAMS>"),
            "PARAMS perdido: {out}"
        );
    }

    /// Uma coluna com aspas no nome é recusada na abertura: ela viraria nome de
    /// atributo na gravação, onde não há escape possível. (Antes o arquivo
    /// abria e só quebrava na hora de salvar.)
    #[test]
    fn nome_de_coluna_com_aspas_e_recusado() {
        let xml = concat!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>",
            "<DATAPACKET Version=\"2.0\"><METADATA><FIELDS>",
            "<FIELD attrname=\"a&quot;b\" fieldtype=\"string\" WIDTH=\"10\"/>",
            "</FIELDS><PARAMS/></METADATA><ROWDATA>",
            "<ROW/>",
            "</ROWDATA></DATAPACKET>",
        );
        let p = tmp_path("lexml_quote.xml");
        std::fs::write(&p, xml).unwrap();
        assert!(XmlDoc::open(&p).is_err());
    }

    /// A camada SQL continua protegida por conta própria: `quote_ident` escapa
    /// aspas, então nenhum nome de coluna quebra uma consulta montada.
    #[test]
    fn quote_ident_escapa_aspas() {
        assert_eq!(quote_ident("a\"b"), "\"a\"\"b\"");
        assert_eq!(quote_ident("x\" FROM dados; --"), "\"x\"\" FROM dados; --\"");
    }

    /// Coleta a coluna `col` em ordem de exibição (__ord).
    fn column_in_order(dp: &XmlDoc, col: &str) -> Vec<String> {
        let res = dp.filter(None).unwrap();
        let idx = res.columns.iter().position(|c| c == col).unwrap();
        res.rows.iter().map(|r| r[idx].clone()).collect()
    }

    /// Descobre o _rid da linha em posição de exibição `pos`.
    fn rid_at(dp: &XmlDoc, pos: usize) -> i64 {
        let res = dp.filter(None).unwrap();
        let idx = res.columns.iter().position(|c| c == "_rid").unwrap();
        res.rows[pos][idx].parse().unwrap()
    }

    #[test]
    fn clonar_linha_copia_valores_e_posiciona() {
        let dp = XmlDoc::open(&sample_xml()).unwrap();
        // clona a 2ª linha (Beta) para baixo
        let beta_rid = rid_at(&dp, 1);
        let (_new, vals) = dp.clone_row(beta_rid, false).unwrap();
        assert_eq!(vals, vec!["2", "Beta", "4.25"]);
        assert_eq!(dp.row_count(), 4);
        // a cópia deve aparecer logo após Beta
        assert_eq!(
            column_in_order(&dp, "nome"),
            vec!["Alpha", "Beta", "Beta", "Gama"]
        );

        // clona a 1ª linha (Alpha) para cima
        let alpha_rid = rid_at(&dp, 0);
        dp.clone_row(alpha_rid, true).unwrap();
        assert_eq!(
            column_in_order(&dp, "nome"),
            vec!["Alpha", "Alpha", "Beta", "Beta", "Gama"]
        );
    }

    #[test]
    fn mover_linha_troca_ordem() {
        let dp = XmlDoc::open(&sample_xml()).unwrap();
        // move a 2ª linha (Beta) para cima → troca com Alpha
        let beta_rid = rid_at(&dp, 1);
        let nb = dp.move_row(beta_rid, true).unwrap();
        assert!(nb.is_some());
        assert_eq!(column_in_order(&dp, "nome"), vec!["Beta", "Alpha", "Gama"]);

        // topo não move mais
        assert_eq!(dp.move_row(beta_rid, true).unwrap(), None);

        // move Beta (agora no topo) para baixo → troca com Alpha de volta
        dp.move_row(beta_rid, false).unwrap();
        assert_eq!(column_in_order(&dp, "nome"), vec!["Alpha", "Beta", "Gama"]);

        // fundo não move mais
        let gama_rid = rid_at(&dp, 2);
        assert_eq!(dp.move_row(gama_rid, false).unwrap(), None);
    }

    // ===== Regressões de segurança/integridade =====

    /// A caixa de SQL é anunciada como somente leitura. Antes destas travas,
    /// `DELETE FROM dados` apagava o documento e `ATTACH` criava/sobrescrevia
    /// arquivos arbitrários no disco.
    #[test]
    fn sql_somente_leitura_recusa_escrita() {
        let dp = XmlDoc::open(&sample_xml()).unwrap();
        assert_eq!(dp.row_count(), 3);

        let alvo = tmp_path("lexml_reg_attach.db");
        let _ = std::fs::remove_file(&alvo);

        for sql in [
            "DELETE FROM dados",
            "UPDATE dados SET nome = 'x'",
            "DROP TABLE dados",
            "CREATE TABLE t (a)",
            "INSERT INTO dados (nome) VALUES ('x')",
            &format!("ATTACH DATABASE '{}' AS ev", alvo.display()),
            &format!("VACUUM INTO '{}'", alvo.display()),
            "PRAGMA journal_mode=DELETE",
            "  /* disfarce */ -- nada\n  DELETE FROM dados",
        ] {
            assert!(dp.query(sql).is_err(), "deveria ter sido recusado: {sql}");
        }

        assert_eq!(dp.row_count(), 3, "os dados foram alterados");
        assert!(!alvo.exists(), "um arquivo foi criado no disco");

        // leitura legítima continua funcionando
        let r = dp.query("SELECT nome FROM dados ORDER BY nome").unwrap();
        assert_eq!(r.rows.len(), 3);
        assert!(dp.query("WITH x AS (SELECT 1 AS a) SELECT a FROM x").is_ok());

        // e as edições normais da tabela não ficaram bloqueadas pelo query_only
        let rid = dp.add_row().unwrap();
        dp.update_cell(rid, "nome", "Delta").unwrap();
        assert_eq!(dp.row_count(), 4);
    }

    /// Um `attrname` com aspas viraria nome de atributo cru na gravação,
    /// produzindo `<ROW a" evil="1="X"/>` — XML quebrado/injetado.
    #[test]
    fn campo_com_nome_invalido_e_recusado() {
        for apelido in ["a&quot; evil=&quot;1", "a/><ROW b=", "com espaço", "1digito", ""] {
            let xml = format!(
                "<DATAPACKET Version=\"2.0\"><METADATA><FIELDS>\
                 <FIELD attrname=\"{apelido}\" fieldtype=\"string\" WIDTH=\"10\"/>\
                 </FIELDS><PARAMS/></METADATA><ROWDATA><ROW/></ROWDATA></DATAPACKET>"
            );
            let p = tmp_path("lexml_reg_inj.xml");
            std::fs::write(&p, &xml).unwrap();
            assert!(
                XmlDoc::open(&p).is_err(),
                "campo {apelido:?} deveria ser recusado"
            );
        }
    }

    /// O mesmo pela interface: renomear/criar coluna com nome inválido.
    #[test]
    fn rename_e_add_column_validam_o_nome() {
        let dp = XmlDoc::open(&sample_xml()).unwrap();
        for ruim in ["a\" evil=\"1", "com espaço", "<tag>", "", "2024"] {
            assert!(dp.rename_column("nome", ruim).is_err(), "rename {ruim:?}");
            assert!(dp.add_column(ruim).is_err(), "add {ruim:?}");
        }
        dp.rename_column("nome", "nome.completo").unwrap();
        dp.add_column("novo-campo").unwrap();

        // o arquivo gravado continua sendo XML relegível
        let out = tmp_path("lexml_reg_nomes.xml");
        dp.save(&out).unwrap();
        let dp2 = XmlDoc::open(&out).unwrap();
        assert_eq!(dp2.field_names(), vec!["id", "nome.completo", "valor", "novo-campo"]);
    }

    /// `<PARAMS>` dentro de um comentário não é a seção real; antes, era ela
    /// que acabava copiada para o arquivo salvo.
    #[test]
    fn params_dentro_de_comentario_e_ignorado() {
        let xml = concat!(
            "<?xml version=\"1.0\"?>",
            "<!-- <DATAPACKET Version=\"9.9\"><PARAMS>injetado</PARAMS> -->",
            "<DATAPACKET Version=\"2.0\"><METADATA><FIELDS>",
            "<FIELD attrname=\"id\" fieldtype=\"string\" WIDTH=\"10\"/>",
            "</FIELDS><PARAMS real=\"1\"/></METADATA>",
            "<ROWDATA><ROW id=\"1\"/></ROWDATA></DATAPACKET>",
        );
        let p = tmp_path("lexml_reg_params.xml");
        std::fs::write(&p, xml).unwrap();
        let dp = XmlDoc::open(&p).unwrap();
        let out = tmp_path("lexml_reg_params_out.xml");
        dp.save(&out).unwrap();
        let salvo = std::fs::read_to_string(&out).unwrap();
        assert!(salvo.contains("<PARAMS real=\"1\"/>"), "{salvo}");
        assert!(!salvo.contains("injetado"), "{salvo}");
        assert!(salvo.contains("Version=\"2.0\""), "{salvo}");
        assert!(!salvo.contains("9.9"), "{salvo}");
    }

    /// Um `>` dentro de valor de atributo não encerra a tag de abertura.
    #[test]
    fn datapacket_com_maior_que_no_atributo() {
        let xml = concat!(
            "<DATAPACKET Version=\"2.0\" nota=\"a &gt; b\"><METADATA><FIELDS>",
            "<FIELD attrname=\"id\" fieldtype=\"string\" WIDTH=\"10\"/>",
            "</FIELDS><PARAMS/></METADATA><ROWDATA><ROW id=\"1\"/></ROWDATA></DATAPACKET>",
        );
        let p = tmp_path("lexml_reg_gt.xml");
        std::fs::write(&p, xml).unwrap();
        let dp = XmlDoc::open(&p).unwrap();
        let out = tmp_path("lexml_reg_gt_out.xml");
        dp.save(&out).unwrap();
        assert_eq!(XmlDoc::open(&out).unwrap().row_count(), 1);
    }

    /// Campos com os nomes das colunas de controle já não impedem a abertura
    /// nem encobrem o rowid interno.
    #[test]
    fn campos_com_nomes_internos_funcionam() {
        for nome in ["__ord", "__rid", "_rid", "rowid", "oid"] {
            let xml = format!(
                "<DATAPACKET Version=\"2.0\"><METADATA><FIELDS>\
                 <FIELD attrname=\"{nome}\" fieldtype=\"string\" WIDTH=\"10\"/>\
                 <FIELD attrname=\"nome\" fieldtype=\"string\" WIDTH=\"10\"/>\
                 </FIELDS><PARAMS/></METADATA><ROWDATA>\
                 <ROW {nome}=\"v1\" nome=\"a\"/><ROW {nome}=\"v2\" nome=\"b\"/>\
                 </ROWDATA></DATAPACKET>"
            );
            let p = tmp_path(format!("lexml_reg_int_{nome}.xml"));
            std::fs::write(&p, &xml).unwrap();
            let dp = XmlDoc::open(&p).unwrap_or_else(|e| panic!("{nome}: {e}"));
            assert_eq!(dp.row_count(), 2, "{nome}");

            // a coluna do arquivo aparece; nenhuma coluna de controle vaza
            let res = dp.filter(None).unwrap();
            assert_eq!(res.columns[0], dp.rid_alias());
            assert_eq!(&res.columns[1..], &[nome.to_string(), "nome".to_string()][..]);

            // o rowid devolvido ainda serve para editar a linha certa
            let rid: i64 = res.rows[0][0].parse().unwrap();
            dp.update_cell(rid, "nome", "editado").unwrap();
            let depois = dp.filter(None).unwrap();
            assert_eq!(depois.rows[0][2], "editado", "{nome}");
            assert_eq!(depois.rows[1][2], "b", "{nome}");

            // e o round-trip preserva o campo
            let out = tmp_path(format!("lexml_reg_int_{nome}_out.xml"));
            dp.save(&out).unwrap();
            assert_eq!(XmlDoc::open(&out).unwrap().field_names(), vec![nome, "nome"]);
        }
    }

    /// A gravação não deve truncar o original antes de ter o conteúdo novo
    /// pronto, nem deixar temporários para trás.
    #[test]
    fn gravacao_atomica_nao_deixa_rastro() {
        let dir = tmp_path("atomica.d");
        std::fs::create_dir_all(&dir).unwrap();
        let alvo = dir.join("saida.xml");

        let dp = XmlDoc::open(&sample_xml()).unwrap();
        dp.save(&alvo).unwrap();
        dp.save(&alvo).unwrap(); // sobrescrita

        let restos: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().into_owned()))
            .filter(|n| n != "saida.xml")
            .collect();
        assert!(restos.is_empty(), "temporários deixados para trás: {restos:?}");
        assert_eq!(XmlDoc::open(&alvo).unwrap().row_count(), 3);

        // um destino impossível falha sem apagar o que já existia
        let antes = std::fs::read_to_string(&alvo).unwrap();
        assert!(dp.save(&dir.join("sem/tal/pasta/x.xml")).is_err());
        assert_eq!(std::fs::read_to_string(&alvo).unwrap(), antes);
    }

    /// Aspas simples em valor também são escapadas na gravação.
    #[test]
    fn valores_com_aspas_sobrevivem() {
        let dp = XmlDoc::open(&sample_xml()).unwrap();
        let rid: i64 = dp.filter(None).unwrap().rows[0][0].parse().unwrap();
        let bruto = r#"a" b' c<d>e&f"#;
        dp.update_cell(rid, "nome", bruto).unwrap();
        let out = tmp_path("lexml_reg_aspas.xml");
        dp.save(&out).unwrap();
        let dp2 = XmlDoc::open(&out).unwrap();
        assert_eq!(dp2.filter(None).unwrap().rows[0][2], bruto);
    }


}
