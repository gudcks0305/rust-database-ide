//! M0 parser comparison. These parsers do not gate arbitrary SQL execution.
use serde::Serialize;
use sqlparser::{dialect::PostgreSqlDialect, parser::Parser as SqlParser};
use std::time::Instant;
use tree_sitter::{InputEdit, Parser, Point, Tree};

pub struct Fixture {
    pub name: &'static str,
    pub sql: &'static str,
    pub expected_statements: Option<usize>,
}

pub const FIXTURES: &[Fixture] = &[
    Fixture {
        name: "literal_semicolon",
        sql: "SELECT ';' AS value; SELECT 2;",
        expected_statements: Some(2),
    },
    Fixture {
        name: "comments",
        sql: "SELECT 1 /* ; /* nested ; */ */; -- ;\nSELECT 2;",
        expected_statements: Some(2),
    },
    Fixture {
        name: "dollar_quote",
        sql: "SELECT $body$한글; 'value'$body$; SELECT 2;",
        expected_statements: Some(2),
    },
    Fixture {
        name: "cte_alias",
        sql: "WITH x AS (SELECT 1 AS id), y AS (SELECT id FROM x) SELECT y.id FROM y;",
        expected_statements: Some(1),
    },
    Fixture {
        name: "korean_identifiers",
        sql: "SELECT '안녕하세요 👋' AS \"이름\", 'é' AS combining;",
        expected_statements: Some(1),
    },
    Fixture {
        name: "transaction_script",
        sql: "BEGIN; CREATE TEMP TABLE t(id integer); INSERT INTO t VALUES (1); ROLLBACK;",
        expected_statements: Some(4),
    },
    Fixture {
        name: "postgres_function",
        sql: "CREATE FUNCTION f() RETURNS int LANGUAGE plpgsql AS $$ BEGIN RETURN 1; END $$; SELECT f();",
        expected_statements: None,
    },
    Fixture {
        name: "copy_protocol",
        sql: "COPY t FROM STDIN;",
        expected_statements: None,
    },
    Fixture {
        name: "unfinished_select",
        sql: "SELECT id, FROM",
        expected_statements: None,
    },
    Fixture {
        name: "unfinished_quote",
        sql: "SELECT '한글",
        expected_statements: None,
    },
    Fixture {
        name: "postgres_escape",
        sql: r"SELECT E'escaped\';inside'; SELECT 2;",
        expected_statements: Some(2),
    },
];

#[derive(Serialize)]
pub struct FixtureResult {
    pub name: String,
    pub input_bytes: usize,
    pub sqlparser_statements: Option<usize>,
    pub sqlparser_error: Option<String>,
    pub expected_statements: Option<usize>,
    pub contract_passed: Option<bool>,
    pub tree_sitter_has_error: bool,
    pub tree_sitter_root: String,
    pub tree_sitter_named_children: usize,
    pub tree_sitter_bytes_covered: usize,
}

pub fn parser() -> Parser {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_sequel::LANGUAGE.into())
        .expect("pinned SQL grammar ABI");
    parser
}

pub fn inspect(fixture: &Fixture, parser: &mut Parser) -> FixtureResult {
    let parsed = SqlParser::parse_sql(&PostgreSqlDialect {}, fixture.sql);
    let (count, error) = match parsed {
        Ok(ast) => (Some(ast.len()), None),
        Err(error) => (None, Some(error.to_string())),
    };
    let tree = parser.parse(fixture.sql, None).expect("uncancelled parse");
    FixtureResult {
        name: fixture.name.into(),
        input_bytes: fixture.sql.len(),
        sqlparser_statements: count,
        sqlparser_error: error,
        expected_statements: fixture.expected_statements,
        contract_passed: fixture
            .expected_statements
            .map(|expected| count == Some(expected)),
        tree_sitter_has_error: tree.root_node().has_error(),
        tree_sitter_root: tree.root_node().kind().into(),
        tree_sitter_named_children: tree.root_node().named_child_count(),
        tree_sitter_bytes_covered: tree.root_node().end_byte(),
    }
}

pub fn byte_point(text: &str, offset: usize) -> Point {
    assert!(text.is_char_boundary(offset));
    let prefix = &text[..offset];
    Point {
        row: prefix.bytes().filter(|&b| b == b'\n').count(),
        column: prefix.rsplit('\n').next().unwrap_or("").len(),
    }
}

/// Apply one insertion using UTF-8 byte locations, preserving the old tree for incremental parse.
pub fn insert(
    parser: &mut Parser,
    old: &str,
    tree: &Tree,
    offset: usize,
    text: &str,
) -> (String, Tree) {
    let mut updated = old.to_owned();
    updated.insert_str(offset, text);
    let mut edited = tree.clone();
    edited.edit(&InputEdit {
        start_byte: offset,
        old_end_byte: offset,
        new_end_byte: offset + text.len(),
        start_position: byte_point(old, offset),
        old_end_position: byte_point(old, offset),
        new_end_position: byte_point(&updated, offset + text.len()),
    });
    let result = parser
        .parse(&updated, Some(&edited))
        .expect("uncancelled incremental parse");
    (updated, result)
}

