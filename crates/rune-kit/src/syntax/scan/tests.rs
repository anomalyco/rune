use super::*;
use Syntax::*;

/// Each coloured range as (text, kind).
fn toks(lang: Lang, src: &str) -> Vec<(std::string::String, Syntax)> {
    highlight(lang, src).into_iter().map(|(a, b, t)| (src[a as usize..b as usize].to_owned(), t)).collect()
}

fn kind_of(lang: Lang, src: &str, text: &str) -> Option<Syntax> {
    toks(lang, src).into_iter().find(|(t, _)| t == text).map(|(_, k)| k)
}

fn has(lang: Lang, src: &str, want: &[(&str, Syntax)]) {
    let got = toks(lang, src);
    for (text, kind) in want {
        assert!(got.iter().any(|(t, k)| t == text && k == kind), "{lang:?}: no {text:?} as {kind:?} in {got:?}");
    }
}

#[test]
fn fence_names_map_to_languages() {
    for (name, lang) in [("TypeScript", Lang::Ts), ("tsx", Lang::Tsx), ("jsx", Lang::Js), ("bash", Lang::Shell), ("yml", Lang::Yaml), ("rs", Lang::Rust),
        ("py", Lang::Python), ("Python3", Lang::Python), ("golang", Lang::Go), ("patch", Lang::Diff)]
    {
        assert_eq!(Lang::from_name(name), Some(lang));
    }
    assert_eq!(Lang::from_name("brainfuck"), None);
}

#[test]
fn typescript() {
    let src = "// note\nexport async function load(id: string): Promise<User> {\n  const n = 0x1f + 2.5e-3;\n  return fetch(`/u/${id}`).then(r => r.json()) /* done */;\n}\nconst re = /a[/]b/g, half = n / 2;\nnode.type = null;";
    has(Lang::Ts, src, &[
        ("// note", Comment), ("export", Keyword), ("function", Keyword), ("load", Function), ("string", Type),
        ("Promise", Type), ("User", Type), ("0x1f", Number), ("2.5e-3", Number), ("`/u/", String), ("${", Punctuation),
        ("fetch", Function), ("json", Function), ("/* done */", Comment), ("/a[/]b/g", String),
        ("null", Number),
    ]);
    // `id` inside the template is a plain name; `n / 2` divides; `.type` is a property.
    assert_eq!(toks(Lang::Ts, "`${id}`").iter().filter(|(t, _)| t == "id").count(), 0);
    assert_eq!(kind_of(Lang::Ts, src, "/ 2"), None);
    assert_eq!(kind_of(Lang::Ts, src, "type"), None);
}

#[test]
fn jsx_text_children_stay_plain() {
    let src = "return <Card title=\"Hi\" on={() => go(1)}>it's {n} items<br/></Card>;";
    has(Lang::Tsx, src, &[
        ("Card", Type), ("title", Property), ("\"Hi\"", String), ("go", Function), ("1", Number), ("br", Keyword),
    ]);
    assert!(!toks(Lang::Tsx, src).iter().any(|(t, _)| t.contains("it's")));
    // Plain TypeScript compares instead.
    assert_eq!(kind_of(Lang::Ts, "a <b", "b"), None);
}

fn count(lang: Lang, src: &str, text: &str, kind: Syntax) -> usize {
    toks(lang, src).iter().filter(|(t, k)| t == text && *k == kind).count()
}

#[test]
fn type_parameters_are_not_jsx_tags() {
    let after = "\nexport function after(n: number) { return n + 1; }";
    for head in ["const id = <T,>(x: T) => x;", "type Fn = <T>(x: T) => T;", "const f = <T extends object>(x: T) => x;"] {
        let src = format!("{head}{after}");
        has(Lang::Tsx, &src, &[("export", Keyword), ("after", Function), ("number", Type), ("1", Number)]);
    }
    // An element whose text starts with a bracket is still an element.
    let src = "const b = <b>(note)</b>;";
    assert_eq!(count(Lang::Tsx, src, "b", Keyword), 2);
    assert_eq!(kind_of(Lang::Tsx, src, "note"), None);
}

