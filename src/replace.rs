/// Critério de Localizar/Substituir aplicado a uma célula.
pub struct Matcher {
    needle: Vec<char>,
    case_sensitive: bool,
    /// A célula inteira precisa ser igual ao texto procurado (e não apenas
    /// contê-lo). Com o texto vazio, casa as células vazias.
    whole_cell: bool,
}

impl Matcher {
    pub fn new(needle: &str, case_sensitive: bool, whole_cell: bool) -> Self {
        Self { needle: needle.chars().collect(), case_sensitive, whole_cell }
    }

    /// `true` se não há o que procurar (texto vazio fora do modo célula inteira).
    pub fn is_empty(&self) -> bool {
        self.needle.is_empty() && !self.whole_cell
    }

    fn eq(&self, a: char, b: char) -> bool {
        a == b || (!self.case_sensitive && a.to_lowercase().eq(b.to_lowercase()))
    }

    /// Se `hay` começa, na posição `at`, com o texto procurado.
    fn match_at(&self, hay: &[char], at: usize) -> bool {
        hay.len() - at >= self.needle.len()
            && self.needle.iter().zip(&hay[at..]).all(|(&n, &h)| self.eq(n, h))
    }

    pub fn matches(&self, cell: &str) -> bool {
        if self.is_empty() {
            return false;
        }
        let hay: Vec<char> = cell.chars().collect();
        if self.whole_cell {
            return hay.len() == self.needle.len() && self.match_at(&hay, 0);
        }
        (0..=hay.len().saturating_sub(self.needle.len())).any(|i| self.match_at(&hay, i))
    }

    /// Substitui as ocorrências em `cell` por `with`. `None` se nada casou.
    pub fn replace(&self, cell: &str, with: &str) -> Option<String> {
        if !self.matches(cell) {
            return None;
        }
        if self.whole_cell {
            return Some(with.to_string());
        }
        let hay: Vec<char> = cell.chars().collect();
        let mut out = String::with_capacity(cell.len());
        let mut i = 0;
        while i < hay.len() {
            if self.match_at(&hay, i) {
                out.push_str(with);
                i += self.needle.len();
            } else {
                out.push(hay[i]);
                i += 1;
            }
        }
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn substitui_todas_as_ocorrencias_sem_diferenciar_maiusculas() {
        let m = Matcher::new("ana", false, false);
        assert_eq!(m.replace("Ana e ANA e banana", "X").unwrap(), "X e X e bXna");
        assert!(m.replace("Bruno", "X").is_none());
    }

    #[test]
    fn diferencia_maiusculas_quando_pedido() {
        let m = Matcher::new("Ana", true, false);
        assert_eq!(m.replace("Ana ana", "X").unwrap(), "X ana");
    }

    #[test]
    fn acentos_e_maiusculas_acentuadas() {
        let m = Matcher::new("débora", false, false);
        assert!(m.matches("DÉBORA Silva"));
        assert_eq!(m.replace("DÉBORA", "Ana").unwrap(), "Ana");
    }

    #[test]
    fn celula_inteira_exige_igualdade_e_casa_vazias() {
        let m = Matcher::new("0", false, true);
        assert!(m.matches("0"));
        assert!(!m.matches("10"));
        let vazias = Matcher::new("", false, true);
        assert!(vazias.matches(""));
        assert_eq!(vazias.replace("", "0").unwrap(), "0");
        assert!(!vazias.matches("x"));
    }

    #[test]
    fn texto_vazio_em_trecho_nao_casa_nada() {
        let m = Matcher::new("", false, false);
        assert!(m.is_empty());
        assert!(!m.matches("abc"));
    }
}
