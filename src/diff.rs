//! Diferença entre duas versões de uma tabela (arquivo em disco, cópia de
//! recuperação, versão do histórico ou o documento aberto), célula a célula.
//!
//! As linhas são casadas pelas colunas que existem nos dois lados; uma linha
//! apagada seguida de uma incluída parecida vira uma linha alterada, com as
//! células que mudaram marcadas. Coluna incluída ou apagada aparece à parte,
//! sem contar como mudança em todas as linhas.

use crate::xmldoc::XmlDoc;
use anyhow::Result;
use std::collections::HashMap;
use std::path::Path;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tag {
    Same,
    Added,
    Removed,
    /// Mesma linha com células diferentes: `old` tem os valores de antes.
    Changed,
    /// Trecho igual recolhido: o primeiro valor diz quantas linhas.
    Gap,
}

/// Situação de uma coluna entre as duas versões.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ColState {
    Same,
    Added,
    Removed,
}

pub struct DiffRow {
    pub tag: Tag,
    /// Valores na ordem de `Diff::columns`.
    pub values: Vec<String>,
    /// Em `Changed`, os valores de antes; vazio nos outros casos.
    pub old: Vec<String>,
}

pub struct Diff {
    pub columns: Vec<(String, ColState)>,
    pub rows: Vec<DiffRow>,
    pub added: usize,
    pub removed: usize,
    pub changed: usize,
    /// A lista foi cortada em `MAX_LINES`.
    pub truncated: bool,
}

/// Linhas iguais mostradas em volta de cada mudança.
const CONTEXT: usize = 2;
/// Teto de linhas exibidas, para a janela não travar com um arquivo enorme.
const MAX_LINES: usize = 20_000;
/// Acima disto (linhas × linhas no trecho que difere), a comparação exata
/// ficaria lenta demais; usa-se a comparação por conteúdo.
const LCS_LIMIT: usize = 4_000_000;

/// Uma tabela como `XmlDoc::filter` a devolve: nomes das colunas e linhas,
/// cada uma com o rowid interno na frente.
pub struct Table {
    pub names: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

impl Table {
    fn open(path: &Path) -> Result<Self> {
        let dp = XmlDoc::open(path)?;
        let names = dp.field_names();
        let rows = dp.filter(None)?.rows;
        Ok(Self { names, rows })
    }

    /// Posição, em cada linha, de cada coluna de `cols` (`None` se a
    /// tabela não tem a coluna). Calculada uma vez por comparação.
    fn index(&self, cols: &[String]) -> Vec<Option<usize>> {
        cols.iter().map(|c| self.names.iter().position(|n| n == c).map(|i| i + 1)).collect()
    }

