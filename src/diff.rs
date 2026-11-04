//! Diferença, linha a linha, entre o arquivo em disco e uma cópia de
//! recuperação. Cada linha da tabela vira o seu `<ROW .../>`, e as duas
//! listas são comparadas como texto.

use crate::rowclip::rows_to_xml;
use crate::xmldoc::XmlDoc;
use anyhow::Result;
use std::collections::HashMap;
use std::path::Path;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tag {
    Same,
    Added,
    Removed,
    /// Trecho igual recolhido: o texto diz quantas linhas.
    Gap,
}

pub struct Diff {
    pub columns_before: Vec<String>,
    pub columns_after: Vec<String>,
    pub added: usize,
    pub removed: usize,
    pub lines: Vec<(Tag, String)>,
    /// A lista de linhas foi cortada em `MAX_LINES`.
    pub truncated: bool,
}

/// Linhas iguais mostradas em volta de cada mudança.
const CONTEXT: usize = 2;
/// Teto de linhas exibidas, para a janela não travar com um arquivo enorme.
const MAX_LINES: usize = 20_000;
/// Acima disto (linhas × linhas no trecho que difere), a comparação exata
/// ficaria lenta demais; usa-se a comparação por conteúdo.
const LCS_LIMIT: usize = 4_000_000;

fn row_lines(path: &Path) -> Result<(Vec<String>, Vec<String>)> {
    let dp = XmlDoc::open(path)?;
    let names = dp.field_names();
    let res = dp.filter(None)?;
    // a primeira coluna do resultado é o rowid interno
    let lines = res
        .rows
        .into_iter()
        .map(|r| rows_to_xml(&names, &[r[1..].to_vec()]).trim_end().to_string())
        .collect();
    Ok((names, lines))
}

/// Compara o arquivo em disco (`before`) com a cópia recuperada (`after`).
pub fn diff_files(before: &Path, after: &Path) -> Result<Diff> {
    let (columns_before, a) = row_lines(before)?;
    let (columns_after, b) = row_lines(after)?;
    let ops = diff_lines(&a, &b);
    let added = ops.iter().filter(|(t, _)| *t == Tag::Added).count();
    let removed = ops.iter().filter(|(t, _)| *t == Tag::Removed).count();
    let (lines, truncated) = collapse(ops);
    Ok(Diff { columns_before, columns_after, added, removed, lines, truncated })
}

/// Sequência de operações que transforma `a` em `b`.
pub fn diff_lines(a: &[String], b: &[String]) -> Vec<(Tag, String)> {
    // prefixo e sufixo iguais saem de graça; só o miolo é comparado
    let pre = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    let suf = a[pre..]
        .iter()
        .rev()
        .zip(b[pre..].iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let (ma, mb) = (&a[pre..a.len() - suf], &b[pre..b.len() - suf]);

    let mut out: Vec<(Tag, String)> = a[..pre].iter().map(|l| (Tag::Same, l.clone())).collect();
    if ma.len().saturating_mul(mb.len()) <= LCS_LIMIT {
        out.extend(lcs(ma, mb));
    } else {
        out.extend(by_content(ma, mb));
    }
    out.extend(a[a.len() - suf..].iter().map(|l| (Tag::Same, l.clone())));
    out
}

/// Diferença exata (maior subsequência comum).
fn lcs(a: &[String], b: &[String]) -> Vec<(Tag, String)> {
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
            out.push((Tag::Same, a[i].clone()));
            i += 1;
            j += 1;
        } else if t[(i + 1) * w + j] >= t[i * w + j + 1] {
            out.push((Tag::Removed, a[i].clone()));
            i += 1;
        } else {
            out.push((Tag::Added, b[j].clone()));
            j += 1;
        }
    }
    out.extend(a[i..].iter().map(|l| (Tag::Removed, l.clone())));
    out.extend(b[j..].iter().map(|l| (Tag::Added, l.clone())));
    out
}

/// Para trechos enormes: removidas são as linhas de `a` que sobram depois
/// de casar com as de `b` (contando repetições), e vice-versa. Não acompanha
/// mudanças de posição, mas aponta tudo o que entrou e saiu.
fn by_content(a: &[String], b: &[String]) -> Vec<(Tag, String)> {
    let mut count: HashMap<&str, i64> = HashMap::new();
    for l in b {
        *count.entry(l).or_default() += 1;
    }
    let mut out = Vec::new();
    for l in a {
        let c = count.entry(l).or_default();
        if *c > 0 {
            *c -= 1;
        } else {
            out.push((Tag::Removed, l.clone()));
        }
    }
    let mut count: HashMap<&str, i64> = HashMap::new();
    for l in a {
        *count.entry(l).or_default() += 1;
    }
    for l in b {
        let c = count.entry(l).or_default();
        if *c > 0 {
            *c -= 1;
        } else {
            out.push((Tag::Added, l.clone()));
        }
    }
    out
}

/// Recolhe os trechos iguais longos, mantendo `CONTEXT` linhas em volta de
/// cada mudança, e corta a lista em `MAX_LINES`.
fn collapse(ops: Vec<(Tag, String)>) -> (Vec<(Tag, String)>, bool) {
    let changed: Vec<usize> =
        ops.iter().enumerate().filter(|(_, (t, _))| *t != Tag::Same).map(|(i, _)| i).collect();
    let near = |i: usize| {
        // há mudança a até CONTEXT linhas de i?
        let p = changed.partition_point(|&c| c + CONTEXT < i);
        changed.get(p).is_some_and(|&c| c <= i + CONTEXT)
    };
    let mut out = Vec::new();
    let mut skipped = 0;
    for (i, op) in ops.into_iter().enumerate() {
        if op.0 != Tag::Same || near(i) {
            if skipped > 0 {
                out.push((Tag::Gap, skipped.to_string()));
                skipped = 0;
            }
            out.push(op);
            if out.len() >= MAX_LINES {
                return (out, true);
            }
        } else {
            skipped += 1;
        }
    }
    if skipped > 0 {
        out.push((Tag::Gap, skipped.to_string()));
    }
    (out, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn aponta_linha_alterada_adicionada_e_removida() {
        let a = v(&["1", "2", "3", "4"]);
        let b = v(&["1", "2x", "3", "5", "4"]);
        let ops = diff_lines(&a, &b);
        let tags: Vec<Tag> = ops.iter().map(|(t, _)| *t).collect();
        use Tag::*;
        assert_eq!(tags, [Same, Removed, Added, Same, Added, Same]);
    }

    #[test]
    fn trechos_iguais_longos_sao_recolhidos() {
        let a: Vec<String> = (0..100).map(|i| i.to_string()).collect();
        let mut b = a.clone();
        b[50] = "x".into();
        let (lines, truncated) = collapse(diff_lines(&a, &b));
        assert!(!truncated);
        assert_eq!(lines[0], (Tag::Gap, "48".into()));
        assert_eq!(lines.last().unwrap(), &(Tag::Gap, "47".into()));
        assert_eq!(lines.iter().filter(|(t, _)| *t == Tag::Same).count(), 4);
    }

    #[test]
    fn trecho_enorme_usa_comparacao_por_conteudo() {
        let a = v(&["a", "b", "b"]);
        let b = v(&["b", "c"]);
        let ops = by_content(&a, &b);
        assert_eq!(ops, [(Tag::Removed, "a".into()), (Tag::Removed, "b".into()), (Tag::Added, "c".into())]);
    }
}
