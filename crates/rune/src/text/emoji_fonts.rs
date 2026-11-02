use super::*;

pub(in crate::text) fn load_emoji(
    collection: &mut Collection,
    source_cache: &mut SourceCache,
    symbols: &mut Vec<&'static str>,
) -> Vec<String> {
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
            let loaded = collection.family_by_name(family).is_some_and(|info| {
                info.fonts().iter().any(|font| {
                    matches!(font.source().kind(),
                    parley::fontique::SourceKind::Path(path) if path.as_ref() == file.as_path())
                })
            });
            if !loaded {
                collection.load_fonts_from_paths([file]);
            }
            if retain_usable_fonts(collection, source_cache, family) && !names.contains(family) {
                names.push(family.clone());
            }
        }
    }
    symbols.retain(|family| {
        !family.contains("Emoji") || retain_usable_fonts(collection, source_cache, family)
    });
    for family in symbols.iter().filter(|name| name.contains("Emoji")) {
        if !names.iter().any(|name| name == family) {
            names.push((*family).to_owned());
        }
    }
    names
}

fn retain_usable_fonts(
    collection: &mut Collection,
    source_cache: &mut SourceCache,
    name: &str,
) -> bool {
    let Some(family) = collection.family_by_name(name) else {
        return false;
    };
    let mut usable = false;
    for font in family.fonts() {
        let supported = font.load(Some(source_cache)).is_some_and(|data| {
            FontRef::from_index(data.as_ref(), font.index() as usize).is_some_and(|face| {
                supported_color_format(
                    face.table(swash::tag_from_bytes(b"COLR")),
                    face.table(swash::tag_from_bytes(b"CBDT")).is_some()
                        || face.table(swash::tag_from_bytes(b"sbix")).is_some(),
                    face.table(swash::tag_from_bytes(b"SVG ")).is_some(),
                )
            })
        });
        if supported {
            usable = true;
        } else {
            collection.unregister_font(family.id(), font.width(), font.style(), font.weight());
        }
    }
    usable
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

fn supported_color_format(colr: Option<&[u8]>, bitmap: bool, svg: bool) -> bool {
    bitmap
        || match colr {
            Some(table) => {
                table.get(2..4).is_some_and(|records| records != [0, 0]) || paints(table)
            }
            None => !svg,
        }
}

fn paints(table: &[u8]) -> bool {
    use skrifa::raw::FontRead;
    skrifa::raw::tables::colr::Colr::read(skrifa::raw::FontData::new(table))
        .ok()
        .and_then(|colr| colr.base_glyph_list()?.ok())
        .is_some_and(|list| list.num_base_glyph_paint_records() > 0)
}

#[cfg(test)]
#[path = "emoji_fonts_tests.rs"]
mod tests;
