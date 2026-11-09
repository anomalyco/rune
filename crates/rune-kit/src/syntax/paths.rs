//! The language Desktop's diffs and file views colour a file as, from its name.

/// The Highlights language name for the file at `path` (`/` or `\` separated), as Desktop picks
/// it: the whole file name (`Makefile`, `CMakeLists.txt`), else a two-part extension
/// (`component.ts`), else the extension, matched case-sensitively. `None` when Desktop draws
/// the file plain: an unknown name, or a language Highlights has no lexer for (`.m` is Wolfram
/// there, `.pl` Prolog).
pub fn lang_for_path(path: &str) -> Option<&'static str> {
    let name = path.rsplit(['/', '\\']).next()?;
    let found = lookup(name).or_else(|| {
        // The extension after the name's first dot, when it has two parts (`a.component.ts`).
        let (_, rest) = name.split_once('.')?;
        rest.contains('.').then(|| lookup(rest)).flatten()
    });
    let found = found.or_else(|| lookup(name.rsplit_once('.')?.1))?;
    Some(found).filter(|l| !l.is_empty())
}

fn lookup(key: &str) -> Option<&'static str> {
    PATHS.binary_search_by(|(k, _)| (*k).cmp(key)).ok().map(|i| PATHS[i].1)
}

/// File names and extensions Desktop's diffs and file views name a language by (Pierre's
/// `getFiletypeFromFileName`, `@pierre/diffs` 1.6.1), sorted; an empty name where Highlights has no lexer, so the file draws plain.
const PATHS: [(&str, &str); 357] = [
    ("1c", ""), ("Appraisals", "ruby"), ("BUCK", "python"), ("BUILD", "python"), ("Berksfile", "ruby"),
    ("Brewfile", "ruby"), ("CMakeLists.txt", "cmake"), ("CODEOWNERS", ""), ("COMMIT_EDITMSG", "git-commit"), ("Capfile", "ruby"),
    ("Dangerfile", "ruby"), ("Dockerfile", "dockerfile"), ("Gemfile", "ruby"), ("Guardfile", "ruby"), ("Makefile", "makefile"),
    ("P", ""), ("PACKAGE", "python"), ("R", "r"), ("Rakefile", "ruby"), ("Thorfile", "ruby"),
    ("Tiltfile", "python"), ("Vagrantfile", "ruby"), ("WORKSPACE", "python"), ("abap", ""), ("ada", ""),
    ("adb", ""), ("ado", ""), ("adoc", ""), ("ads", ""), ("apl", ""),
    ("applescript", ""), ("ara", ""), ("arb", "ruby"), ("as", ""), ("asciidoc", ""),
    ("asm", "asm"), ("astro", "astro"), ("awk", ""), ("bal", ""), ("bas", ""),
    ("bash", "zsh"), ("bat", "cmd"), ("bazel", "python"), ("be", ""), ("beancount", ""),
    ("bib", ""), ("bicep", ""), ("blade.php", ""), ("bsl", ""), ("builder", "ruby"),
    ("bxl", "python"), ("bzl", "python"), ("bzlmod", "python"), ("c", "c"), ("cairo", ""),
    ("cbl", ""), ("cc", "cpp"), ("cdc", ""), ("cfg", ""), ("cjs", "javascript"),
    ("cl", "lisp"), ("clar", ""), ("clj", "clojure"), ("cljc", "clojure"), ("cljs", "clojure"),
    ("cls", "tex"), ("cmake", "cmake"), ("cmd", "cmd"), ("cob", ""), ("cobol", ""),
    ("coffee", ""), ("component.html", "angular-html"), ("component.ts", "angular-ts"), ("conf", ""), ("config", ""),
    ("cpp", "cpp"), ("cql", ""), ("cr", ""), ("cs", "csharp"), ("cshtml", ""),
    ("css", "css"), ("csv", ""), ("cts", "typescript"), ("cue", ""), ("cxx", "cpp"),
    ("cyp", ""), ("cypher", ""), ("d", ""), ("dart", "dart"), ("dax", ""),
    ("desktop", ""), ("device", ""), ("diff", "diff"), ("dm", ""), ("do", ""),
    ("dockerfile", "dockerfile"), ("dta", ""), ("edge", ""), ("el", "emacs-lisp"), ("elm", "elm"),
    ("env", ""), ("erb", ""), ("erl", "erlang"), ("ex", "elixir"), ("exs", "elixir"),
    ("f", "fortran-fixed-form"), ("f03", "f03"), ("f08", "f08"), ("f18", ""), ("f77", "f77"),
    ("f90", "fortran-free-form"), ("f95", "fortran-free-form"), ("feature", ""), ("fish", ""), ("fnl", ""),
    ("for", "fortran-fixed-form"), ("frag", "glsl"), ("fs", "fsharp"), ("fsi", "fsharp"), ("fsx", "fsharp"),
    ("ftl", ""), ("fx", "hlsl"), ("gd", "gdscript"), ("gdshader", "gdshader"), ("gemspec", "ruby"),
    ("git-rebase-todo", "git-rebase"), ("gjs", ""), ("gleam", "gleam"), ("glsl", "glsl"), ("gnuplot", ""),
    ("go", "go"), ("gp", ""), ("gql", "graphql"), ("graphql", "graphql"), ("groovy", "groovy"),
    ("gs", ""), ("gts", ""), ("gvy", "groovy"), ("h", "objective-cpp"), ("hack", ""),
    ("haml", ""), ("handlebars", ""), ("hbs", ""), ("hcl", "hcl"), ("hh", "cpp"),
    ("hjson", ""), ("hlsl", "hlsl"), ("hpp", "cpp"), ("hrl", "erlang"), ("hs", "haskell"),
    ("htaccess", ""), ("htm", "html"), ("html", "html"), ("http", ""), ("hx", ""),
    ("hxml", ""), ("hy", ""), ("imba", ""), ("ini", ""), ("j2", ""),
    ("jade", ""), ("java", "java"), ("jbuilder", "ruby"), ("jinja", ""), ("jinja2", ""),
    ("jison", ""), ("jl", "julia"), ("js", "javascript"), ("json", "json"), ("json5", "json5"),
    ("jsonc", "jsonc"), ("jsonl", "jsonl"), ("jsonnet", ""), ("jssm", ""), ("jsx", "jsx"),
    ("kql", ""), ("kt", "kotlin"), ("kts", "kts"), ("lean", ""), ("less", "less"),
    ("lhs", "haskell"), ("libsonnet", ""), ("liquid", ""), ("lisp", "lisp"), ("lit", ""),
    ("ll", ""), ("log", ""), ("logo", ""), ("lsp", "lisp"), ("ltx", "tex"),
    ("lua", "lua"), ("luau", ""), ("m", ""), ("makefile", "makefile"), ("markdown", "markdown"),
    ("marko", ""), ("mat", "matlab"), ("md", "markdown"), ("mdc", ""), ("mdx", "mdx"),
    ("mediawiki", ""), ("mermaid", ""), ("mips", ""), ("mjs", "javascript"), ("mk", "makefile"),
    ("ml", "ocaml"), ("mli", "ocaml"), ("mll", "ocaml"), ("mly", "ocaml"), ("mm", "objective-cpp"),
    ("mmd", ""), ("mojo", ""), ("move", ""), ("mts", "typescript"), ("nar", ""),
    ("nb", ""), ("nf", ""), ("nim", "nim"), ("nimble", "nim"), ("nims", "nim"),
    ("nix", "nix"), ("nu", ""), ("odin", ""), ("p", "pascal"), ("p6", ""),
    ("pas", "pascal"), ("patch", "diff"), ("pcss", ""), ("php", "php"), ("phtml", "php"),
    ("pl", ""), ("pl6", ""), ("pls", ""), ("plt", ""), ("pm", "perl"),
    ("po", ""), ("podspec", "ruby"), ("polar", ""), ("pot", ""), ("potx", ""),
    ("pp", ""), ("pq", ""), ("pqm", ""), ("prisma", ""), ("pro", ""),
    ("properties", ""), ("proto", "protobuf"), ("ps1", "powershell"), ("psd1", "powershell"), ("psm1", "powershell"),
    ("pug", ""), ("purs", ""), ("py", "python"), ("pyi", "python"), ("pyw", "python"),
    ("ql", ""), ("qml", ""), ("qmldir", ""), ("qss", ""), ("r", "r"),
    ("rabl", "ruby"), ("rake", "ruby"), ("raku", ""), ("razor", ""), ("rb", "ruby"),
    ("rbw", "ruby"), ("reg", ""), ("regex", ""), ("rel", ""), ("res", "gdresource"),
    ("rest", ""), ("rkt", ""), ("rktl", ""), ("rq", ""), ("rs", "rust"),
    ("rst", ""), ("ru", "ruby"), ("s", ""), ("sas", ""), ("sass", "sass"),
    ("sc", "scala"), ("scala", "scala"), ("scm", "scheme"), ("scpt", ""), ("scss", "scss"),
    ("sdbl", ""), ("service", ""), ("sh", "zsh"), ("shader", ""), ("shadergraph", ""),
    ("sky", "python"), ("sld", "scheme"), ("socket", ""), ("sol", "solidity"), ("soy", ""),
    ("sparql", ""), ("spl", ""), ("sql", "sql"), ("ss", "scheme"), ("st", ""),
    ("star", "python"), ("sty", "tex"), ("styl", ""), ("stylus", ""), ("sv", ""),
    ("svelte", "svelte"), ("svh", ""), ("swift", "swift"), ("t", "perl"), ("talon", ""),
    ("tasl", ""), ("tcl", ""), ("templ", ""), ("tex", "tex"), ("tf", "tf"),
    ("tfvars", "tfvars"), ("timer", ""), ("toml", "toml"), ("tres", "gdresource"), ("trigger", ""),
    ("ts", "typescript"), ("tsp", ""), ("tsv", ""), ("tsx", "tsx"), ("ttl", ""),
    ("twig", ""), ("typ", ""), ("v", ""), ("vala", ""), ("vapi", ""),
    ("vb", ""), ("vbs", ""), ("vert", "glsl"), ("vh", ""), ("vhd", ""),
    ("vhdl", ""), ("vim", ""), ("vine.ts", ""), ("vue", "vue"), ("vv", ""),
    ("vy", ""), ("wasm", "wasm"), ("wat", "wasm"), ("wgsl", "wgsl"), ("wiki", ""),
    ("wit", ""), ("wl", ""), ("wy", ""), ("xml", "xml"), ("xsl", ""),
    ("xslt", ""), ("yaml", "yaml"), ("yml", "yml"), ("zig", "zig"), ("zs", ""),
    ("zsh", "zsh"), ("🔥", ""),
];
