use super::*;

pub(in crate::text) fn load_emoji(collection: &mut Collection, symbols: &[&str]) -> Vec<String> {
    let mut names = Vec::new();
    #[cfg(all(unix, not(target_vendor = "apple"), not(target_os = "android")))]
    {
        static MATCHES: std::sync::OnceLock<Vec<(String, std::path::PathBuf)>> =
            std::sync::OnceLock::new();
        for (family, file) in MATCHES.get_or_init(|| {
            std::process::Command::new("fc-match")
                .args(["-s", "--format=%{family[0]}\t%{file}\t%{color}\n", "emoji"])
                .output()
                .ok()
                .filter(|out| out.status.success())
                .and_then(|out| String::from_utf8(out.stdout).ok())
                .map(|out| candidates(&out))
                .unwrap_or_default()
        }) {
            collection.load_fonts_from_paths([file]);
            if collection.family_id(family).is_some() && !names.contains(family) {
                names.push(family.clone());
            }
        }
    }
    for family in symbols.iter().filter(|name| name.contains("Emoji")) {
        if !names.iter().any(|name| name == family) {
            names.push((*family).to_owned());
        }
    }
    names
}

#[cfg(any(
    test,
    all(unix, not(target_vendor = "apple"), not(target_os = "android"))
))]
fn candidates(output: &str) -> Vec<(String, std::path::PathBuf)> {
    let mut colors = Vec::new();
    let mut monochrome = None;
    for line in output.lines() {
        let mut fields = line.split('\t');
        let (Some(family), Some(file), Some(color)) = (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        if family.is_empty() || !std::path::Path::new(file).is_absolute() {
            continue;
        }
        let entry = (family.to_owned(), file.into());
        if color.eq_ignore_ascii_case("true") {
            if !colors.iter().any(|(name, _)| name == family) {
                colors.push(entry);
            }
        } else if monochrome.is_none() {
            monochrome = Some(entry);
        }
    }
    colors.extend(monochrome);
    colors
}

#[cfg(test)]
#[path = "emoji_fonts_tests.rs"]
mod tests;
