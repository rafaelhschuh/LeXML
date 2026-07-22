use anyhow::{anyhow, bail, Context, Result};
use quick_xml::events::Event;
use quick_xml::reader::Reader;
use rusqlite::{Connection, OptionalExtension};
use std::cell::RefCell;
use std::io::Write;
use std::path::Path;

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
        // Lê o texto cru uma vez para preservar a moldura do documento
        // (tag <DATAPACKET> e seção <PARAMS>) sem depender do modelo de eventos.
        let raw = std::fs::read_to_string(path).unwrap_or_default();
        let datapacket_open =
            extract_open_tag(&raw, "DATAPACKET").unwrap_or_else(|| DEFAULT_DATAPACKET_OPEN.into());
        let params_xml = extract_params(&raw).unwrap_or_else(|| DEFAULT_PARAMS.into());

        let mut reader = Reader::from_file(path)
            .with_context(|| format!("abrindo {}", path.display()))?;
        reader.config_mut().trim_text(false);

        let mut fields: Vec<Field> = Vec::new();
        let mut field_names: Vec<String> = Vec::new();
        let mut in_fields = false;

        let db = Connection::open_in_memory()?;
        // Acelera carga em massa.
        db.execute_batch("PRAGMA journal_mode=OFF; PRAGMA synchronous=OFF;")?;

        let mut table_ready = false;
        let mut buf = Vec::new();

        // Coletor de linhas: acumula e insere em lote dentro de uma transação.
        let tx = db.unchecked_transaction()?;
        let mut insert_sql = String::new();
        let mut pending: Vec<Vec<Option<String>>> = Vec::new();

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
                            let val = a.unescape_value()?.to_string();
                            if key == "attrname" {
                                name = val.clone();
                            }
                            attrs.push((key, val));
                        }
                        if !name.is_empty() {
                            field_names.push(name.clone());
                            fields.push(Field { name, attrs });
                        }
                    } else if tag.as_ref() == b"ROW" {
                        if !table_ready {
                            create_table(&tx, &field_names)?;
                            insert_sql = build_insert(&field_names);
                            table_ready = true;
                        }
                        let mut vals: Vec<Option<String>> = vec![None; field_names.len()];
                        for a in e.attributes().flatten() {
                            let key = String::from_utf8_lossy(a.key.as_ref());
                            if let Some(idx) = field_names.iter().position(|n| n == key.as_ref()) {
                                vals[idx] = Some(a.unescape_value()?.to_string());
                            }
                        }
                        pending.push(vals);
                    }
                }
                _ => {}
            }
            buf.clear();
        }

        if table_ready {
            let mut stmt = tx.prepare(&insert_sql)?;
            for vals in &pending {
                let params: Vec<&dyn rusqlite::ToSql> =
                    vals.iter().map(|v| v as &dyn rusqlite::ToSql).collect();
                stmt.execute(params.as_slice())?;
            }
        } else if !field_names.is_empty() {
            create_table(&tx, &field_names)?;
        } else {
            return Err(anyhow!("XML não reconhecido: faltam as seções METADATA/FIELDS"));
        }
        drop(insert_sql);
        tx.commit()?;

        // inicializa a ordem interna seguindo a ordem original das linhas
        db.execute("UPDATE dados SET __ord = rowid WHERE __ord IS NULL", [])?;

        Ok(Self {
            fields: RefCell::new(fields),
            datapacket_open,
            params_xml,
            db,
        })
    }

    /// Cria um documento em branco com uma coluna inicial e nenhuma linha.
    pub fn new_empty() -> Result<Self> {
        let db = Connection::open_in_memory()?;
        db.execute_batch("PRAGMA journal_mode=OFF; PRAGMA synchronous=OFF;")?;
        let names = vec!["coluna1".to_string()];
        create_table(&db, &names)?;
        Ok(Self {
            fields: RefCell::new(vec![default_field("coluna1")]),
            datapacket_open: DEFAULT_DATAPACKET_OPEN.into(),
            params_xml: DEFAULT_PARAMS.into(),
            db,
        })
    }

    pub fn field_names(&self) -> Vec<String> {
        self.fields.borrow().iter().map(|f| f.name.clone()).collect()
    }

    /// Adiciona uma nova coluna (TEXT) ao final.
    pub fn add_column(&self, name: &str) -> Result<()> {
        let name = name.trim();
        if name.is_empty() {
            bail!("nome de coluna vazio");
        }
        if self.fields.borrow().iter().any(|f| f.name == name) {
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
        if new.is_empty() {
            bail!("nome de coluna vazio");
        }
        if self.fields.borrow().iter().any(|f| f.name == new) {
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
        cols.push("\"__ord\"".to_string());
        let mut vals: Vec<String> = names.iter().map(|_| "''".to_string()).collect();
        vals.push(ord.to_string());
        self.db.execute(
            &format!("INSERT INTO dados ({}) VALUES ({})", cols.join(", "), vals.join(", ")),
            [],
        )?;
        Ok(self.db.last_insert_rowid())
    }

    /// Insere uma linha em branco no final. Retorna o rowid criado.
    pub fn add_row(&self) -> Result<i64> {
        let max: f64 = self
            .db
            .query_row("SELECT COALESCE(MAX(__ord), 0.0) FROM dados", [], |r| r.get(0))?;
        self.insert_blank(max + 1.0)
    }

    /// Insere uma linha em branco acima (ou abaixo) da linha de `ref_rowid`.
    /// Retorna o rowid criado.
    pub fn insert_row_relative(&self, ref_rowid: i64, above: bool) -> Result<i64> {
        let ref_ord: f64 =
            self.db
                .query_row("SELECT __ord FROM dados WHERE rowid = ?1", [ref_rowid], |r| r.get(0))?;
        let new_ord = self.ord_between(ref_ord, above)?;
        self.insert_blank(new_ord)
    }

    /// Exclui a linha de `rowid`.
    pub fn delete_row(&self, rowid: i64) -> Result<()> {
        self.db
            .execute("DELETE FROM dados WHERE rowid = ?1", [rowid])?;
        Ok(())
    }

    /// Calcula um `__ord` para inserir imediatamente acima (ou abaixo) da linha
    /// `ref_ord`, respeitando a vizinha existente.
    fn ord_between(&self, ref_ord: f64, above: bool) -> Result<f64> {
        let neighbor_sql = if above {
            "SELECT MAX(__ord) FROM dados WHERE __ord < ?1"
        } else {
            "SELECT MIN(__ord) FROM dados WHERE __ord > ?1"
        };
        let neighbor: Option<f64> = self.db.query_row(neighbor_sql, [ref_ord], |r| r.get(0))?;
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
        let ref_ord: f64 = self
            .db
            .query_row("SELECT __ord FROM dados WHERE rowid = ?1", [ref_rowid], |r| r.get(0))?;
        let new_ord = self.ord_between(ref_ord, above)?;

        let names = self.field_names();
        if names.is_empty() {
            bail!("documento sem colunas");
        }
        // lê os valores da linha de origem
        let sel = names.iter().map(|n| quote_ident(n)).collect::<Vec<_>>().join(", ");
        let vals: Vec<String> = {
            let mut stmt = self
                .db
                .prepare(&format!("SELECT {sel} FROM dados WHERE rowid = ?1"))?;
            stmt.query_row([ref_rowid], |r| {
                let mut v = Vec::with_capacity(names.len());
                for i in 0..names.len() {
                    let val: rusqlite::types::Value = r.get(i)?;
                    v.push(value_to_string(&val));
                }
                Ok(v)
            })?
        };

        // insere a cópia com os mesmos valores
        let mut cols: Vec<String> = names.iter().map(|n| quote_ident(n)).collect();
        cols.push("\"__ord\"".to_string());
        let mut placeholders: Vec<String> = (1..=names.len()).map(|i| format!("?{i}")).collect();
        placeholders.push(new_ord.to_string());
        self.db.execute(
            &format!(
                "INSERT INTO dados ({}) VALUES ({})",
                cols.join(", "),
                placeholders.join(", ")
            ),
            rusqlite::params_from_iter(vals.iter()),
        )?;
        Ok((self.db.last_insert_rowid(), vals))
    }

    /// Move a linha `rowid` uma posição para cima (ou para baixo), trocando a
    /// ordem com a vizinha. Retorna o rowid da linha vizinha que trocou de
    /// lugar, ou `None` se `rowid` já estava no topo/fundo.
    pub fn move_row(&self, rowid: i64, up: bool) -> Result<Option<i64>> {
        let ord: f64 = self
            .db
            .query_row("SELECT __ord FROM dados WHERE rowid = ?1", [rowid], |r| r.get(0))?;
        let neighbor_sql = if up {
            "SELECT rowid, __ord FROM dados WHERE __ord < ?1 ORDER BY __ord DESC LIMIT 1"
        } else {
            "SELECT rowid, __ord FROM dados WHERE __ord > ?1 ORDER BY __ord ASC LIMIT 1"
        };
        let neighbor: Option<(i64, f64)> = self
            .db
            .query_row(neighbor_sql, [ord], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?;
        let Some((nb_rowid, nb_ord)) = neighbor else {
            return Ok(None);
        };
        // troca os __ord das duas linhas
        self.db.execute(
            "UPDATE dados SET __ord = ?1 WHERE rowid = ?2",
            rusqlite::params![nb_ord, rowid],
        )?;
        self.db.execute(
            "UPDATE dados SET __ord = ?1 WHERE rowid = ?2",
            rusqlite::params![ord, nb_rowid],
        )?;
        Ok(Some(nb_rowid))
    }

    pub fn row_count(&self) -> i64 {
        self.db
            .query_row("SELECT COUNT(*) FROM dados", [], |r| r.get(0))
            .unwrap_or(0)
    }

    /// Visão editável: inclui rowid como coluna "_rid".
    pub fn filter(&self, where_clause: Option<&str>) -> Result<QueryResult> {
        let mut sql = String::from("SELECT rowid AS _rid, * FROM dados");
        if let Some(w) = where_clause {
            if !w.trim().is_empty() {
                sql.push_str(" WHERE ");
                sql.push_str(w);
            }
        }
        sql.push_str(" ORDER BY __ord");
        self.run(&sql)
    }

    /// SQL arbitrário (somente leitura, sem _rid garantido).
    pub fn query(&self, sql: &str) -> Result<QueryResult> {
        self.run(sql)
    }

    fn run(&self, sql: &str) -> Result<QueryResult> {
        let mut stmt = self.db.prepare(sql)?;
        let ncol = stmt.column_count();
        let columns: Vec<String> = stmt
            .column_names()
            .iter()
            .map(|s| s.to_string())
            .collect();
        let mut rows = Vec::new();
        let mut q = stmt.query([])?;
        while let Some(r) = q.next()? {
            let mut row = Vec::with_capacity(ncol);
            for i in 0..ncol {
                let v: rusqlite::types::Value = r.get(i)?;
                row.push(value_to_string(&v));
            }
            rows.push(row);
        }
        Ok(QueryResult { columns, rows })
    }

    pub fn update_cell(&self, rowid: i64, column: &str, value: &str) -> Result<()> {
        let sql = format!("UPDATE dados SET {} = ?1 WHERE rowid = ?2", quote_ident(column));
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
        let mut stmt = self.db.prepare(&sql)?;
        let mut q = stmt.query([])?;
        let mut total = 0.0;
        let mut count = 0;
        while let Some(r) = q.next()? {
            let v: rusqlite::types::Value = r.get(0)?;
            let s = value_to_string(&v);
            if let Some(n) = parse_number(&s) {
                total += n;
                count += 1;
            }
        }
        Ok((total, count))
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let names = self.field_names();
        let mut out = String::with_capacity(1024 * 1024);
        out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>  ");
        out.push_str(&self.datapacket_open);
        out.push_str("<METADATA><FIELDS>");
        for f in self.fields.borrow().iter() {
            out.push_str("<FIELD");
            for (k, v) in &f.attrs {
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
        let sql = format!("SELECT {cols} FROM dados ORDER BY __ord");
        let mut stmt = self.db.prepare(&sql)?;
        let mut q = stmt.query([])?;
        while let Some(r) = q.next()? {
            out.push_str("<ROW");
            for (i, name) in names.iter().enumerate() {
                let v: rusqlite::types::Value = r.get(i)?;
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

        let mut file = std::fs::File::create(path)
            .with_context(|| format!("gravando {}", path.display()))?;
        file.write_all(out.as_bytes())?;
        Ok(())
    }

    pub fn export_csv(&self, path: &Path, result: &QueryResult) -> Result<()> {
        let mut out = String::new();
        // cabeçalho (pula _rid se presente)
        let visible: Vec<usize> = result
            .columns
            .iter()
            .enumerate()
            .filter(|(_, c)| *c != "_rid" && *c != "__ord")
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
        std::fs::write(path, out).with_context(|| format!("gravando {}", path.display()))?;
        Ok(())
    }
}

/// Coloca um nome de coluna entre aspas duplas, duplicando aspas internas, para
/// uso seguro em SQL (nomes podem conter espaços, aspas ou caracteres especiais
/// vindos dos atributos do XML ou de um rename do usuário).
fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

const DEFAULT_DATAPACKET_OPEN: &str = "<DATAPACKET Version=\"2.0\">";
const DEFAULT_PARAMS: &str = "<PARAMS/>";

/// Extrai a tag de abertura `<TAG ...>` (inclusive o `>`) do texto cru.
/// Retorna `None` se a tag não existir.
fn extract_open_tag(raw: &str, tag: &str) -> Option<String> {
    let start = raw.find(&format!("<{tag}"))?;
    // garante que é a tag exata (próximo char é espaço, '>' ou '/')
    let after = raw[start + 1 + tag.len()..].chars().next()?;
    if !(after.is_whitespace() || after == '>' || after == '/') {
        return None;
    }
    let end = raw[start..].find('>')? + start;
    Some(raw[start..=end].to_string())
}

/// Extrai a seção `<PARAMS.../>` ou `<PARAMS>...</PARAMS>` literal do texto cru.
fn extract_params(raw: &str) -> Option<String> {
    let start = raw.find("<PARAMS")?;
    let after = raw[start + "<PARAMS".len()..].chars().next()?;
    if !(after.is_whitespace() || after == '>' || after == '/') {
        return None;
    }
    let open_end = raw[start..].find('>')? + start;
    // Auto-fechada: `<PARAMS .../>`
    if raw.as_bytes().get(open_end.wrapping_sub(1)) == Some(&b'/') {
        return Some(raw[start..=open_end].to_string());
    }
    // Com conteúdo: fecha em `</PARAMS>`
    let close = raw[open_end..].find("</PARAMS>")? + open_end;
    let close_end = close + "</PARAMS>".len();
    Some(raw[start..close_end].to_string())
}

fn create_table(conn: &Connection, names: &[String]) -> Result<()> {
    let mut cols: Vec<String> = names.iter().map(|n| format!("{} TEXT", quote_ident(n))).collect();
    // coluna interna de ordenação (não exibida, não salva nos FIELDS)
    cols.push("\"__ord\" REAL".to_string());
    conn.execute(&format!("CREATE TABLE dados ({})", cols.join(", ")), [])?;
    Ok(())
}

fn build_insert(names: &[String]) -> String {
    let cols = names
        .iter()
        .map(|n| quote_ident(n))
        .collect::<Vec<_>>()
        .join(", ");
    let ph = (1..=names.len())
        .map(|i| format!("?{i}"))
        .collect::<Vec<_>>()
        .join(", ");
    format!("INSERT INTO dados ({cols}) VALUES ({ph})")
}

fn value_to_string(v: &rusqlite::types::Value) -> String {
    use rusqlite::types::Value::*;
    match v {
        Null => String::new(),
        Integer(i) => i.to_string(),
        Real(r) => r.to_string(),
        Text(t) => t.clone(),
        Blob(_) => String::new(),
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
        let p = std::env::temp_dir().join("lexml_sample.xml");
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
        let tmp = std::env::temp_dir().join("lexml_roundtrip.xml");
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
        let p = std::env::temp_dir().join("lexml_ctrl.xml");
        std::fs::write(&p, xml).unwrap();

        let hist_of = |res: &QueryResult| -> String {
            let idx = res.columns.iter().position(|c| c == "hist").unwrap();
            res.rows[0][idx].clone()
        };

        let dp = XmlDoc::open(&p).unwrap();
        let antes = dp.filter(None).unwrap();
        let hist_antes = hist_of(&antes);
        assert!(hist_antes.contains('\n'), "parser deveria ter LF: {hist_antes:?}");

        let tmp = std::env::temp_dir().join("lexml_ctrl_out.xml");
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
        let p = std::env::temp_dir().join("lexml_frame.xml");
        std::fs::write(&p, xml).unwrap();

        let dp = XmlDoc::open(&p).unwrap();
        let tmp = std::env::temp_dir().join("lexml_frame_out.xml");
        dp.save(&tmp).unwrap();
        let out = std::fs::read_to_string(&tmp).unwrap();

        assert!(out.contains("<DATAPACKET Version=\"3.1\">"), "Version perdida: {out}");
        assert!(
            out.contains("<PARAMS><PARAM Name=\"LCID\" fieldtype=\"i4\" VALUE=\"1046\"/></PARAMS>"),
            "PARAMS perdido: {out}"
        );
    }

    /// Uma coluna com aspas no nome não deve quebrar o SQL (deve abrir como tabela).
    #[test]
    fn nome_de_coluna_com_aspas() {
        let xml = concat!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>",
            "<DATAPACKET Version=\"2.0\"><METADATA><FIELDS>",
            "<FIELD attrname=\"a&quot;b\" fieldtype=\"string\" WIDTH=\"10\"/>",
            "</FIELDS><PARAMS/></METADATA><ROWDATA>",
            "<ROW/>",
            "</ROWDATA></DATAPACKET>",
        );
        let p = std::env::temp_dir().join("lexml_quote.xml");
        std::fs::write(&p, xml).unwrap();

        let dp = XmlDoc::open(&p).unwrap();
        assert_eq!(dp.field_names(), vec!["a\"b".to_string()]);
        // operações que montam SQL com o nome não devem falhar
        dp.add_row().unwrap();
        assert_eq!(dp.row_count(), 2);
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
}