#[test]
fn a_less_than_in_jsx_text_is_text() {
    let src = "const view = <div>a < b</div>;\nexport const after = 1;";
    has(Lang::Tsx, src, &[("export", Keyword), ("1", Number)]);
    assert_eq!(count(Lang::Tsx, src, "div", Keyword), 2);
    assert_eq!(kind_of(Lang::Tsx, src, "b"), None);
}

#[test]
fn an_element_that_never_closes_gives_the_code_after_it_back() {
    for head in ["const el = <div>\n  unclosed text", "const el = <div className=\"a\"", "const el = <div <span>"] {
        let src = format!("{head}\nexport const after = 1;");
        has(Lang::Tsx, &src, &[("export", Keyword), ("1", Number)]);
    }
    // Elements over several lines still take their children.
    let src = "return (\n  <ul>\n    <li>one</li>\n    {items.map(i => <li>{i}</li>)}\n  </ul>\n);";
    has(Lang::Tsx, src, &[("map", Function)]);
    assert_eq!(count(Lang::Tsx, src, "ul", Keyword), 2);
    assert_eq!(kind_of(Lang::Tsx, src, "one"), None);
}

#[test]
fn comments_inside_embedded_code_hide_their_brackets_and_quotes() {
    let after = "\nconst after = 1;";
    for (lang, head, comment) in [
        (Lang::Tsx, "<Card onClick={() => { /* don't */ run(); }} />", "/* don't */"),
        (Lang::Tsx, "<Card onClick={() => {\n  // {\n  run();\n}} />", "// {"),
        (Lang::Ts, "const s = `${a /* } */}`;", "/* } */"),
    ] {
        let src = format!("{head}{after}");
        has(lang, &src, &[(comment, Comment), ("const", Keyword), ("1", Number)]);
    }
    let src = "echo $( # )\n cmd )\necho after";
    has(Lang::Shell, src, &[("# )", Comment), ("cmd", Function)]);
    assert_eq!(count(Lang::Shell, src, "echo", Function), 2);
    // `$#` is a variable, not a comment.
    let src = "n=$(echo $#); echo after";
    has(Lang::Shell, src, &[("$#", Property)]);
    assert_eq!(count(Lang::Shell, src, "echo", Function), 2);
}

#[test]
fn an_apostrophe_in_nested_jsx_text_is_not_a_string() {
    let after = "\nexport const after = 1;";
    for (src, tag) in [
        ("return <div>{ok && <p>don't</p>}</div>;", "div"),
        ("const ul = <ul>{list.map(x => <li>it's {x}</li>)}</ul>;", "ul"),
    ] {
        let src = format!("{src}{after}");
        has(Lang::Tsx, &src, &[("export", Keyword), ("1", Number)]);
        assert_eq!(count(Lang::Tsx, &src, tag, Keyword), 2, "{src}");
    }
    has(Lang::Tsx, &format!("const ul = <ul>{{list.map(x => <li>it's {{x}}</li>)}}</ul>;{after}"), &[("map", Function)]);
    // Quotes in code and in attributes are still strings.
    let src = format!("<i>{{a < b ? 'x}}' : <a title=\"}}'\">y</a>}}</i>;{after}");
    has(Lang::Tsx, &src, &[("'x}'", String), ("\"}'\"", String), ("export", Keyword)]);
    assert_eq!(count(Lang::Tsx, &src, "i", Keyword), 2);
}

#[test]
fn a_line_continuation_keeps_the_next_word_a_command() {
    assert_eq!(count(Lang::Shell, "cargo build && \\\n  cargo test", "cargo", Function), 2);
    assert_eq!(count(Lang::Shell, "cargo build && \\\r\n  cargo test", "cargo", Function), 2);
    assert_eq!(kind_of(Lang::Shell, "cargo build \\\n  --release", "--release"), None);
}

