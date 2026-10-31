use std::cmp::Ordering;

use rune::Icon;

use super::TreeModel;

/// A node of a file tree: a folder (`dir`) or a file carrying the app's data.
#[derive(Clone, Debug, PartialEq)]
pub struct FileNode<D> {
    /// What the row shows: the file or folder name, or `a/b/c` for folders compacted into one.
    pub name: String,
    /// The path from the root, which is also the node's id.
    pub path: String,
    pub dir: bool,
    /// The file's data; `None` for folders.
    pub data: Option<D>,
}

impl<D> FileNode<D> {
    pub fn label(&self) -> &str {
        &self.name
    }
    /// The folder or file icon for this node ([`file_icon`]).
    pub fn icon(&self, open: bool) -> &'static Icon {
        if self.dir {
            if open { &crate::glyphs::FOLDER_OPEN } else { &crate::glyphs::FOLDER }
        } else {
            file_icon(&self.name)
        }
    }
}

/// An icon for a file by its extension: code, text and docs, images, or a plain page.
pub fn file_icon(name: &str) -> &'static Icon {
    let ext = name.rsplit_once('.').map_or("", |(_, e)| e).to_ascii_lowercase();
    match ext.as_str() {
        "rs" | "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" | "py" | "go" | "swift" | "c" | "h" | "cc" | "cpp" | "hpp" | "m" | "mm"
        | "java" | "kt" | "rb" | "php" | "cs" | "zig" | "lua" | "sh" | "bash" | "zsh" | "fish" | "wgsl" | "glsl" | "metal" | "html"
        | "css" | "scss" | "vue" | "svelte" | "sql" | "ex" | "exs" | "hs" | "ml" | "scala" | "dart" => &crate::glyphs::FILE_CODE,
        "md" | "mdx" | "txt" | "rst" | "adoc" | "json" | "jsonc" | "toml" | "yaml" | "yml" | "lock" | "xml" | "csv" | "ini" | "cfg"
        | "conf" | "env" => &crate::glyphs::FILE_TEXT,
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "ico" | "bmp" | "tiff" | "avif" | "heic" => &crate::glyphs::FILE_IMAGE,
        _ => &crate::glyphs::FILE,
    }
}

/// Builds a [`TreeModel`] of [`FileNode`]s from file paths (`/`-separated, relative to a root).
///
/// By default folders sort before files, names sort naturally (`file2` before `file10`, case
/// aside), and a chain of folders that each hold only one folder shows as one row named
/// `a/b/c`, as an editor's compact folders do. Folders start closed; open them on the model.
pub struct FileTree {
    compact: bool,
    dirs_first: bool,
    key: String,
}

impl Default for FileTree {
    fn default() -> Self {
        Self::new()
    }
}

impl FileTree {
    pub fn new() -> FileTree {
        FileTree { compact: true, dirs_first: true, key: "files".into() }
    }
    /// Show a chain of single-folder folders as one `a/b/c` row (on by default).
    pub fn compact_folders(mut self, on: bool) -> Self {
        self.compact = on;
        self
    }
    /// Sort folders before files (on by default); off, they mix by name.
    pub fn dirs_first(mut self, on: bool) -> Self {
        self.dirs_first = on;
        self
    }
    /// The model's row-key prefix ([`TreeModel::key`], `"files"` here).
    pub fn key(mut self, key: impl Into<String>) -> Self {
        self.key = key.into();
        self
    }

    pub fn build<P: AsRef<str>, D>(self, files: impl IntoIterator<Item = (P, D)>) -> TreeModel<FileNode<D>> {
        // A trie of folders; files hang off their folder.
        struct Dir<D> {
            dirs: Vec<(String, Dir<D>)>,
            files: Vec<(String, D)>,
        }
        impl<D> Dir<D> {
            fn dir(&mut self, name: &str) -> &mut Dir<D> {
                let i = match self.dirs.iter().position(|(n, _)| n == name) {
                    Some(i) => i,
                    None => {
                        self.dirs.push((name.to_owned(), Dir { dirs: Vec::new(), files: Vec::new() }));
                        self.dirs.len() - 1
                    }
                };
                &mut self.dirs[i].1
            }
        }
        let mut root: Dir<D> = Dir { dirs: Vec::new(), files: Vec::new() };
        for (path, data) in files {
            let path = path.as_ref().trim_matches('/');
            if path.is_empty() {
                continue;
            }
            let mut parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
            let file = parts.pop().unwrap();
            let dir = parts.iter().fold(&mut root, |d, p| d.dir(p));
            dir.files.push((file.to_owned(), data));
        }
        let mut model = TreeModel::new().key(self.key.clone());
        fn emit<D>(model: &mut TreeModel<FileNode<D>>, compact: bool, parent: Option<&str>, prefix: &str, dir: Dir<D>) {
            for (name, mut d) in dir.dirs {
                let mut name = name;
                let mut path = join(prefix, &name);
                while compact && d.files.is_empty() && d.dirs.len() == 1 {
                    let (n, inner) = d.dirs.pop().unwrap();
                    name = format!("{name}/{n}");
                    path = join(&path, &n);
                    d = inner;
                }
                model.insert(parent, path.clone(), FileNode { name, path: path.clone(), dir: true, data: None });
                emit(model, compact, Some(&path), &path, d);
            }
            for (name, data) in dir.files {
                let path = join(prefix, &name);
                // A file sharing a folder's path (`a` and `a/b`) is left out.
                model.insert(parent, path.clone(), FileNode { name, path, dir: false, data: Some(data) });
            }
        }
        fn join(a: &str, b: &str) -> String {
            if a.is_empty() { b.to_owned() } else { format!("{a}/{b}") }
        }
        emit(&mut model, self.compact, None, "", root);
        let dirs_first = self.dirs_first;
        model.sort_by(|a, b| {
            let by_kind = if dirs_first { b.dir.cmp(&a.dir) } else { Ordering::Equal };
            by_kind.then_with(|| natural_cmp(&a.name, &b.name))
        });
        model
    }
}

/// Compares names as people sort them: case aside, and runs of digits by value (`file2` before
/// `file10`). Ties fall back to the exact text so the order is total.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut x, mut y) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (x.peek().copied(), y.peek().copied()) {
            (None, None) => return a.cmp(b),
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(p), Some(q)) if p.is_ascii_digit() && q.is_ascii_digit() => {
                let take = |it: &mut std::iter::Peekable<std::str::Chars>| {
                    let mut s = String::new();
                    while let Some(&c) = it.peek().filter(|c| c.is_ascii_digit()) {
                        s.push(c);
                        it.next();
                    }
                    s
                };
                let (m, n) = (take(&mut x), take(&mut y));
                let (mt, nt) = (m.trim_start_matches('0'), n.trim_start_matches('0'));
                let o = mt.len().cmp(&nt.len()).then_with(|| mt.cmp(nt));
                if o != Ordering::Equal {
                    return o;
                }
            }
            (Some(p), Some(q)) => {
                let o = p.to_lowercase().cmp(q.to_lowercase());
                if o != Ordering::Equal {
                    return o;
                }
                x.next();
                y.next();
            }
        }
    }
}
