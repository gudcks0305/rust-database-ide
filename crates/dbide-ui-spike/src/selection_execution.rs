use std::ops::Range;
/// Exact selected UTF-8 bytes; never falls back to the rest of a script.
pub fn selected_sql(source: &str, range: Range<usize>) -> Option<&str> {
    source.get(range).filter(|s| !s.trim().is_empty())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selection_never_executes_surrounding_statements() {
        let s = "SELECT '한글';\nSELECT 2;\nSELECT 3;";
        let start = s.find("SELECT 2").unwrap();
        assert_eq!(selected_sql(s, start..start + 9), Some("SELECT 2;"));
        assert_eq!(selected_sql(s, 0..0), None);
        assert_eq!(selected_sql(s, usize::MAX..usize::MAX), None);
        let p = s.find('한').unwrap();
        assert_eq!(selected_sql(s, p + 1..p + 3), None);
        assert_eq!(selected_sql(" \n", 0..2), None);
    }
}