#[test]
fn python() {
    let src = "def f(a, *, b=None):\n    \"\"\"Doc with 'quotes'\n    and lines.\"\"\"\n    return {k: v for k, v in a.items() if v is not None}\n\nclass Config(BaseModel):\n    PORT = 8080\n    path: list[str] = []\n\n@app.route(\"/x\")\nasync def view(req):\n    await req.send(rb'\\x00', 0o17, 1_000.5j)";
    has(Lang::Python, src, &[
        ("def", Keyword), ("f", Function), ("None", Number), ("\"\"\"Doc with 'quotes'\n    and lines.\"\"\"", String),
        ("for", Keyword), ("in", Keyword), ("items", Function), ("is", Keyword), ("not", Keyword), ("class", Keyword),
        ("Config", Type), ("BaseModel", Type), ("PORT", Number), ("8080", Number), ("list", Type), ("str", Type),
        ("@app.route", Keyword), ("\"/x\"", String), ("async", Keyword), ("view", Function), ("await", Keyword),
        ("send", Function), ("rb'\\x00'", String), ("0o17", Number), ("1_000.5j", Number),
    ]);
    // `v` and `a` are plain names; `items` without a call after a dot would be too.
    assert_eq!(kind_of(Lang::Python, src, "v"), None);
    assert_eq!(kind_of(Lang::Python, "x = a.items", "items"), None);
}

#[test]
fn python_f_strings_colour_their_fields() {
    let src = "f\"{d['k']:>10} {{literal}} {n + 1}\" + F'{x}' + \"{plain}\"";
    has(Lang::Python, src, &[
        ("f\"", String), ("'k'", String), (" {{literal}} ", String), ("1", Number), ("F'", String), ("\"{plain}\"", String),
    ]);
    assert_eq!(count(Lang::Python, src, "{", Punctuation), 3, "three fields, and {{ is a brace");
    // A field that never closes leaves the rest of the string a string.
    has(Lang::Python, "f'{oops' + 1", &[("f'{oops'", String), ("1", Number)]);
}

#[test]
fn python_soft_keywords_only_start_statements() {
    let src = "match command.split():\n    case [\"go\", where]:\n        pass\n    case _:  # anything\n        pass\nmatch = re.match(p, s)\ncase(1)";
    assert_eq!(count(Lang::Python, src, "match", Keyword), 1);
    assert_eq!(count(Lang::Python, src, "case", Keyword), 2);
    has(Lang::Python, src, &[("match", Function), ("case", Function)]);
}

#[test]
fn go() {
    let src = "type Server struct {\n\tAddr string\n\tdone chan error\n}\n\nfunc (s *Server) Run(ctx context.Context) error {\n\tpath := `C:\\dir\\`\n\tgo s.loop()\n\treturn NewError(\"x\", iota, 0x1F)\n}";
    has(Lang::Go, src, &[
        ("type", Keyword), ("Server", Type), ("struct", Keyword), ("string", Type), ("chan", Keyword), ("error", Type),
        ("func", Keyword), ("Run", Function), ("`C:\\dir\\`", String), ("go", Keyword), ("loop", Function),
        ("return", Keyword), ("NewError", Function), ("iota", Number), ("0x1F", Number),
    ]);
    // A raw string's backslash escapes nothing, so the string ends at the next backtick.
    assert_eq!(kind_of(Lang::Go, src, "go"), Some(Keyword));
    // Exported fields and composite-literal keys aren't types; `pkg.Name` stays plain.
    assert_eq!(kind_of(Lang::Go, src, "Addr"), None);
    assert_eq!(kind_of(Lang::Go, "s := Server{Addr: \":80\"}", "Addr"), None);
    assert_eq!(kind_of(Lang::Go, "s := Server{Addr: \":80\"}", "Server"), Some(Type));
    assert_eq!(kind_of(Lang::Go, src, "Context"), None);
}

