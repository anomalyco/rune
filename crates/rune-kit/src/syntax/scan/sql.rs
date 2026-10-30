//! SQL.

use super::*;

const SQL_KEYWORDS: &[&str] = &[
    "add", "all", "alter", "analyze", "and", "any", "as", "asc", "begin", "between", "by", "cascade", "case", "check",
    "column", "commit", "conflict", "constraint", "create", "cross", "database", "declare", "default", "delete", "desc",
    "distinct", "do", "drop", "else", "end", "except", "exists", "explain", "fetch", "filter", "first", "for", "foreign",
    "from", "full", "function", "grant", "group", "having", "if", "ilike", "in", "index", "inner", "insert", "intersect",
    "into", "is", "join", "key", "language", "lateral", "left", "like", "limit", "materialized", "next", "not", "nothing",
    "offset", "on", "only", "or", "order", "outer", "over", "partition", "primary", "procedure", "recursive",
    "references", "rename", "replace", "return", "returning", "returns", "revoke", "right", "rollback", "rows", "schema",
    "select", "set", "some", "table", "temp", "temporary", "then", "to", "transaction", "trigger", "truncate", "union",
    "unique", "update", "using", "vacuum", "values", "view", "when", "where", "window", "with",
];
const SQL_TYPES: &[&str] = &[
    "bigint", "bigserial", "blob", "bool", "boolean", "bytea", "char", "character", "date", "decimal", "double", "float",
    "int", "integer", "interval", "json", "jsonb", "money", "numeric", "precision", "real", "serial", "smallint", "text",
    "time", "timestamp", "timestamptz", "tinyint", "uuid", "varchar",
];
const SQL_CONSTANTS: &[&str] = &["true", "false", "null"];

pub(super) fn sql(s: &[u8], mut i: usize, end: usize, out: &mut Out) {
    use Syntax::*;
    while i < end && !out.full {
        let b = s[i];
        let start = i;
        if b.is_ascii_whitespace() {
            i += 1;
        } else if b == b'-' && at(s, i + 1, end) == b'-' {
            i = line_end(s, i, end);
            out.push(start, i, Comment);
        } else if b == b'/' && at(s, i + 1, end) == b'*' {
            i = block_comment_end(s, i + 2, end, false);
            out.push(start, i, Comment);
        } else if b == b'\'' {
            // '' inside a string is a quote.
            i += 1;
            while i < end {
                if s[i] == b'\'' {
                    if at(s, i + 1, end) == b'\'' {
                        i += 2;
                        continue;
                    }
                    i += 1;
                    break;
                }
                i += 1;
            }
            out.push(start, i.min(end), String);
        } else if b == b'"' || b == b'`' {
            i = string_end(s, i, end, b, true);
            out.push(start, i, Property);
        } else if b.is_ascii_digit() {
            i = number_end(s, i, end);
            out.push(start, i, Number);
        } else if matches!(b, b'$' | b':' | b'@') && (is_ident(at(s, i + 1, end))) {
            i = ident_end(s, i + 1, end, b"");
            out.push(start, i, Property);
        } else if b == b'?' {
            i += 1;
            out.push(start, i, Property);
        } else if is_ident_start(b) {
            i = ident_end(s, i, end, b"");
            let w = &s[start..i];
            let mut k = i;
            while k < end && matches!(s[k], b' ' | b'\t') {
                k += 1;
            }
            if is_word_ci(w, SQL_CONSTANTS) {
                out.push(start, i, Number);
            } else if is_word_ci(w, SQL_KEYWORDS) {
                out.push(start, i, Keyword);
            } else if is_word_ci(w, SQL_TYPES) {
                out.push(start, i, Type);
            } else if at(s, k, end) == b'(' {
                out.push(start, i, Function);
            }
        } else {
            i += 1;
            out.push(start, i, Punctuation);
        }
    }
}
