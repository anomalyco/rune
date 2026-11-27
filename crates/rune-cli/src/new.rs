//! Project scaffolding for `rune new <path>`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::args::NewArgs;
use crate::{CliError, DEFAULT_REPO_URL, DEFAULT_REV, default_patches};

const RUST_KEYWORDS: &[&str] = &[
    "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum",
    "extern", "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move",
    "mut", "pub", "ref", "return", "self", "Self", "static", "struct", "super", "trait", "true",
    "type", "unsafe", "use", "where", "while", "abstract", "become", "box", "do", "final",
    "macro", "override", "priv", "try", "typeof", "unsized", "virtual", "yield", "gen",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuneSource {
    Git { url: String, rev: Option<String> },
    LocalPath(PathBuf),
}

/// Validates a Cargo package name.
pub fn validate_package_name(name: &str) -> Result<(), CliError> {
    if name.is_empty() {
        return Err(CliError::usage("package name cannot be empty"));
    }
    let mut chars = name.chars();
    let first = chars.next().unwrap_or('\0');
    if !(first.is_ascii_alphabetic() || first == '_') {
        return Err(CliError::usage(format!(
            "invalid package name '{name}': must start with an ASCII letter or '_'"
        )));
    }
    if !chars.all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err(CliError::usage(format!(
            "invalid package name '{name}': only ASCII letters, digits, '-' and '_' are allowed"
        )));
    }
    if RUST_KEYWORDS.contains(&name) {
        return Err(CliError::usage(format!(
            "invalid package name '{name}': Rust keyword cannot be used as a package name"
        )));
    }
    Ok(())
}

/// Executes `rune new`, writing a ready-to-run Rune app to `opts.target`.
pub fn run(opts: &NewArgs) -> Result<(), CliError> {
    let cwd = std::env::current_dir()
        .map_err(|err| CliError::new(format!("cannot read current directory: {err}")))?;
    let target_dir = if opts.target.is_absolute() {
        opts.target.clone()
    } else {
        cwd.join(&opts.target)
    };

    let pkg_name = match &opts.name {
        Some(n) => n.clone(),
        None => target_dir
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| {
                CliError::usage(
                    "could not infer package name from path; pass --name <package>",
                )
            })?
            .to_string(),
    };
    validate_package_name(&pkg_name)?;

    if target_dir.exists() && !opts.force {
        let mut entries = fs::read_dir(&target_dir).map_err(|err| {
            CliError::new(format!(
                "failed to inspect {}: {err}",
                target_dir.display()
            ))
        })?;
        if entries.next().is_some() {
            return Err(CliError::new(format!(
                "destination {} already exists and is not empty (pass --force to overwrite)",
                target_dir.display()
            )));
        }
    }

    let source = if let Some(rune_path) = &opts.rune_path {
        let resolved = if rune_path.is_absolute() {
            rune_path.clone()
        } else {
            cwd.join(rune_path)
        };
        let resolved = crate::config::canonical_or_self(&resolved)?;
        if !resolved.join("crates/rune/Cargo.toml").is_file() {
            return Err(CliError::new(format!(
                "--path {} does not look like a Rune repository (missing crates/rune/Cargo.toml)",
                resolved.display()
            )));
        }
        RuneSource::LocalPath(resolved)
    } else {
        let url = opts
            .git
            .clone()
            .unwrap_or_else(|| DEFAULT_REPO_URL.to_string());
        let rev = opts.rev.clone().or_else(|| {
            if DEFAULT_REV.is_empty() {
                None
            } else {
                Some(DEFAULT_REV.to_string())
            }
        });
        RuneSource::Git { url, rev }
    };

    let patches = match &source {
        RuneSource::LocalPath(root) => {
            let root_manifest = root.join("Cargo.toml");
            fs::read_to_string(&root_manifest)
                .ok()
                .and_then(|t| crate::pins::required_patches_from_manifest(&t).ok())
                .filter(|v| !v.is_empty())
                .unwrap_or_else(default_patches)
        }
        RuneSource::Git { .. } => default_patches(),
    };

    let src_dir = target_dir.join("src");
    fs::create_dir_all(&src_dir).map_err(|err| {
        CliError::new(format!(
            "failed to create {}: {err}",
            src_dir.display()
        ))
    })?;

    write_file(
        &target_dir.join("Cargo.toml"),
        &render_cargo_toml(&pkg_name, &source, &patches),
    )?;
    write_file(&target_dir.join("rune.toml"), &render_rune_toml(&pkg_name))?;
    write_file(
        &src_dir.join("main.rs"),
        &render_main_rs(&display_title(&pkg_name)),
    )?;
    write_file(&src_dir.join("view.rs"), RENDER_VIEW_RS)?;
    write_file(&target_dir.join(".gitignore"), "/target\n")?;
    write_file(
        &target_dir.join("README.md"),
        &render_readme(&pkg_name, &source),
    )?;

    if !opts.no_git && !inside_git_repo(&target_dir) {
        Command::new("git")
            .args(["init", "-q"])
            .current_dir(&target_dir)
            .status()
            .ok();
    }

    println!("Created Rune app `{pkg_name}` in {}", opts.target.display());
    println!();
    println!("Next steps:");
    println!("  cd {}", opts.target.display());
    println!("  rune dev");
    Ok(())
}