#[test]
fn diff_lines() {
    let src = "--- a/f\n+++ b/f\r\n@@ -1 +1 @@ fn x\n-a\r\n+b\n same\n+\n";
    let want = [("--- a/f", Comment), ("+++ b/f", Comment), ("@@ -1 +1 @@ fn x", Comment), ("-a", Deleted), ("+b", Inserted), ("+", Inserted)];
    assert_eq!(toks(Lang::Diff, src), want.map(|(t, k)| (t.to_owned(), k)));
}

#[test]
fn json_keys_are_properties() {
    has(Lang::Json, "{\"name\": \"rune\", \"n\": -1.5, \"ok\": true, \"x\": null}", &[
        ("\"name\"", Property), ("\"rune\"", String), ("1.5", Number), ("true", Number), ("null", Number), ("{", Punctuation),
    ]);
}

#[test]
fn rust() {
    let src = "#[derive(Debug)]\npub fn get<'a>(s: &'a str) -> Option<u8> {\n    let c = 'x';\n    println!(\"{}\", r#\"raw \"q\"\"#);\n    /* outer /* inner */ still */ Some(b'\\n')\n}";
    has(Lang::Rust, src, &[
        ("pub", Keyword), ("fn", Keyword), ("get", Function), ("'a", Keyword), ("str", Type), ("Option", Type),
        ("u8", Type), ("'x'", String), ("println!", Function), ("r#\"raw \"q\"\"#", String),
        ("/* outer /* inner */ still */", Comment), ("Some", Type), ("b'\\n'", String),
    ]);
}

#[test]
fn swift() {
    let src = "@MainActor\nstruct Row: View {\n  let title: String = \"\"\"\n  hi\n  \"\"\"\n  func body() -> some View { Text(title).padding(8) }\n}\n#if DEBUG\nlet x = nil";
    has(Lang::Swift, src, &[
        ("@MainActor", Keyword), ("struct", Keyword), ("Row", Type), ("View", Type), ("\"\"\"\n  hi\n  \"\"\"", String),
        ("func", Keyword), ("body", Function), ("some", Keyword), ("padding", Function), ("8", Number), ("#if", Keyword),
        ("nil", Number),
    ]);
}

#[test]
fn shell() {
    let src = "#!/bin/bash\nFOO=1 npm run build --if-present # build\nif [ -n \"$HOME/${X}\" ]; then\n  echo $(git rev-parse HEAD) | tee out.txt\nfi\ncat <<EOF\n$not a var\nEOF\nls";
    has(Lang::Shell, src, &[
        ("#!/bin/bash", Comment), ("FOO", Property), ("npm", Function), ("# build", Comment), ("if", Keyword),
        ("[", Punctuation), ("\"", String), ("$HOME", Property), ("${X}", Property), ("then", Keyword), ("echo", Function),
        ("$(", Punctuation), ("git", Function), ("tee", Function), ("fi", Keyword), ("cat", Function),
        ("EOF", String), ("ls", Function),
    ]);
    assert_eq!(kind_of(Lang::Shell, src, "run"), None);
    assert!(toks(Lang::Shell, src).iter().any(|(t, k)| t.contains("$not a var") && *k == String));
}

#[test]
fn html_with_script_and_style() {
    let src = "<!-- c -->\n<a href=\"/x\" data-id=7>Tom &amp; Jerry</a>\n<script>const a = 1;</script>\n<style>p { color: red; }</style>";
    has(Lang::Html, src, &[
        ("<!-- c -->", Comment), ("a", Keyword), ("href", Property), ("\"/x\"", String), ("7", String), ("&amp;", Number),
        ("const", Keyword), ("1", Number), ("color", Property),
    ]);
    assert_eq!(kind_of(Lang::Html, src, "Tom"), None);
}

#[test]
fn css() {
    let src = "@media (min-width: 600px) {\n  .card:hover > a, #main { color: #fff !important; margin: -4px 0 calc(1em + 2%); --gap: 4px; }\n}";
    has(Lang::Css, src, &[
        ("@media", Keyword), ("600px", Number), (".card", Type), (":hover", Keyword), ("#main", Type), ("color", Property),
        ("#fff", Number), ("!important", Keyword), ("-4px", Number), ("calc", Function), ("1em", Number), ("2%", Number),
        ("--gap", Property),
    ]);
}

