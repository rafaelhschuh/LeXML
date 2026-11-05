//! Linhas na área de transferência: cada linha vira um elemento `<ROW .../>`
//! idêntico ao do arquivo salvo, um por linha de texto. Assim o mesmo texto
//! serve para colar em outra janela do Lê-XML e num editor de texto.

use crate::xmldoc::{is_valid_xml_name, xml_escape_attr};
use quick_xml::events::Event;
use quick_xml::reader::Reader;

/// Monta o texto de cópia. `names` são as colunas e cada linha traz os
/// valores na mesma ordem. Valores vazios não viram atributo, como no
/// salvamento; colunas cujo nome não é um nome XML válido (possível num
/// resultado SQL, ex.: `COUNT(*)`) ficam de fora.
pub fn rows_to_xml(names: &[String], rows: &[Vec<String>]) -> String {
    let mut out = String::new();
    for row in rows {
        out.push_str("<ROW");
        for (name, value) in names.iter().zip(row) {
            if value.is_empty() || !is_valid_xml_name(name) {
                continue;
            }
            out.push(' ');
            out.push_str(name);
            out.push_str("=\"");
            xml_escape_attr(value, &mut out);
            out.push('"');
        }
        out.push_str("/>\n");
    }
    out
}

/// Atributos de cada `<ROW>` encontrado em `text`, na ordem. `None` se o texto
/// não traz nenhuma linha — aí não há o que colar.
pub fn parse_rows(text: &str) -> Option<Vec<Vec<(String, String)>>> {
    let mut reader = Reader::from_str(text);
    // O texto copiado é um fragmento (várias linhas soltas, ou um trecho de
    // arquivo): não exige uma raiz única nem tags fechadas corretamente.
    reader.config_mut().check_end_names = false;
    let mut rows = Vec::new();
    loop {
        match reader.read_event() {
            Ok(Event::Eof) => break,
            Ok(Event::Empty(e) | Event::Start(e)) if e.name().as_ref() == b"ROW" => {
                let mut attrs = Vec::new();
                for a in e.attributes().flatten() {
                    let key = String::from_utf8_lossy(a.key.as_ref()).into_owned();
                    let Ok(val) = a.unescape_value() else {
                        continue;
                    };
                    attrs.push((key, val.into_owned()));
                }
                rows.push(attrs);
            }
            Ok(_) => {}
            // Texto que não é XML (ou quebra no meio): fica com o que já leu.
            Err(_) => break,
        }
    }
    (!rows.is_empty()).then_some(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn copia_e_cola_preservam_caracteres_especiais_e_quebras() {
        let names = s(&["id", "hist"]);
        let rows = vec![s(&["1", "a \"b\" & <c>\nlinha 2"]), s(&["2", ""])];
        let text = rows_to_xml(&names, &rows);
        assert_eq!(
            text,
            "<ROW id=\"1\" hist=\"a &quot;b&quot; &amp; &lt;c&gt;&#10;linha 2\"/>\n<ROW id=\"2\"/>\n"
        );
        let back = parse_rows(&text).unwrap();
        assert_eq!(
            back[0],
            vec![
                ("id".into(), "1".into()),
                ("hist".into(), "a \"b\" & <c>\nlinha 2".into())
            ]
        );
        assert_eq!(back[1], vec![("id".to_string(), "2".to_string())]);
    }

    #[test]
    fn colunas_com_nome_invalido_ficam_de_fora() {
        let text = rows_to_xml(&s(&["nome", "COUNT(*)"]), &[s(&["Ana", "3"])]);
        assert_eq!(text, "<ROW nome=\"Ana\"/>\n");
    }

    #[test]
    fn acha_linhas_dentro_de_um_trecho_de_arquivo() {
        let text = "<ROWDATA>\n  <ROW id=\"7\"/>\n  <ROW id=\"8\"></ROW>\n</ROWDATA>";
        let rows = parse_rows(text).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1], vec![("id".to_string(), "8".to_string())]);
    }

    #[test]
    fn texto_sem_linhas_nao_cola_nada() {
        assert!(parse_rows("só um texto qualquer").is_none());
        assert!(parse_rows("<FIELD attrname=\"x\"/>").is_none());
    }
}