    /// Valores da linha `row` nas posições `idx` ("" onde a coluna não existe).
    fn values(&self, row: usize, idx: &[Option<usize>]) -> Vec<String> {
        idx.iter()
            .map(|i| i.and_then(|i| self.rows[row].get(i).cloned()).unwrap_or_default())
            .collect()
    }
}

/// Compara o arquivo em disco (`before`) com a cópia recuperada (`after`).
pub fn diff_files(before: &Path, after: &Path) -> Result<Diff> {
    Ok(diff_tables(&Table::open(before)?, &Table::open(after)?))
}

/// Compara o arquivo `before` com uma tabela já carregada (o documento
/// aberto): o que mudou desde aquela versão.
pub fn diff_since(before: &Path, after: &Table) -> Result<Diff> {
    Ok(diff_tables(&Table::open(before)?, after))
}

enum Op {
    Same(usize, usize),
    Del(usize),
    Add(usize),
}

fn diff_tables(a: &Table, b: &Table) -> Diff {
    let mut common: Vec<String> = b.names.iter().filter(|n| a.names.contains(n)).cloned().collect();
    if common.is_empty() {
        // nenhuma coluna em comum: compara as linhas inteiras
        common = b.names.clone();
    }
    // colunas da versão nova e, no fim, as que foram apagadas
    let mut columns: Vec<(String, ColState)> = b
        .names
        .iter()
        .map(|n| (n.clone(), if a.names.contains(n) { ColState::Same } else { ColState::Added }))
        .collect();
    columns.extend(a.names.iter().filter(|n| !b.names.contains(n)).map(|n| (n.clone(), ColState::Removed)));
    let all: Vec<String> = columns.iter().map(|(n, _)| n.clone()).collect();

    let key = |t: &Table| -> Vec<String> {
        let idx = t.index(&common);
        (0..t.rows.len()).map(|r| t.values(r, &idx).join("\u{1f}")).collect()
    };
    let (ka, kb) = (key(a), key(b));
    let (ia, ib) = (a.index(&all), b.index(&all));
    // linha nas duas versões: colunas novas vêm de `b`, apagadas de `a`
    let both = |i: usize, j: usize| -> Vec<String> {
        let (va, vb) = (a.values(i, &ia), b.values(j, &ib));
        columns
            .iter()
            .zip(va.into_iter().zip(vb))
            .map(|((_, st), (x, y))| if *st == ColState::Removed { x } else { y })
            .collect()
    };

    let mut rows = Vec::new();
    let (mut added, mut removed, mut changed) = (0, 0, 0);
    let mut dels: Vec<usize> = Vec::new();
    let mut adds: Vec<usize> = Vec::new();
    // Fecha um trecho de apagadas/incluídas: casa as parecidas como alteradas.
    let mut flush = |dels: &mut Vec<usize>, adds: &mut Vec<usize>, rows: &mut Vec<DiffRow>| {
        let pairs = dels.len().min(adds.len());
        let mut rest_d = Vec::new();
        let mut rest_a = Vec::new();
        for k in 0..pairs {
            let (i, j) = (dels[k], adds[k]);
            if similar(&ka[i], &kb[j]) {
                rows.push(DiffRow { tag: Tag::Changed, values: both(i, j), old: a.values(i, &ia) });
                changed += 1;
            } else {
                rest_d.push(i);
                rest_a.push(j);
            }
        }
        rest_d.extend_from_slice(&dels[pairs..]);
        rest_a.extend_from_slice(&adds[pairs..]);
        for i in rest_d {
            rows.push(DiffRow { tag: Tag::Removed, values: a.values(i, &ia), old: Vec::new() });
            removed += 1;
        }
        for j in rest_a {
            rows.push(DiffRow { tag: Tag::Added, values: b.values(j, &ib), old: Vec::new() });
            added += 1;
        }
        dels.clear();
        adds.clear();
    };
    for op in diff_ops(&ka, &kb) {
        match op {
            Op::Del(i) => dels.push(i),
            Op::Add(j) => adds.push(j),
            Op::Same(i, j) => {
                flush(&mut dels, &mut adds, &mut rows);
                rows.push(DiffRow { tag: Tag::Same, values: both(i, j), old: Vec::new() });
            }
        }
    }
    flush(&mut dels, &mut adds, &mut rows);

    // Coluna incluída ou apagada muda todas as linhas: elas ficam à mostra
    // (sem recolher), para dar para ver os valores da coluna.
    let cols_changed = columns.iter().any(|(_, st)| *st != ColState::Same);
    let (rows, truncated) = if cols_changed {
        let truncated = rows.len() > MAX_LINES;
        (rows.into_iter().take(MAX_LINES).collect(), truncated)
    } else {
        collapse(rows)
    };
    Diff { columns, rows, added, removed, changed, truncated }
}

/// Duas linhas são "a mesma, alterada" se no máximo metade das células
/// mudou.
fn similar(a: &str, b: &str) -> bool {
    let (va, vb): (Vec<&str>, Vec<&str>) = (a.split('\u{1f}').collect(), b.split('\u{1f}').collect());
    let differ = va.iter().zip(&vb).filter(|(x, y)| x != y).count();
    differ * 2 <= va.len().max(1)
}

/// Operações que transformam `a` em `b`.
fn diff_ops(a: &[String], b: &[String]) -> Vec<Op> {
    // prefixo e sufixo iguais saem de graça; só o miolo é comparado
    let pre = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    let suf = a[pre..]
        .iter()
        .rev()
        .zip(b[pre..].iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let (ma, mb) = (&a[pre..a.len() - suf], &b[pre..b.len() - suf]);

    let mut out: Vec<Op> = (0..pre).map(|i| Op::Same(i, i)).collect();
    let mid = if ma.len().saturating_mul(mb.len()) <= LCS_LIMIT { lcs(ma, mb) } else { by_content(ma, mb) };
    out.extend(mid.into_iter().map(|op| match op {
        Op::Same(i, j) => Op::Same(i + pre, j + pre),
        Op::Del(i) => Op::Del(i + pre),
        Op::Add(j) => Op::Add(j + pre),
    }));
    let (ea, eb) = (a.len() - suf, b.len() - suf);
    out.extend((0..suf).map(|k| Op::Same(ea + k, eb + k)));
    out
}

/// Diferença exata (maior subsequência comum).
fn lcs(a: &[String], b: &[String]) -> Vec<Op> {
    let (n, m) = (a.len(), b.len());
    let w = m + 1;
    let mut t = vec![0u32; (n + 1) * w];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            t[i * w + j] = if a[i] == b[j] {
                t[(i + 1) * w + j + 1] + 1
            } else {
                t[(i + 1) * w + j].max(t[i * w + j + 1])
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    let mut out = Vec::new();
    while i < n && j < m {
        if a[i] == b[j] {
            out.push(Op::Same(i, j));
            i += 1;
            j += 1;
        } else if t[(i + 1) * w + j] >= t[i * w + j + 1] {
            out.push(Op::Del(i));
            i += 1;
        } else {
            out.push(Op::Add(j));
            j += 1;
        }
    }
    out.extend((i..n).map(Op::Del));
    out.extend((j..m).map(Op::Add));
    out
}

/// Para trechos enormes: apagadas são as linhas de `a` que sobram depois
/// de casar com as de `b` (contando repetições), e vice-versa. Não acompanha
/// mudanças de posição, mas aponta tudo o que entrou e saiu.
fn by_content(a: &[String], b: &[String]) -> Vec<Op> {
    let leftover = |from: &[String], other: &[String]| -> Vec<usize> {
        let mut count: HashMap<&str, i64> = HashMap::new();
        for l in other {
            *count.entry(l).or_default() += 1;
        }
        let mut out = Vec::new();
        for (i, l) in from.iter().enumerate() {
            let c = count.entry(l).or_default();
            if *c > 0 {
                *c -= 1;
            } else {
                out.push(i);
            }
        }
        out
    };
    let mut out: Vec<Op> = leftover(a, b).into_iter().map(Op::Del).collect();
    out.extend(leftover(b, a).into_iter().map(Op::Add));
    out
}

/// Recolhe os trechos iguais longos, mantendo `CONTEXT` linhas em volta de
/// cada mudança, e corta a lista em `MAX_LINES`.
fn collapse(rows: Vec<DiffRow>) -> (Vec<DiffRow>, bool) {
    let changed: Vec<usize> =
        rows.iter().enumerate().filter(|(_, r)| r.tag != Tag::Same).map(|(i, _)| i).collect();
    let near = |i: usize| {
        // há mudança a até CONTEXT linhas de i?
        let p = changed.partition_point(|&c| c + CONTEXT < i);
        changed.get(p).is_some_and(|&c| c <= i + CONTEXT)
    };
    let gap = |n: usize| DiffRow { tag: Tag::Gap, values: vec![n.to_string()], old: Vec::new() };
    let mut out = Vec::new();
    let mut skipped = 0;
    for (i, row) in rows.into_iter().enumerate() {
        if row.tag != Tag::Same || near(i) {
            if skipped > 0 {
                out.push(gap(skipped));
                skipped = 0;
            }
            out.push(row);
            if out.len() >= MAX_LINES {
                return (out, true);
            }
        } else {
            skipped += 1;
        }
    }
    if skipped > 0 {
        out.push(gap(skipped));
    }
    (out, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| x.to_string()).collect()
    }

    /// Tabela com uma linha por item de `rows` (valores separados por `|`).
    fn table(names: &[&str], rows: &[&str]) -> Table {
        Table {
            names: v(names),
            rows: rows
                .iter()
                .map(|r| std::iter::once("0").chain(r.split('|')).map(String::from).collect())
                .collect(),
        }
    }

    fn tags(d: &Diff) -> Vec<Tag> {
        d.rows.iter().map(|r| r.tag).collect()
    }

    #[test]
    fn aponta_linha_alterada_incluida_e_apagada() {
        let a = table(&["id", "v"], &["1|a", "2|b", "3|c", "4|d"]);
        let b = table(&["id", "v"], &["1|a", "2|X", "3|c", "9|z", "4|d"]);
        let d = diff_tables(&a, &b);
        use Tag::*;
        assert_eq!(tags(&d), [Same, Changed, Same, Added, Same]);
        assert_eq!(d.rows[1].old, ["2", "b"]);
        assert_eq!(d.rows[1].values, ["2", "X"]);
        assert_eq!((d.added, d.removed, d.changed), (1, 0, 1));
    }

    #[test]
    fn linhas_sem_relacao_nao_viram_alteracao() {
        let a = table(&["id", "v"], &["1|a", "2|b"]);
        let b = table(&["id", "v"], &["1|a", "7|q"]);
        let d = diff_tables(&a, &b);
        assert_eq!(tags(&d), [Tag::Same, Tag::Removed, Tag::Added]);
    }

    #[test]
    fn coluna_apagada_nao_marca_todas_as_linhas() {
        let a = table(&["id", "valor"], &["1|10", "2|20"]);
        let b = table(&["id"], &["1", "2"]);
        let d = diff_tables(&a, &b);
        assert_eq!((d.added, d.removed, d.changed), (0, 0, 0));
        assert_eq!(d.columns, [("id".to_string(), ColState::Same), ("valor".to_string(), ColState::Removed)]);
        // a coluna apagada continua mostrando o valor que tinha
        assert_eq!(d.rows[0].values, ["1", "10"]);
    }

    #[test]
    fn trechos_iguais_longos_sao_recolhidos() {
        let rows: Vec<String> = (0..100).map(|i| format!("{i}|x")).collect();
        let a = table(&["id", "v"], &rows.iter().map(String::as_str).collect::<Vec<_>>());
        let mut rb = rows.clone();
        rb[50] = "50|y".into();
        let b = table(&["id", "v"], &rb.iter().map(String::as_str).collect::<Vec<_>>());
        let d = diff_tables(&a, &b);
        assert!(!d.truncated);
        assert_eq!((d.rows[0].tag, d.rows[0].values[0].as_str()), (Tag::Gap, "48"));
        let last = d.rows.last().unwrap();
        assert_eq!((last.tag, last.values[0].as_str()), (Tag::Gap, "47"));
        assert_eq!(d.rows.iter().filter(|r| r.tag == Tag::Same).count(), 4);
    }

    #[test]
    fn trecho_enorme_usa_comparacao_por_conteudo() {
        let ops = by_content(&v(&["a", "b", "b"]), &v(&["b", "c"]));
        let got: Vec<String> = ops
            .iter()
            .map(|o| match o {
                Op::Del(i) => format!("-{i}"),
                Op::Add(j) => format!("+{j}"),
                Op::Same(..) => "=".into(),
            })
            .collect();
        assert_eq!(got, ["-0", "-2", "+1"]);
    }
}