#[test]
fn sql() {
    let src = "-- users\nSELECT id, count(*) AS n FROM \"users\" WHERE name = 'O''Brien' AND age > $1 LIMIT 10;\nCREATE TABLE t (id uuid PRIMARY KEY, ok boolean DEFAULT false);";
    has(Lang::Sql, src, &[
        ("-- users", Comment), ("SELECT", Keyword), ("count", Function), ("AS", Keyword), ("\"users\"", Property),
        ("'O''Brien'", String), ("$1", Property), ("10", Number), ("uuid", Type), ("boolean", Type), ("false", Number),
    ]);
}

#[test]
fn yaml() {
    let src = "---\nname: rune # the name\nversion: 1.2\nanchors: &base\n  - item\n  - key: \"v\"\n    on: true\nscript: |\n  echo hi\n  # not a comment\nflow: [a, 1, {b: c}]\nafter: x";
    has(Lang::Yaml, src, &[
        ("---", Punctuation), ("name", Property), ("rune", String), ("# the name", Comment), ("1.2", Number),
        ("&base", Type), ("-", Punctuation), ("item", String), ("key", Property), ("\"v\"", String), ("on", Property),
        ("true", Number), ("echo hi", String), ("# not a comment", String), ("1", Number), ("b", Property),
        ("c", String), ("after", Property),
    ]);
}

#[test]
fn ranges_are_ordered_and_on_char_boundaries() {
    let delim = format!("{}é", "a".repeat(63));
    let heredoc = format!("cat << {delim}\nhi\n{delim}\nls");
    let body = format!("hi\n{delim}");
    has(Lang::Shell, &heredoc, &[("cat", Function), (&body, String), ("ls", Function)]);
    let cap_utf8 = format!("let x = 1;\n{}", "🎉".repeat(MAX_SOURCE / 4 + 4));
    let pieces: &[&str] = &[
        "\"", "'", "`", "\"\"\"", "'''", "r#\"", "\"#", "b\"", "b'", "f\"", "f'{", "${", "$(", "/*", "*/", "//",
        "<!--", "-->", "<![CDATA[", "]]>", "<script>", "</script>", "<style>", "<<EOF\n", "<<-\t", "\\", "\\uD800",
        "\\u{D800}", "\\x00", "\r\n", "\r", "\n", "\t", " ", "é", "🎉", "日本語", "a", "1_000.5e-2", ":", "|", ">",
        "- ", "@", "#", "$", "&amp;", "=", "(", ")", "[", "]", "{", "}", "/", "+",
    ];
    let mut fuzz = Vec::with_capacity(64);
    let mut seed = 0x9e37_79b9_7f4a_7c15u64;
    for len in 1..=64 {
        let mut s = std::string::String::new();
        for _ in 0..len {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            s.push_str(pieces[(seed >> 32) as usize % pieces.len()]);
        }
        fuzz.push(s);
    }
    let fixed = [
        "const café = \"naïve 🎉\"; // ✓\n<p>é</p> `${'ü'}`",
        "echo \\🎉 123 \\é",
        heredoc.as_str(),
        cap_utf8.as_str(),
    ];
    let langs = [
        Lang::Js, Lang::Ts, Lang::Tsx, Lang::Rust, Lang::Swift, Lang::Go, Lang::Python, Lang::Shell, Lang::Html,
        Lang::Css, Lang::Sql, Lang::Yaml, Lang::Json, Lang::Diff,
    ];
    for src in fixed.into_iter().chain(fuzz.iter().map(std::string::String::as_str)) {
        let end = coloured_len(src);
        assert!(src.is_char_boundary(end));
        for lang in langs {
            let mut raw = Out::default();
            scan(lang, src.as_bytes(), 0, end, &mut raw, 0);
            assert!(raw.tokens.iter().all(|&(a, b, _)| src.is_char_boundary(a as usize) && src.is_char_boundary(b as usize)), "{lang:?}: {src:?}");
            let t = highlight(lang, src);
            assert!(t.windows(2).all(|w| w[0].1 <= w[1].0), "{lang:?}");
            assert!(t.iter().all(|&(a, b, _)| a < b && (b as usize) <= end && src.is_char_boundary(a as usize) && src.is_char_boundary(b as usize)));
        }
    }
}

