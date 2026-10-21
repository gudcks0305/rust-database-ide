//! Source-preserving SQL editor services. Analysis never gates SQL execution.
use sha2::{Digest, Sha256};
use sqlparser::{
    ast::{
        Expr, Ident, JoinConstraint, JoinOperator, ObjectName, Query, Select, SelectItem,
        SelectItemQualifiedWildcardKind, SetExpr, Spanned, Statement, TableFactor, Visit, Visitor,
    },
    dialect::{Dialect as ParserDialect, MySqlDialect, PostgreSqlDialect, SQLiteDialect},
    parser::Parser,
    tokenizer::{Location, Span, Token, TokenWithSpan, Tokenizer, Whitespace},
};
use std::{
    collections::BTreeSet,
    fmt,
    ops::{ControlFlow, Range},
};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Dialect {
    #[default]
    PostgreSql,
    SQLite,
    MySql,
}
impl Dialect {
    fn parser(self) -> Box<dyn ParserDialect> {
        match self {
            Self::PostgreSql => Box::new(PostgreSqlDialect {}),
            Self::SQLite => Box::new(SQLiteDialect {}),
            Self::MySql => Box::new(MySqlDialect {}),
        }
    }
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Catalog {
    pub tables: Vec<CatalogTable>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CatalogTable {
    pub schema: String,
    pub name: String,
    pub columns: Vec<String>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SqlError(pub String);
impl fmt::Display for SqlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for SqlError {}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiagnosticKind {
    Syntax,
    Semantic,
    Unsupported,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Diagnostic {
    pub kind: DiagnosticKind,
    pub range: Range<usize>,
    pub message: String,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SymbolKind {
    Alias,
    Cte,
    Table,
    Column,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Symbol {
    pub name: String,
    pub kind: SymbolKind,
    pub declaration: Option<Range<usize>>,
    pub references: Vec<Range<usize>>,
    pub catalog_table: Option<(String, String)>,
}
#[derive(Clone, Debug, Default)]
pub struct Analysis {
    pub symbols: Vec<Symbol>,
    pub diagnostics: Vec<Diagnostic>,
}
impl Analysis {
    /// Returns a uniquely bound symbol at a UTF-8 byte position.
    pub fn symbol_at(&self, cursor: usize) -> Option<&Symbol> {
        let mut matches = self.symbols.iter().filter(|s| {
            s.declaration
                .iter()
                .chain(s.references.iter())
                .any(|r| r.contains(&cursor))
        });
        let result = matches.next()?;
        if matches.next().is_some() {
            None
        } else {
            Some(result)
        }
    }
}

/// Converts only valid UTF-8 boundaries. UTF-16 offsets count code units, not graphemes.
pub fn byte_to_utf16(sql: &str, byte: usize) -> Option<usize> {
    sql.get(..byte).map(|s| s.encode_utf16().count())
}
pub fn utf16_to_byte(sql: &str, units: usize) -> Option<usize> {
    let mut count = 0;
    for (byte, ch) in sql.char_indices() {
        if count == units {
            return Some(byte);
        }
        count += ch.len_utf16();
        if count > units {
            return None;
        }
    }
    (count == units).then_some(sql.len())
}
fn offset(sql: &str, location: Location) -> Option<usize> {
    if location.line == 0 || location.column == 0 {
        return None;
    }
    let (mut line, mut col) = (1, 1);
    for (byte, ch) in sql.char_indices() {
        if line == location.line && col == location.column {
            return Some(byte);
        }
        if ch == '\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
    }
    (line == location.line && col == location.column).then_some(sql.len())
}
fn range(sql: &str, span: Span) -> Option<Range<usize>> {
    Some(offset(sql, span.start)?..offset(sql, span.end)?)
}
fn tokens(sql: &str, dialect: Dialect) -> (Vec<TokenWithSpan>, Option<String>) {
    let dialect = dialect.parser();
    let mut out = Vec::new();
    let error = Tokenizer::new(dialect.as_ref(), sql)
        .tokenize_with_location_into_buf(&mut out)
        .err()
        .map(|e| e.to_string());
    (out, error)
}
fn comment(token: &Token) -> bool {
    matches!(
        token,
        Token::Whitespace(Whitespace::SingleLineComment { .. } | Whitespace::MultiLineComment(_))
    )
}
fn significant(token: &Token) -> bool {
    !matches!(token, Token::Whitespace(_) | Token::EOF)
}
fn parse(sql: &str, dialect: Dialect) -> Result<Vec<Statement>, SqlError> {
    Parser::parse_sql(dialect.parser().as_ref(), sql).map_err(|e| SqlError(e.to_string()))
}

/// SQL AST formatting is deliberately refused for commented input: the formatter
/// discards comments (including optimizer/security hints). The caller retains source.
pub fn format_sql(sql: &str, dialect: Dialect) -> Result<String, SqlError> {
    let (tokens, error) = tokens(sql, dialect);
    if let Some(error) = error {
        return Err(SqlError(error));
    }
    if tokens.iter().any(|t| comment(&t.token)) || sql.contains("/*!") {
        return Err(SqlError(
            "Formatting would discard comments; original source retained".into(),
        ));
    }
    let ast = parse(sql, dialect)?;
    if ast.is_empty() {
        return Ok(sql.to_owned());
    }
    let mut formatted = ast
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(";\n");
    if sql.trim_end().ends_with(';') {
        formatted.push(';');
    }
    // Ensure the formatter's output retains the same normalized AST.
    if parse(&formatted, dialect)?
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        != ast.iter().map(ToString::to_string).collect::<Vec<_>>()
    {
        return Err(SqlError(
            "Formatter could not verify equivalent syntax".into(),
        ));
    }
    Ok(formatted)
}

/// Token-based boundaries preserve quoted semicolons, nested comments, dollar
/// bodies and BEGIN ATOMIC / routine BEGIN..END blocks. Unsupported procedural
/// control flow conservatively returns the whole routine rather than executing a fragment.
pub fn statement_ranges(sql: &str, dialect: Dialect) -> Vec<Range<usize>> {
    boundaries(sql, dialect)
        .into_iter()
        .map(|(range, _)| range)
        .collect()
}

// The bool is executable-current-statement confidence, not SQL validity. Native
// procedural bodies require a dialect procedural grammar for such confidence.
fn boundaries(sql: &str, dialect: Dialect) -> Vec<(Range<usize>, bool)> {
    let (tokens, error) = tokens(sql, dialect);
    let mut ranges = Vec::new();
    let (mut start, mut depth, mut block, mut routine) = (0, 0usize, 0usize, false);
    let mut saw_create = false;
    let mut code_tokens = 0;
    let mut safe_current = true;
    let mut has_code = false;
    let mut retain_routine_tail = false;
    for (index, t) in tokens.iter().enumerate() {
        if significant(&t.token) && t.token != Token::SemiColon {
            has_code = true;
        }
        match &t.token {
            Token::Word(w) if w.quote_style.is_none() => {
                let word = w.value.to_uppercase();
                saw_create |= word == "CREATE";
                routine |= saw_create
                    && matches!(
                        word.as_str(),
                        "FUNCTION" | "PROCEDURE" | "TRIGGER" | "EVENT"
                    );
                // Nested vendor control flow needs a dialect procedural grammar.
                // Keep its complete tail rather than falsely splitting END IF/CASE.
                retain_routine_tail |= (routine || block > 0)
                    && matches!(word.as_str(), "IF" | "CASE" | "LOOP" | "REPEAT" | "WHILE");
                if word == "BEGIN" {
                    let next = tokens[index + 1..]
                        .iter()
                        .find(|t| significant(&t.token))
                        .map(|t| &t.token);
                    let transaction_begin = code_tokens == 0
                        && match next {
                            None | Some(Token::SemiColon) => true,
                            Some(Token::Word(w)) if w.quote_style.is_none() => matches!(
                                w.value.to_uppercase().as_str(),
                                "WORK"
                                    | "TRANSACTION"
                                    | "ISOLATION"
                                    | "READ"
                                    | "DEFERRABLE"
                                    | "NOT"
                            ),
                            _ => false,
                        };
                    if !transaction_begin {
                        // This recognizes blocks independently of CREATE/EXPLAIN,
                        // object kind, or wrappers. Never expose their interior
                        // statements for direct execution if grammar is unknown.
                        block += 1;
                        safe_current = false;
                    }
                }
                if word == "END" && block > 0 {
                    block -= 1;
                }
            }
            Token::LParen => depth += 1,
            Token::RParen => depth = depth.saturating_sub(1),
            Token::SemiColon if depth == 0 && block == 0 && !retain_routine_tail => {
                if let Some(end) = offset(sql, t.span.end) {
                    if has_code {
                        ranges.push((start..end, safe_current));
                    }
                    start = end;
                    has_code = false;
                    routine = false;
                    saw_create = false;
                    code_tokens = 0;
                    safe_current = true;
                }
            }
            _ => {}
        }
        if significant(&t.token) && t.token != Token::SemiColon {
            code_tokens += 1;
        }
    }
    if (has_code || error.is_some()) && start < sql.len() {
        ranges.push((start..sql.len(), safe_current && error.is_none()));
    }
    ranges
}
pub fn statement_at_cursor(sql: &str, cursor: usize, dialect: Dialect) -> Option<Range<usize>> {
    if !sql.is_char_boundary(cursor) {
        return None;
    }
    let ranges = boundaries(sql, dialect);
    let (selected, safe_current) = ranges
        .iter()
        .find(|(r, _)| r.contains(&cursor))
        .cloned()
        .or_else(|| {
            if cursor == sql.len() {
                ranges.last().cloned()
            } else {
                None
            }
        })?;
    if !safe_current {
        return None;
    }
    let (selected_tokens, _) = tokens(&sql[selected.clone()], dialect);
    let words: Vec<_> = selected_tokens
        .iter()
        .filter_map(|t| match &t.token {
            Token::Word(w) if w.quote_style.is_none() => Some(w.value.to_uppercase()),
            _ => None,
        })
        .collect();
    if words.iter().any(|w| w == "CREATE")
        && words
            .iter()
            .any(|w| matches!(w.as_str(), "FUNCTION" | "PROCEDURE" | "TRIGGER" | "EVENT"))
        && words
            .iter()
            .any(|w| matches!(w.as_str(), "IF" | "CASE" | "LOOP" | "REPEAT" | "WHILE"))
    {
        return None;
    }
    Some(selected)
}

fn key(id: &Ident, dialect: Dialect) -> String {
    if id.quote_style.is_some() {
        id.value.clone()
    } else {
        match dialect {
            Dialect::PostgreSql => id.value.to_lowercase(),
            _ => id.value.to_ascii_lowercase(),
        }
    }
}
fn matches_name(id: &Ident, name: &str, dialect: Dialect) -> bool {
    if dialect == Dialect::PostgreSql || id.quote_style.is_some() {
        key(id, dialect) == name
    } else {
        id.value.eq_ignore_ascii_case(name)
    }
}
fn parts(name: &ObjectName) -> Option<Vec<&Ident>> {
    name.0.iter().map(|p| p.as_ident()).collect()
}
#[derive(Clone)]
struct Binding {
    name: String,
    symbol: usize,
    columns: Vec<String>,
    target: Option<(String, String)>,
    visible_after: usize,
}
#[derive(Default)]
struct Frame {
    ctes: Vec<Binding>,
    locals: Vec<Binding>,
}
struct Binder<'a> {
    sql: &'a str,
    dialect: Dialect,
    catalog: &'a Catalog,
    analysis: Analysis,
    frames: Vec<Frame>,
    completion: Vec<String>,
    completion_relations: Vec<String>,
}
impl<'a> Binder<'a> {
    fn new(sql: &'a str, dialect: Dialect, catalog: &'a Catalog) -> Self {
        Self {
            sql,
            dialect,
            catalog,
            analysis: Analysis::default(),
            frames: vec![],
            completion: vec![],
            completion_relations: vec![],
        }
    }
    fn diag(&mut self, kind: DiagnosticKind, span: Span, message: impl Into<String>) {
        self.analysis.diagnostics.push(Diagnostic {
            kind,
            range: range(self.sql, span).unwrap_or(0..self.sql.len()),
            message: message.into(),
        });
    }
    fn symbol(
        &mut self,
        id: &Ident,
        kind: SymbolKind,
        declaration: bool,
        target: Option<(String, String)>,
    ) -> usize {
        if kind == SymbolKind::Table
            && let Some(existing) = self
                .analysis
                .symbols
                .iter()
                .position(|s| s.kind == kind && s.catalog_table == target)
        {
            self.reference(existing, id);
            return existing;
        }
        let i = self.analysis.symbols.len();
        self.analysis.symbols.push(Symbol {
            name: key(id, self.dialect),
            kind,
            declaration: if declaration {
                range(self.sql, id.span)
            } else {
                None
            },
            references: if declaration {
                vec![]
            } else {
                range(self.sql, id.span).into_iter().collect()
            },
            catalog_table: target,
        });
        i
    }
    fn reference(&mut self, symbol: usize, id: &Ident) {
        if let Some(r) = range(self.sql, id.span) {
            let refs = &mut self.analysis.symbols[symbol].references;
            if !refs.contains(&r) {
                refs.push(r);
            }
        }
    }
    fn cte(&self, id: &Ident) -> Option<Binding> {
        self.frames
            .iter()
            .rev()
            .flat_map(|f| f.ctes.iter().rev())
            .find(|b| {
                matches_name(id, &b.name, self.dialect)
                    && offset(self.sql, id.span.start).is_some_and(|p| p >= b.visible_after)
            })
            .cloned()
    }
    fn table(&mut self, factor: &TableFactor) -> Option<Binding> {
        let TableFactor::Table {
            name,
            alias,
            args: None,
            ..
        } = factor
        else {
            self.diag(
                DiagnosticKind::Unsupported,
                factor.span(),
                "Derived tables, table functions and special FROM factors are not bound",
            );
            return None;
        };
        let ids = parts(name)?;
        let last = ids.last()?;
        if last.value == "__dbide_completion__" {
            let cursor = offset(self.sql, last.span.start).unwrap_or(0);
            self.completion_relations.extend(
                self.frames
                    .iter()
                    .rev()
                    .flat_map(|f| &f.ctes)
                    .filter(|b| b.visible_after <= cursor)
                    .map(|b| b.name.clone()),
            );
            return None;
        }
        let mut binding = if ids.len() == 1 { self.cte(last) } else { None };
        if let Some(b) = &binding {
            let decl = self.analysis.symbols[b.symbol].declaration.clone();
            if decl
                .as_ref()
                .is_some_and(|r| r.start >= offset(self.sql, last.span.start).unwrap_or(0))
            {
                self.diag(
                    DiagnosticKind::Unsupported,
                    name.span(),
                    "Forward or recursive CTE reference is not bound",
                );
                return None;
            }
            self.reference(b.symbol, last);
        }
        if binding.is_none() {
            let candidates: Vec<_> = self
                .catalog
                .tables
                .iter()
                .filter(|t| {
                    matches_name(last, &t.name, self.dialect)
                        && (ids.len() == 1
                            || (ids.len() == 2 && matches_name(ids[0], &t.schema, self.dialect)))
                })
                .collect();
            if candidates.len() != 1 {
                self.diag(
                    DiagnosticKind::Semantic,
                    name.span(),
                    if candidates.is_empty() {
                        format!("Unresolved table {name}")
                    } else {
                        format!("Ambiguous table {name}; qualify its schema")
                    },
                );
                return None;
            }
            let t = candidates[0];
            let target = Some((t.schema.clone(), t.name.clone()));
            let symbol = self.symbol(last, SymbolKind::Table, false, target.clone());
            binding = Some(Binding {
                name: key(last, self.dialect),
                symbol,
                columns: t.columns.clone(),
                target,
                visible_after: 0,
            });
        }
        let mut b = binding?;
        if let Some(alias) = alias {
            if !alias.columns.is_empty() {
                self.diag(
                    DiagnosticKind::Unsupported,
                    alias.name.span,
                    "Table column alias lists are not bound",
                );
                return None;
            }
            b.name = key(&alias.name, self.dialect);
            b.symbol = self.symbol(&alias.name, SymbolKind::Alias, true, b.target.clone());
        }
        Some(b)
    }
    fn bind_column(&mut self, ids: &[Ident]) {
        let Some(column) = ids.last() else {
            return;
        };
        if ids.len() > 2 {
            self.diag(
                DiagnosticKind::Unsupported,
                column.span,
                "Three-part expression references are not bound",
            );
            return;
        }
        let mut candidates = Vec::new();
        for frame in self.frames.iter().rev() {
            candidates = frame
                .locals
                .iter()
                .filter(|b| {
                    if ids.len() == 2 {
                        matches_name(&ids[0], &b.name, self.dialect)
                    } else {
                        b.columns
                            .iter()
                            .any(|c| matches_name(column, c, self.dialect))
                    }
                })
                .cloned()
                .collect();
            if !candidates.is_empty() {
                break;
            }
        }
        if column.value == "__dbide_completion__" {
            if ids.len() == 1 {
                let columns: Vec<_> = self
                    .frames
                    .last()
                    .into_iter()
                    .flat_map(|f| &f.locals)
                    .flat_map(|b| b.columns.clone())
                    .collect();
                self.completion.extend(
                    columns
                        .iter()
                        .filter(|c| columns.iter().filter(|other| other == c).count() == 1)
                        .cloned(),
                );
            } else if candidates.len() == 1 {
                self.completion.extend(candidates[0].columns.clone());
            }
            return;
        }
        if candidates.len() != 1 {
            self.diag(
                DiagnosticKind::Semantic,
                column.span,
                if candidates.is_empty() {
                    format!("Unresolved column {column}")
                } else {
                    format!("Ambiguous column {column}")
                },
            );
            return;
        }
        let b = &candidates[0];
        if ids.len() == 2 {
            self.reference(b.symbol, &ids[0]);
        }
        if !b
            .columns
            .iter()
            .any(|c| matches_name(column, c, self.dialect))
        {
            self.diag(
                DiagnosticKind::Semantic,
                column.span,
                format!("Unresolved column {column}"),
            );
            return;
        }
        let symbol = self.analysis.symbols.iter().position(|s| {
            s.kind == SymbolKind::Column
                && s.name == key(column, self.dialect)
                && s.catalog_table == b.target
                && b.target.is_some()
        });
        if let Some(symbol) = symbol {
            self.reference(symbol, column);
        } else {
            self.symbol(column, SymbolKind::Column, false, b.target.clone());
        }
    }
}
fn output_columns(query: &Query, dialect: Dialect) -> Vec<String> {
    let SetExpr::Select(select) = query.body.as_ref() else {
        return vec![];
    };
    select
        .projection
        .iter()
        .filter_map(|p| match p {
            SelectItem::ExprWithAlias { alias, .. } => Some(key(alias, dialect)),
            SelectItem::UnnamedExpr(Expr::Identifier(id)) => Some(key(id, dialect)),
            SelectItem::UnnamedExpr(Expr::CompoundIdentifier(ids)) => {
                ids.last().map(|i| key(i, dialect))
            }
            _ => None,
        })
        .collect()
}
impl Visitor for Binder<'_> {
    type Break = ();
    fn pre_visit_query(&mut self, query: &Query) -> ControlFlow<()> {
        let mut frame = Frame::default();
        if !matches!(query.body.as_ref(), SetExpr::Select(_) | SetExpr::Query(_)) {
            self.diag(
                DiagnosticKind::Unsupported,
                query.span(),
                "Set operations and non-SELECT query bodies are not bound for refactoring",
            );
        }
        if let Some(with) = &query.with {
            if with.recursive {
                self.diag(
                    DiagnosticKind::Unsupported,
                    query.span(),
                    "Recursive CTE semantics are not supported for refactoring",
                );
            }
            let mut cte_names = BTreeSet::new();
            for cte in &with.cte_tables {
                if !cte_names.insert(key(&cte.alias.name, self.dialect)) {
                    self.diag(
                        DiagnosticKind::Semantic,
                        cte.alias.name.span,
                        "Duplicate CTE name in query scope",
                    );
                }
                let symbol = self.symbol(&cte.alias.name, SymbolKind::Cte, true, None);
                let columns = if cte.alias.columns.is_empty() {
                    output_columns(&cte.query, self.dialect)
                } else {
                    cte.alias
                        .columns
                        .iter()
                        .map(|c| key(&c.name, self.dialect))
                        .collect()
                };
                frame.ctes.push(Binding {
                    name: key(&cte.alias.name, self.dialect),
                    symbol,
                    columns,
                    target: None,
                    visible_after: offset(self.sql, cte.query.span().end).unwrap_or(self.sql.len()),
                });
            }
        }
        if !query.locks.is_empty() {
            self.diag(
                DiagnosticKind::Unsupported,
                query.span(),
                "Lock target references are not bound for refactoring",
            );
        }
        self.frames.push(frame);
        ControlFlow::Continue(())
    }
    fn post_visit_query(&mut self, _: &Query) -> ControlFlow<()> {
        self.frames.pop();
        ControlFlow::Continue(())
    }
    fn pre_visit_select(&mut self, select: &Select) -> ControlFlow<()> {
        let mut locals = Vec::new();
        for from in &select.from {
            if let Some(b) = self.table(&from.relation) {
                locals.push(b);
            }
            for join in &from.joins {
                if let Some(b) = self.table(&join.relation) {
                    locals.push(b);
                }
                // USING/NATURAL joins alter column resolution; conservatively refuse refactoring.
                let constraint = match &join.join_operator {
                    JoinOperator::Join(c)
                    | JoinOperator::Inner(c)
                    | JoinOperator::Left(c)
                    | JoinOperator::LeftOuter(c)
                    | JoinOperator::Right(c)
                    | JoinOperator::RightOuter(c)
                    | JoinOperator::FullOuter(c)
                    | JoinOperator::CrossJoin(c) => Some(c),
                    _ => None,
                };
                if !matches!(
                    constraint,
                    Some(JoinConstraint::On(_) | JoinConstraint::None)
                ) {
                    self.diag(
                        DiagnosticKind::Unsupported,
                        select.span(),
                        "USING/NATURAL column coalescing and vendor-specific joins are not bound",
                    );
                }
            }
        }
        let mut names = BTreeSet::new();
        for b in &locals {
            if !names.insert(&b.name) {
                self.diag(
                    DiagnosticKind::Semantic,
                    select.span(),
                    "Duplicate relation alias in query scope",
                );
            }
        }
        if let Some(frame) = self.frames.last_mut() {
            frame.locals = locals;
        }
        for item in &select.projection {
            if let SelectItem::QualifiedWildcard(
                SelectItemQualifiedWildcardKind::ObjectName(name),
                _,
            ) = item
            {
                let ids = parts(name).unwrap_or_default();
                if ids.len() != 1 {
                    self.diag(
                        DiagnosticKind::Unsupported,
                        name.span(),
                        "Multi-part qualified wildcards are not bound",
                    );
                    continue;
                }
                let found: Vec<_> = self
                    .frames
                    .last()
                    .into_iter()
                    .flat_map(|f| &f.locals)
                    .filter(|b| matches_name(ids[0], &b.name, self.dialect))
                    .cloned()
                    .collect();
                if found.len() == 1 {
                    self.reference(found[0].symbol, ids[0]);
                } else {
                    self.diag(
                        DiagnosticKind::Semantic,
                        name.span(),
                        "Unresolved or ambiguous wildcard qualifier",
                    );
                }
            }
        }
        ControlFlow::Continue(())
    }
    fn pre_visit_expr(&mut self, expr: &Expr) -> ControlFlow<()> {
        match expr {
            Expr::Identifier(id) => self.bind_column(std::slice::from_ref(id)),
            Expr::CompoundIdentifier(ids) => self.bind_column(ids),
            _ => {}
        }
        ControlFlow::Continue(())
    }
    fn pre_visit_statement(&mut self, statement: &Statement) -> ControlFlow<()> {
        if !matches!(statement, Statement::Query(_)) {
            self.diag(DiagnosticKind::Unsupported, statement.span(), "Only SELECT/CTE statements have semantic binding; DDL, DML and dynamic SQL remain unverified");
        }
        ControlFlow::Continue(())
    }
}

pub fn analyze(sql: &str, dialect: Dialect, catalog: &Catalog) -> Analysis {
    let ast = match parse(sql, dialect) {
        Ok(ast) => ast,
        Err(e) => {
            return Analysis {
                symbols: vec![],
                diagnostics: vec![Diagnostic {
                    kind: DiagnosticKind::Syntax,
                    range: 0..sql.len(),
                    message: e.0,
                }],
            };
        }
    };
    let mut binder = Binder::new(sql, dialect, catalog);
    let _ = ast.visit(&mut binder);
    binder.analysis
}
pub fn diagnostics(sql: &str, dialect: Dialect, catalog: &Catalog) -> Vec<Diagnostic> {
    analyze(sql, dialect, catalog).diagnostics
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CompletionKind {
    Keyword,
    Schema,
    Table,
    Column,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Completion {
    pub label: String,
    pub insert_text: String,
    pub kind: CompletionKind,
    pub replace: Range<usize>,
}
fn quoted(name: &str, dialect: Dialect) -> String {
    let q = if dialect == Dialect::MySql { '`' } else { '"' };
    format!("{q}{}{q}", name.replace(q, &format!("{q}{q}")))
}
/// Completion binds a synthetic identifier in a separate buffer. The editor source is never mutated.
pub fn completions(
    sql: &str,
    cursor: usize,
    dialect: Dialect,
    catalog: &Catalog,
) -> Vec<Completion> {
    if !sql.is_char_boundary(cursor) {
        return vec![];
    }
    let (ts, error) = tokens(sql, dialect);
    // Do not suggest replacements inside comments or literals, including unfinished strings.
    for t in &ts {
        if let Some(r) = range(sql, t.span) {
            if r.contains(&cursor)
                && !matches!(
                    t.token,
                    Token::Word(_)
                        | Token::Whitespace(
                            Whitespace::Space | Whitespace::Tab | Whitespace::Newline
                        )
                )
            {
                return vec![];
            }
            if r.start < cursor
                && cursor == r.end
                && (comment(&t.token)
                    || matches!(
                        t.token,
                        Token::SingleQuotedString(_) | Token::DollarQuotedString(_)
                    ))
            {
                return vec![];
            }
        }
    }
    if error.is_some() && ts.last().and_then(|t| offset(sql, t.span.end)).unwrap_or(0) < cursor {
        return vec![];
    }
    let current = ts.iter().find(|t| {
        matches!(t.token, Token::Word(_))
            && range(sql, t.span).is_some_and(|r| r.start <= cursor && cursor <= r.end)
    });
    let replacement = current
        .and_then(|t| range(sql, t.span))
        .unwrap_or(cursor..cursor);
    let prefix = sql
        .get(replacement.start..cursor)
        .unwrap_or("")
        .trim_matches(['"', '`'])
        .to_lowercase();
    let before = &sql[..replacement.start];
    let significant: Vec<_> = ts
        .iter()
        .filter(|t| {
            significant(&t.token) && offset(sql, t.span.end).is_some_and(|p| p <= replacement.start)
        })
        .collect();
    let last = significant.last().map(|t| &t.token);
    let qualified = matches!(last, Some(Token::Period));
    let relation_context = matches!(last, Some(Token::Word(w)) if matches!(w.value.to_uppercase().as_str(), "FROM" | "JOIN" | "UPDATE" | "INTO"));
    let mut candidates: Vec<(String, CompletionKind)> = Vec::new();
    if !qualified || relation_context {
        for table in &catalog.tables {
            candidates.push((table.schema.clone(), CompletionKind::Schema));
            candidates.push((
                format!(
                    "{}.{}",
                    quoted(&table.schema, dialect),
                    quoted(&table.name, dialect)
                ),
                CompletionKind::Table,
            ));
        }
    }
    if qualified
        && let Some(Token::Word(schema)) = significant.iter().rev().nth(1).map(|t| &t.token)
    {
        for t in catalog.tables.iter().filter(|t| schema.value == t.schema) {
            candidates.push((t.name.clone(), CompletionKind::Table));
        }
    }
    let patched = format!("{before}__dbide_completion__{}", &sql[replacement.end..]);
    if let Ok(ast) = parse(&patched, dialect) {
        let mut binder = Binder::new(&patched, dialect, catalog);
        let _ = ast.visit(&mut binder);
        candidates.extend(
            binder
                .completion
                .into_iter()
                .map(|c| (c, CompletionKind::Column)),
        );
        candidates.extend(
            binder
                .completion_relations
                .into_iter()
                .map(|c| (c, CompletionKind::Table)),
        );
    }
    if !qualified && !relation_context {
        candidates.extend(
            [
                "SELECT", "FROM", "WHERE", "JOIN", "GROUP BY", "ORDER BY", "LIMIT", "WITH",
            ]
            .into_iter()
            .map(|s| (s.to_owned(), CompletionKind::Keyword)),
        );
    }
    let mut seen = BTreeSet::new();
    candidates
        .into_iter()
        .filter(|(name, _)| {
            let plain = name.replace(['"', '`'], "");
            (prefix.is_empty()
                || plain.to_lowercase().starts_with(&prefix)
                || plain.to_lowercase().contains(&format!(".{prefix}")))
                && seen.insert(name.clone())
        })
        .map(|(label, kind)| {
            let insert_text = if kind == CompletionKind::Keyword
                || label.contains("\".\"")
                || label.contains("`.`")
            {
                label.clone()
            } else {
                quoted(&label, dialect)
            };
            Completion {
                label,
                insert_text,
                kind,
                replace: replacement.clone(),
            }
        })
        .collect()
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TextEdit {
    pub range: Range<usize>,
    pub replacement: String,
}
#[derive(Clone, Debug)]
pub struct RenamePreview {
    pub revision: u64,
    pub source_hash: [u8; 32],
    pub edits: Vec<TextEdit>,
}
impl RenamePreview {
    /// Verify revision AND original bytes, validate every range, then atomically produce a new buffer.
    pub fn apply(&self, sql: &str, revision: u64) -> Result<String, SqlError> {
        let hash: [u8; 32] = Sha256::digest(sql.as_bytes()).into();
        if revision != self.revision || hash != self.source_hash {
            return Err(SqlError(
                "Stale rename preview; analyze the current document revision".into(),
            ));
        }
        let mut previous_end = 0;
        let mut edits = self.edits.clone();
        edits.sort_by_key(|e| e.range.start);
        for e in &edits {
            if e.range.start < previous_end
                || e.range.start >= e.range.end
                || sql.get(e.range.clone()).is_none()
            {
                return Err(SqlError("Invalid or overlapping edit range".into()));
            }
            previous_end = e.range.end;
        }
        let mut out = sql.to_owned();
        for e in edits.iter().rev() {
            out.replace_range(e.range.clone(), &e.replacement);
        }
        Ok(out)
    }
}
/// Document-local alias/CTE rename only. Catalog object renaming requires separate DDL.
pub fn rename(
    sql: &str,
    cursor: usize,
    new_name: &str,
    dialect: Dialect,
    catalog: &Catalog,
    revision: u64,
) -> Result<RenamePreview, SqlError> {
    if new_name.is_empty() || new_name.contains('\0') {
        return Err(SqlError(
            "An identifier must be nonempty and contain no NUL".into(),
        ));
    }
    let analysis = analyze(sql, dialect, catalog);
    if !analysis.diagnostics.is_empty() {
        return Err(SqlError(
            "Rename requires fully resolved supported SELECT/CTE syntax".into(),
        ));
    }
    let symbol = analysis
        .symbol_at(cursor)
        .ok_or_else(|| SqlError("No uniquely resolved symbol at cursor".into()))?;
    if !matches!(symbol.kind, SymbolKind::Alias | SymbolKind::Cte) {
        return Err(SqlError(
            "Only document-local aliases and CTE names can be renamed; catalog objects require DDL"
                .into(),
        ));
    }
    if analysis.symbols.iter().any(|s| {
        !std::ptr::eq(s, symbol)
            && matches!(
                s.kind,
                SymbolKind::Alias | SymbolKind::Cte | SymbolKind::Table
            )
            && s.name.eq_ignore_ascii_case(new_name)
    }) {
        return Err(SqlError(
            "New name collides with another relation or CTE".into(),
        ));
    }
    // Every same-spelled identifier token must have a demonstrated AST binding.
    // This fails closed for vendor identifier positions the visitor does not yet
    // understand (for example lock targets or named-window declarations).
    let (source_tokens, _) = tokens(sql, dialect);
    for token in source_tokens {
        if let Token::Word(word) = &token.token {
            let id = Ident {
                value: word.value.clone(),
                quote_style: word.quote_style,
                span: token.span,
            };
            if matches_name(&id, &symbol.name, dialect) {
                let token_range = range(sql, token.span);
                let known = analysis.symbols.iter().any(|s| {
                    s.declaration
                        .iter()
                        .chain(s.references.iter())
                        .any(|r| Some(r) == token_range.as_ref())
                });
                if !known {
                    return Err(SqlError(
                        "A same-spelled identifier has no proven binding; rename refused".into(),
                    ));
                }
            }
        }
    }
    let edits = symbol
        .declaration
        .iter()
        .chain(symbol.references.iter())
        .map(|r| TextEdit {
            range: r.clone(),
            replacement: quoted(new_name, dialect),
        })
        .collect();
    let preview = RenamePreview {
        revision,
        source_hash: Sha256::digest(sql.as_bytes()).into(),
        edits,
    };
    let output = preview.apply(sql, revision)?;
    if !analyze(&output, dialect, catalog).diagnostics.is_empty() {
        return Err(SqlError("Renamed buffer failed binding validation".into()));
    }
    Ok(preview)
}
