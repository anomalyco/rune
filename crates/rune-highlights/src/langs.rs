//! Highlights' language names and token types, from `languages.js` and `token-types.js` in
//! `@pierre/highlights` 0.2.0.

/// Every name and alias Highlights accepts (lowercase), sorted, with its lexer's id.
pub(crate) const ALIASES: [(&str, u8); 211] = [
    ("angular-html", 1), ("angular-ts", 72), ("asm", 2), ("assembly", 2), ("astro", 3), ("bash", 4),
    ("bat", 5), ("batch", 5), ("batchfile", 5), ("c", 6), ("c#", 11), ("c++", 10),
    ("c3", 7), ("cc", 10), ("cjs", 35), ("cl", 44), ("clj", 8), ("cljc", 8),
    ("cljs", 8), ("clojure", 8), ("cmake", 9), ("cmd", 5), ("comp", 27), ("containerfile", 16),
    ("cpp", 10), ("cs", 11), ("csharp", 11), ("css", 12), ("cts", 72), ("cu", 13),
    ("cuda", 13), ("cuh", 13), ("cxx", 10), ("dart", 14), ("delphi", 54), ("diff", 15),
    ("docker", 16), ("dockerfile", 16), ("dos", 5), ("dpk", 54), ("dpr", 54), ("edn", 8),
    ("el", 44), ("elisp", 44), ("elixir", 17), ("elm", 18), ("emacs-lisp", 44), ("erl", 19),
    ("erlang", 19), ("ex", 17), ("exs", 17), ("f", 21), ("f#", 22), ("f03", 20),
    ("f08", 20), ("f77", 21), ("f90", 20), ("f95", 20), ("for", 21), ("fortran", 20),
    ("fortran-fixed-form", 21), ("fortran-free-form", 20), ("frag", 27), ("fs", 22), ("fsharp", 22), ("fsi", 22),
    ("fsx", 22), ("gd", 24), ("gdresource", 23), ("gdscript", 24), ("gdshader", 25), ("gdshaderinc", 25),
    ("geom", 27), ("git-commit", 15), ("git-rebase", 15), ("gleam", 26), ("glsl", 27), ("go", 28),
    ("godot-resource", 23), ("godot-shader", 25), ("golang", 28), ("gql", 29), ("gradle", 30), ("graphql", 29),
    ("groovy", 30), ("gsh", 30), ("gvy", 30), ("gy", 30), ("h", 6), ("haskell", 31),
    ("hcl", 70), ("hh", 10), ("hlsl", 32), ("hpp", 10), ("hrl", 19), ("hs", 31),
    ("htm", 33), ("html", 33), ("hxx", 10), ("java", 34), ("javascript", 35), ("jl", 40),
    ("js", 35), ("json", 36), ("json5", 37), ("jsonc", 36), ("jsonl", 38), ("jsx", 39),
    ("julia", 40), ("kotlin", 41), ("kt", 41), ("kts", 41), ("latex", 42), ("less", 43),
    ("lisp", 44), ("lpr", 54), ("lsp", 44), ("lua", 45), ("m", 52), ("make", 46),
    ("makefile", 46), ("markdown", 47), ("matlab", 48), ("md", 47), ("mdx", 49), ("mjs", 35),
    ("mk", 46), ("ml", 53), ("mli", 53), ("mm", 52), ("mts", 72), ("nim", 50),
    ("nix", 51), ("objc", 52), ("objcpp", 52), ("object-pascal", 54), ("objective-c", 52), ("objective-cpp", 52),
    ("objectivec", 52), ("objectpascal", 54), ("ocaml", 53), ("octave", 48), ("pas", 54), ("pascal", 54),
    ("patch", 15), ("perl", 55), ("php", 56), ("pl", 55), ("plain", 0), ("plaintext", 0),
    ("pm", 55), ("powershell", 57), ("pp", 54), ("proto", 58), ("protobuf", 58), ("ps", 57),
    ("ps1", 57), ("psd1", 57), ("psm1", 57), ("pwsh", 57), ("py", 59), ("python", 59),
    ("r", 60), ("rb", 61), ("rs", 62), ("rscript", 60), ("ruby", 61), ("rust", 62),
    ("s", 2), ("sass", 63), ("sbt", 64), ("sc", 64), ("scala", 64), ("scheme", 44),
    ("scm", 44), ("scss", 65), ("sh", 4), ("shell", 4), ("shellscript", 4), ("shellsession", 4),
    ("sol", 66), ("solidity", 66), ("sql", 67), ("svelte", 68), ("svg", 78), ("swift", 69),
    ("terraform", 70), ("tex", 42), ("text", 0), ("tf", 70), ("tfvars", 70), ("toml", 71),
    ("tres", 23), ("ts", 72), ("tscn", 23), ("tsrx", 73), ("tsx", 74), ("txt", 0),
    ("typescript", 72), ("vert", 27), ("vue", 75), ("wasm", 76), ("wat", 76), ("wgsl", 77),
    ("xml", 78), ("xsd", 78), ("yaml", 79), ("yml", 79), ("zig", 80), ("zon", 80),
    ("zsh", 4),
];

/// Each lexer's name, by id.
pub(crate) const NAMES: [&str; 81] = [
    "plain", "angular-html", "asm", "astro", "bash", "bat", "c", "c3",
    "clojure", "cmake", "cpp", "csharp", "css", "cuda", "dart", "diff",
    "dockerfile", "elixir", "elm", "erlang", "fortran", "fortran-fixed-form", "fsharp", "gdresource",
    "gdscript", "gdshader", "gleam", "glsl", "go", "graphql", "groovy", "haskell",
    "hlsl", "html", "java", "javascript", "json", "json5", "jsonl", "jsx",
    "julia", "kotlin", "latex", "less", "lisp", "lua", "makefile", "markdown",
    "matlab", "mdx", "nim", "nix", "objective-c", "ocaml", "pascal", "perl",
    "php", "powershell", "proto", "python", "r", "ruby", "rust", "sass",
    "scala", "scss", "solidity", "sql", "svelte", "swift", "terraform", "toml",
    "typescript", "tsrx", "tsx", "vue", "wasm", "wgsl", "xml", "yaml",
    "zig",
];

/// Each token type's Zed capture name, by id.
pub(crate) const TOKENS: [&str; 73] = [
    "none", "attribute", "attribute.jsx", "boolean", "comment", "comment.doc",
    "constant", "constant.builtin", "constructor", "diff.minus", "diff.plus", "embedded",
    "emphasis", "emphasis.strong", "function", "function.definition", "function.method", "keyword",
    "keyword.control", "keyword.declaration", "keyword.import", "keyword.operator", "label", "namespace",
    "number", "operator", "predictive", "preproc", "primary", "property",
    "property.json_key", "punctuation", "punctuation.bracket", "punctuation.bracket.html", "punctuation.bracket.jsx", "punctuation.delimiter",
    "punctuation.delimiter.html", "punctuation.delimiter.jsx", "punctuation.list_marker", "punctuation.markup", "punctuation.special", "selector",
    "selector.class", "selector.id", "string", "string.escape", "string.regex", "string.special",
    "string.special.symbol", "tag", "tag.doctype", "tag.jsx", "text.jsx", "text.literal",
    "title", "type", "type.builtin", "type.class", "variable", "variable.parameter",
    "variable.special", "enum", "hint", "keyword.jsdoc", "link_text", "link_uri",
    "selector.pseudo", "tag.component.jsx", "type.jsdoc", "variable.jsdoc", "variant", "background",
    "foreground",
];