#[test]
fn hostile_input_finishes_quickly_and_stays_bounded() {
    let n = MAX_SOURCE - 64;
    let inputs = [
        "\"".repeat(n),
        "/*".repeat(n / 2),
        "`${".repeat(n / 3),
        "{".repeat(n),
        "<a b={".repeat(n / 6),
        "(/".repeat(n / 2),
        format!("r{}\"", "#".repeat(n)),
        "$($(".repeat(n / 4),
        "'".repeat(n),
        "a".repeat(n),
        "<script>".repeat(n / 8),
        "- ".repeat(n / 2),
        "x: |\n".repeat(n / 5),
        "@media{".repeat(n / 7),
        "f'{".repeat(n / 3),
        "f\"{'".repeat(n / 4),
        "\"\"\"".repeat(n / 3),
        "`".repeat(n),
        "match x\n".repeat(n / 8),
    ];
    let t0 = std::time::Instant::now();
    for src in &inputs {
        for lang in [Lang::Js, Lang::Tsx, Lang::Rust, Lang::Swift, Lang::Go, Lang::Python, Lang::Shell, Lang::Html, Lang::Css, Lang::Sql, Lang::Yaml, Lang::Json, Lang::Diff] {
            let t = highlight(lang, src);
            assert!(t.len() <= MAX_TOKENS);
        }
    }
    // 247 scans of ~32 KiB each; linear scanning takes milliseconds even in a debug build.
    assert!(t0.elapsed().as_secs_f32() < 5.0, "{:?}", t0.elapsed());
}

#[test]
fn colour_stops_at_the_caps() {
    let line = "// one comment per line, each its own range\n";
    let big = line.repeat(MAX_SOURCE / line.len() + 10);
    let last = highlight(Lang::Rust, &big).last().unwrap().1 as usize;
    assert!(last <= MAX_SOURCE && last > MAX_SOURCE - line.len(), "{last}");
    let many = "1 ".repeat(MAX_TOKENS + 10);
    assert_eq!(highlight(Lang::Js, &many).len(), MAX_TOKENS);
}

