use std::cmp::Ordering;

/// Chave de ordenação de uma célula. Os valores do XML são todos texto, então
/// a chave é deduzida do conteúdo: datas e números comparam pelo valor, o
/// resto como texto sem distinguir maiúsculas.
pub enum SortKey {
    Num(f64),
    Text(String),
    Empty,
}

pub fn sort_key(s: &str) -> SortKey {
    let t = s.trim();
    if t.is_empty() {
        return SortKey::Empty;
    }
    if let Some(d) = parse_date_br(t) {
        return SortKey::Num(d);
    }
    match crate::xmldoc::parse_number(t) {
        Some(n) if n.is_finite() => SortKey::Num(n),
        _ => SortKey::Text(t.to_lowercase()),
    }
}

/// Compara duas chaves. Números vêm antes de texto; células vazias ficam
/// sempre no fim, nos dois sentidos, para não esconder os dados.
pub fn cmp_keys(a: &SortKey, b: &SortKey, ascending: bool) -> Ordering {
    use SortKey::*;
    let ord = match (a, b) {
        (Empty, Empty) => return Ordering::Equal,
        (Empty, _) => return Ordering::Greater,
        (_, Empty) => return Ordering::Less,
        (Num(x), Num(y)) => x.partial_cmp(y).unwrap_or(Ordering::Equal),
        (Num(_), Text(_)) => Ordering::Less,
        (Text(_), Num(_)) => Ordering::Greater,
        (Text(x), Text(y)) => x.cmp(y),
    };
    if ascending {
        ord
    } else {
        ord.reverse()
    }
}

/// `dd/mm/aaaa` → `aaaammdd` como número, para datas ordenarem no tempo e não
/// pelo dia.
fn parse_date_br(s: &str) -> Option<f64> {
    let mut parts = s.split('/');
    let (d, m, y) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() || y.len() != 4 || d.len() > 2 || m.len() > 2 {
        return None;
    }
    let (d, m, y): (u32, u32, u32) = (d.parse().ok()?, m.parse().ok()?, y.parse().ok()?);
    if !(1..=31).contains(&d) || !(1..=12).contains(&m) {
        return None;
    }
    Some((y * 10_000 + m * 100 + d) as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ordena(vals: &[&str], asc: bool) -> Vec<String> {
        let mut v: Vec<(SortKey, &str)> = vals.iter().map(|s| (sort_key(s), *s)).collect();
        v.sort_by(|a, b| cmp_keys(&a.0, &b.0, asc));
        v.into_iter().map(|(_, s)| s.to_string()).collect()
    }

    #[test]
    fn numeros_comparam_pelo_valor_inclusive_no_formato_brasileiro() {
        assert_eq!(ordena(&["10", "9", "1.234,56", "100.5"], true), ["9", "10", "100.5", "1.234,56"]);
    }

    #[test]
    fn datas_comparam_no_tempo() {
        assert_eq!(
            ordena(&["01/02/2024", "31/01/2024", "15/12/2023"], true),
            ["15/12/2023", "31/01/2024", "01/02/2024"]
        );
    }

    #[test]
    fn vazios_ficam_no_fim_nos_dois_sentidos() {
        assert_eq!(ordena(&["", "b", "A", "2"], true), ["2", "A", "b", ""]);
        assert_eq!(ordena(&["", "b", "A", "2"], false), ["b", "A", "2", ""]);
    }
}