#[derive(Serialize)]
pub struct ParseMeasurement {
    pub target_bytes: usize,
    pub actual_bytes: usize,
    pub iterations: usize,
    pub cold_tree_parse_us: u128,
    pub incremental_insert_us: Vec<u128>,
    pub median_incremental_us: u128,
    pub incremental_matches_full_reparse: bool,
    pub has_error: bool,
}

/// Compare structure AND source coordinates without allocating two giant S-expressions.
pub fn equivalent_trees(left: &Tree, right: &Tree) -> bool {
    let (mut a, mut b) = (left.walk(), right.walk());
    loop {
        let (x, y) = (a.node(), b.node());
        if x.kind() != y.kind()
            || x.range() != y.range()
            || x.is_missing() != y.is_missing()
            || x.is_named() != y.is_named()
            || x.has_error() != y.has_error()
        {
            return false;
        }
        match (a.goto_first_child(), b.goto_first_child()) {
            (true, true) => continue,
            (false, false) => {}
            _ => return false,
        }
        loop {
            match (a.goto_next_sibling(), b.goto_next_sibling()) {
                (true, true) => break,
                (false, false) => {}
                _ => return false,
            }
            match (a.goto_parent(), b.goto_parent()) {
                (true, true) => {}
                (false, false) => return true,
                _ => return false,
            }
        }
    }
}

pub fn measure(target_bytes: usize, iterations: usize) -> ParseMeasurement {
    let line = "SELECT '한글' AS label, 42 AS value;\n";
    let source = line.repeat(target_bytes.div_ceil(line.len()));
    let mut parser = parser();
    let started = Instant::now();
    let original = parser.parse(&source, None).unwrap();
    let cold_tree_parse_us = started.elapsed().as_micros();
    let offset = source.find("한글").unwrap() + "한글".len();
    let mut samples = Vec::with_capacity(iterations);
    let mut same = true;
    for _ in 0..iterations {
        let started = Instant::now();
        let (updated, tree) = insert(&mut parser, &source, &original, offset, " 입력");
        samples.push(started.elapsed().as_micros());
        let full = parser.parse(&updated, None).unwrap();
        same &= equivalent_trees(&tree, &full);
    }
    let mut sorted = samples.clone();
    sorted.sort_unstable();
    ParseMeasurement {
        target_bytes,
        actual_bytes: source.len(),
        iterations,
        cold_tree_parse_us,
        median_incremental_us: sorted[sorted.len() / 2],
        incremental_insert_us: samples,
        incremental_matches_full_reparse: same,
        has_error: original.root_node().has_error(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn postgres_statement_boundaries_respect_literals_comments_and_bodies() {
        let mut parser = parser();
        for fixture in FIXTURES.iter().filter(|f| f.expected_statements.is_some()) {
            let result = inspect(fixture, &mut parser);
            assert_eq!(
                result.contract_passed,
                Some(true),
                "{}: {:?}",
                fixture.name,
                result.sqlparser_error
            );
        }
    }
    #[test]
    fn incomplete_input_is_represented_without_a_successful_ast_requirement() {
        let mut parser = parser();
        for name in ["unfinished_select", "unfinished_quote"] {
            let result = inspect(
                FIXTURES.iter().find(|f| f.name == name).unwrap(),
                &mut parser,
            );
            assert!(result.sqlparser_error.is_some());
            // This permissive grammar can parse FROM as an identifier. A tree without
            // ERROR nodes is not validation: retain the full input and parser diagnostic.
            assert_eq!(result.tree_sitter_bytes_covered, result.input_bytes);
        }
    }
    #[test]
    fn utf8_multiline_edit_matches_full_reparse() {
        let source = "SELECT '한글';\nSELECT 2;";
        let mut parser = parser();
        let old = parser.parse(source, None).unwrap();
        let offset = source.find("한글").unwrap() + "한".len();
        assert_eq!(byte_point(source, offset), Point { row: 0, column: 11 });
        let (text, changed) = insert(&mut parser, source, &old, offset, "\n😊");
        let fresh = parser.parse(&text, None).unwrap();
        assert!(equivalent_trees(&changed, &fresh));
        assert_eq!(changed.root_node().end_byte(), text.len());
    }
    #[test]
    fn comparison_catches_equal_shape_with_different_byte_ranges() {
        let mut parser = parser();
        let a = parser.parse("SELECT 1;", None).unwrap();
        let b = parser.parse("SELECT  1;", None).unwrap();
        assert_eq!(a.root_node().to_sexp(), b.root_node().to_sexp());
        assert!(!equivalent_trees(&a, &b));
    }
}