/// One snippet per language and every range the scanner gives it, in order.
#[rustfmt::skip]
const GOLDENS: &[(Lang, &str, &[(&str, Syntax)])] = &[
    (Lang::Rust, "fn main() {\n    let v: Vec<u8> = vec![1, 2]; // two\n    println!(\"{}\", v.len());\n}", &[
        ("fn", Keyword), ("main", Function), ("()", Punctuation), ("{", Punctuation), ("let", Keyword), (":", Punctuation),
        ("Vec", Type), ("<", Punctuation), ("u8", Type), (">", Punctuation), ("=", Punctuation), ("vec!", Function), ("[", Punctuation),
        ("1", Number), (",", Punctuation), ("2", Number), ("];", Punctuation), ("// two", Comment), ("println!", Function),
        ("(", Punctuation), ("\"{}\"", String), (",", Punctuation), (".", Punctuation), ("len", Function), ("());", Punctuation),
        ("}", Punctuation),
    ]),
    (Lang::Js, "const xs = [1, 2].map((x) => x * 2); // double\nconsole.log(`n=${xs.length}`, /a+/g);", &[
        ("const", Keyword), ("=", Punctuation), ("[", Punctuation), ("1", Number), (",", Punctuation), ("2", Number),
        ("].", Punctuation), ("map", Function), ("((", Punctuation), (")", Punctuation), ("=>", Punctuation), ("*", Punctuation),
        ("2", Number), (");", Punctuation), ("// double", Comment), (".", Punctuation), ("log", Function), ("(", Punctuation),
        ("`n=", String), ("${", Punctuation), (".", Punctuation), ("}", Punctuation), ("`", String), (",", Punctuation),
        ("/a+/g", String), (");", Punctuation),
    ]),
    (Lang::Ts, "interface User { id: number; name?: string }\nexport function greet(u: User): string {\n  return `hi ${u.name ?? \"anon\"}`;\n}", &[
        ("interface", Keyword), ("User", Type), ("{", Punctuation), (":", Punctuation), ("number", Type), (";", Punctuation),
        ("?:", Punctuation), ("string", Type), ("}", Punctuation), ("export", Keyword), ("function", Keyword), ("greet", Function),
        ("(", Punctuation), (":", Punctuation), ("User", Type), ("):", Punctuation), ("string", Type), ("{", Punctuation),
        ("return", Keyword), ("`hi ", String), ("${", Punctuation), (".", Punctuation), ("??", Punctuation), ("\"anon\"", String),
        ("}", Punctuation), ("`", String), (";", Punctuation), ("}", Punctuation),
    ]),
    (Lang::Python, "@dataclass\nclass Point:\n    x: int = 0  # origin\n    def dist(self, other: \"Point\") -> float:\n        return math.hypot(self.x - other.x, f\"{self.y!r}\")\nprint(None, True, 1e3, r'\\d+', b\"x\")", &[
        ("@dataclass", Keyword), ("class", Keyword), ("Point", Type), (":", Punctuation), (":", Punctuation), ("int", Type),
        ("=", Punctuation), ("0", Number), ("# origin", Comment), ("def", Keyword), ("dist", Function), ("(", Punctuation),
        ("self", Keyword), (",", Punctuation), (":", Punctuation), ("\"Point\"", String), (")", Punctuation), ("->", Punctuation),
        ("float", Type), (":", Punctuation), ("return", Keyword), (".", Punctuation), ("hypot", Function), ("(", Punctuation),
        ("self", Keyword), (".", Punctuation), ("-", Punctuation), (".", Punctuation), (",", Punctuation), ("f\"", String),
        ("{", Punctuation), ("self", Keyword), (".", Punctuation), ("!", Punctuation), ("}", Punctuation), ("\"", String),
        (")", Punctuation), ("print", Function), ("(", Punctuation), ("None", Number), (",", Punctuation), ("True", Number),
        (",", Punctuation), ("1e3", Number), (",", Punctuation), ("r'\\d+'", String), (",", Punctuation), ("b\"x\"", String),
        (")", Punctuation),
    ]),
    (Lang::Go, "package main\n\nimport \"fmt\"\n\nfunc main() {\n\tfor i := 0; i < 3; i++ {\n\t\tfmt.Println(`raw\\n`, 'x', nil) // done\n\t}\n}", &[
        ("package", Keyword), ("import", Keyword), ("\"fmt\"", String), ("func", Keyword), ("main", Function), ("()", Punctuation),
        ("{", Punctuation), ("for", Keyword), (":=", Punctuation), ("0", Number), (";", Punctuation), ("<", Punctuation), ("3", Number),
        (";", Punctuation), ("++", Punctuation), ("{", Punctuation), (".", Punctuation), ("Println", Function), ("(", Punctuation),
        ("`raw\\n`", String), (",", Punctuation), ("'x'", String), (",", Punctuation), ("nil", Number), (")", Punctuation),
        ("// done", Comment), ("}", Punctuation), ("}", Punctuation),
    ]),
    (Lang::Swift, "struct Point { var x: Double = 0 }\nlet p = Point(x: 1.5) // origin\nprint(\"\\(p.x)\")", &[
        ("struct", Keyword), ("Point", Type), ("{", Punctuation), ("var", Keyword), (":", Punctuation), ("Double", Type),
        ("=", Punctuation), ("0", Number), ("}", Punctuation), ("let", Keyword), ("=", Punctuation), ("Point", Type),
        ("(", Punctuation), (":", Punctuation), ("1.5", Number), (")", Punctuation), ("// origin", Comment), ("print", Function),
        ("(", Punctuation), ("\"\\(p.x)\"", String), (")", Punctuation),
    ]),
    (Lang::Json, "{\"a\": [1, true, null], \"b\": \"x\\\"y\"}", &[
        ("{", Punctuation), ("\"a\"", Property), (":", Punctuation), ("[", Punctuation), ("1", Number), (",", Punctuation),
        ("true", Number), (",", Punctuation), ("null", Number), ("],", Punctuation), ("\"b\"", Property), (":", Punctuation),
        ("\"x\\\"y\"", String), ("}", Punctuation),
    ]),
    (Lang::Shell, "export PATH=\"$HOME/bin:$PATH\"\nfor f in *.txt; do wc -l \"$f\"; done # count", &[
        ("export", Keyword), ("\"", String), ("$HOME", Property), ("/bin:", String), ("$PATH", Property), ("\"", String),
        ("for", Keyword), ("in", Keyword), (";", Punctuation), ("do", Keyword), ("wc", Function), ("\"", String), ("$f", Property),
        ("\"", String), (";", Punctuation), ("done", Keyword), ("# count", Comment),
    ]),
    (Lang::Sql, "SELECT name, COUNT(*) FROM users WHERE id = 1 -- one\nGROUP BY name;", &[
        ("SELECT", Keyword), (",", Punctuation), ("COUNT", Function), ("(*)", Punctuation), ("FROM", Keyword), ("WHERE", Keyword),
        ("=", Punctuation), ("1", Number), ("-- one", Comment), ("GROUP", Keyword), ("BY", Keyword), (";", Punctuation),
    ]),
    (Lang::Html, "<div class=\"a\"><!-- hi --><b>x</b></div>", &[
        ("<", Punctuation), ("div", Keyword), ("class", Property), ("=", Punctuation), ("\"a\"", String), (">", Punctuation),
        ("<!-- hi -->", Comment), ("<", Punctuation), ("b", Keyword), (">", Punctuation), ("</", Punctuation), ("b", Keyword),
        ("></", Punctuation), ("div", Keyword), (">", Punctuation),
    ]),
    (Lang::Css, ".btn:hover { color: #fff; padding: 4px 8px; } /* hover */", &[
        (".btn", Type), (":hover", Keyword), ("{", Punctuation), ("color", Property), (":", Punctuation), ("#fff", Number),
        (";", Punctuation), ("padding", Property), (":", Punctuation), ("4px", Number), ("8px", Number), (";", Punctuation),
        ("}", Punctuation), ("/* hover */", Comment),
    ]),
    (Lang::Yaml, "name: ci\non: [push]\nenv:\n  DEBUG: true # verbose\n  N: 3", &[
        ("name", Property), (":", Punctuation), ("ci", String), ("on", Property), (":", Punctuation), ("[", Punctuation),
        ("push", String), ("]", Punctuation), ("env", Property), (":", Punctuation), ("DEBUG", Property), (":", Punctuation),
        ("true", Number), ("# verbose", Comment), ("N", Property), (":", Punctuation), ("3", Number),
    ]),
    (Lang::Diff, "diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -1,2 +1,2 @@\n context\n-old\n+new\n\\ No newline at end of file", &[
        ("diff --git a/x b/x", Comment), ("--- a/x", Comment), ("+++ b/x", Comment), ("@@ -1,2 +1,2 @@", Comment), ("-old", Deleted),
        ("+new", Inserted), ("\\ No newline at end of file", Comment),
    ]),
];

#[test]
fn goldens() {
    for (lang, src, want) in GOLDENS {
        let want: Vec<(std::string::String, Syntax)> = want.iter().map(|&(t, k)| (t.to_owned(), k)).collect();
        assert_eq!(toks(*lang, src), want, "{lang:?}");
    }
}