/// Renders the generated project's `Cargo.toml`.
pub fn render_cargo_toml(pkg_name: &str, source: &RuneSource, patches: &[String]) -> String {
    let mut out = format!(
        "[package]\nname = \"{pkg_name}\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[features]\nhot = [\"rune/hot\"]\n\n[dependencies]\n"
    );

    match source {
        RuneSource::Git { url, rev } => {
            let spec = git_spec(url, rev.as_deref());
            out.push_str(&format!("rune = {spec}\n\n"));
            if rev.is_none() {
                out.push_str(
                    "# Tip: pin a specific `dev` commit with `rev = \"<sha>\"` so builds stay reproducible.\n",
                );
            }
            out.push_str(
                "# Cargo does not inherit [patch.crates-io] from dependencies; keep these on the same URL and rev as `rune`.\n[patch.crates-io]\n",
            );
            for patch in patches {
                out.push_str(&format!("{patch} = {spec}\n"));
            }
        }
        RuneSource::LocalPath(root) => {
            let rune_crate = toml_path(&root.join("crates/rune"));
            out.push_str(&format!("rune = {{ path = \"{rune_crate}\" }}\n\n"));
            out.push_str(
                "# Cargo does not inherit [patch.crates-io] from dependencies; match Rune's root Cargo.toml.\n[patch.crates-io]\n",
            );
            for patch in patches {
                let vendor = toml_path(&root.join("vendor").join(patch));
                out.push_str(&format!("{patch} = {{ path = \"{vendor}\" }}\n"));
            }
        }
    }

    out.push_str(
        "\n[profile.rune-hot]\ninherits = \"dev\"\ndebug = 0\nincremental = true\n\n[profile.rune-hot.package.png]\nopt-level = 3\n\n[profile.rune-hot.package.fdeflate]\nopt-level = 3\n",
    );
    out
}

pub fn render_rune_toml(pkg_name: &str) -> String {
    format!(
        "[dev]\npatch = [\"src/view.rs\", \"src/ui/**/*.rs\"]\nfeatures = []\nbin = \"{pkg_name}\"\n"
    )
}

fn render_main_rs(title: &str) -> String {
    format!(
        r#"#![forbid(unsafe_code)]

mod view;

use rune::AppSpec;

#[derive(Default)]
pub struct State {{
    pub on: bool,
}}

fn main() {{
    rune::run(AppSpec::<State> {{
        scenes: &[("on", |s| s.on = true)],
        ..AppSpec::new("{title}", (360.0, 220.0), State::default, view::view)
    }});
}}

#[cfg(test)]
mod tests {{
    use super::*;
    use rune::{{Env, Runtime}};

    #[test]
    fn tapping_the_switch_springs_the_knob_across() {{
        let env = Env {{
            system_dark: true,
            scale: 2.0,
        }};
        let mut rt = Runtime::new(State::default(), view::view, (360.0, 220.0), env);
        rt.frame(0.0);
        let knob_x = |rt: &Runtime<State>| rt.presented("knob").unwrap().0[0];
        let off = knob_x(&rt);

        let (x, y) = rt.find("switch").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        for _ in 0..6 {{
            rt.frame(1.0 / 60.0);
        }}
        assert!(rt.state.on);
        let mid = knob_x(&rt);
        for _ in 0..60 {{
            rt.frame(1.0 / 60.0);
        }}
        let on = knob_x(&rt);
        assert!(off < mid && mid < on, "knob travels: {{off}} -> {{mid}} -> {{on}}");
        assert!((on - off - 16.0).abs() < 0.01);
    }}
}}
"#
    )
}

const RENDER_VIEW_RS: &str = r#"//! View function for the app.
//! Under `rune dev`, edits to function bodies in this file patch live without restarting.

use rune::tok::{self, motion, type_};
use rune::{Cx, El, col, el, text};

use crate::State;

pub fn view(s: &State, cx: &mut Cx<State>) -> El {
    let dark = cx.system_dark();
    let t = tok::theme(dark);

    let title = text(if s.on { "Lights on" } else { "Lights off" })
        .font(type_::TITLE)
        .color(t.text);

    let knob = el()
        .key("knob")
        .size(18.0, 18.0)
        .radius(9.0)
        .fill(t.knob)
        .absolute(if s.on { 18.0 } else { 2.0 }, 2.0);

    let switch = el()
        .key("switch")
        .role("switch")
        .label("Lights")
        .checked(s.on)
        .focusable(true)
        .size(38.0, 22.0)
        .radius(11.0)
        .fill(if s.on {
            tok::accent(0, dark)
        } else {
            t.control_off
        })
        .motion(motion::SNAPPY)
        .on_press(|s: &mut State| s.on = !s.on)
        .child(knob);

    col()
        .w_full()
        .h_full()
        .center()
        .gap(16.0)
        .fill(t.bg)
        .child(title)
        .child(switch)
}
"#;

fn render_readme(pkg_name: &str, source: &RuneSource) -> String {
    let pin_note = match source {
        RuneSource::Git { rev: Some(rev), .. } => {
            let short = &rev[..rev.len().min(12)];
            format!(
                "Pinned to Rune `dev` revision `{short}`. Update `rune` and every `[patch.crates-io]` entry together, then run `rune check --pins-only`.\n"
            )
        }
        RuneSource::Git { rev: None, .. } => {
            "Tracks Rune `dev`. Pin a specific `rev = \"<sha>\"` on `rune` and every `[patch.crates-io]` entry before shipping, then verify with `rune check --pins-only`.\n".to_string()
        }
        RuneSource::LocalPath(_) => {
            "Configured against a local Rune checkout via `path` dependencies.\n".to_string()
        }
    };

    format!(
        "# {pkg_name}\n\nA Rune desktop application.\n\n## Develop\n\n```sh\nrune dev\n```\n\nEdit `src/view.rs` to see live updates. Struct/state or signature changes automatically trigger a rebuild and relaunch.\n\n## Verify\n\n```sh\nrune check\ncargo test\ncargo run -- --snap preview.png --scene on\n```\n\n## Rune pin\n\n{pin_note}"
    )
}

fn git_spec(url: &str, rev: Option<&str>) -> String {
    match rev {
        Some(r) => format!("{{ git = \"{url}\", rev = \"{r}\" }}"),
        None => format!("{{ git = \"{url}\", branch = \"dev\" }}"),
    }
}

fn toml_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn display_title(pkg_name: &str) -> String {
    pkg_name
        .split(['-', '_'])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => format!("{}{}", first.to_ascii_uppercase(), chars.as_str()),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn inside_git_repo(dir: &Path) -> bool {
    let mut cur = Some(dir);
    while let Some(d) = cur {
        if d.join(".git").exists() {
            return true;
        }
        cur = d.parent();
    }
    false
}

fn write_file(path: &Path, content: &str) -> Result<(), CliError> {
    fs::write(path, content)
        .map_err(|err| CliError::new(format!("failed to write {}: {err}", path.display())))
}
