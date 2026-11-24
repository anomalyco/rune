//! Freedesktop Icon Theme Specification lookup, desktop theme detection, and bounded theme
//! index caching (`themed_icon_rgba`, `ImageCache::themed_icon`).
//!
//! Follows the Freedesktop Icon Theme Specification: parses `index.theme` (`[Icon Theme]`
//! `Inherits`, `Directories`, `ScaledDirectories` and per-directory `Size`, `Scale`, `Type`,
//! `MinSize`, `MaxSize`, `Threshold`, `Context`), walks `Inherits` breadth-first ending at
//! `hicolor` and then `pixmaps`, and selects icons with `DirectoryMatchesSize` followed by
//! `DirectorySizeDistance`.

use super::{Image, ImageCache, ImageError, Input, icon_pixels};

pub(crate) struct ThemedIconRequest {
    #[cfg_attr(not(any(target_os = "linux", all(test, not(target_arch = "wasm32")))), allow(dead_code))]
    pub names: Vec<String>,
    #[cfg_attr(not(any(target_os = "linux", all(test, not(target_arch = "wasm32")))), allow(dead_code))]
    pub theme: Option<String>,
    #[cfg_attr(not(any(target_os = "linux", all(test, not(target_arch = "wasm32")))), allow(dead_code))]
    pub nominal_px: u32,
    #[cfg_attr(not(any(target_os = "linux", all(test, not(target_arch = "wasm32")))), allow(dead_code))]
    pub scale: u32,
    #[cfg_attr(not(any(target_os = "linux", all(test, not(target_arch = "wasm32")))), allow(dead_code))]
    pub raster_px: u32,
    #[cfg(all(test, not(target_arch = "wasm32")))]
    pub data_dirs: Option<Vec<std::path::PathBuf>>,
}

/// Rasterize the best matching Linux freedesktop themed icon for the fallback `names` list at
/// nominal size `px` and integer scale factor `scale`, returning straight-alpha sRGB RGBA8.
///
/// Most code wants [`ImageCache::themed_icon`], which runs lookup and rasterization off the UI
/// thread on the cache's decoder workers and caches handles.
///
/// - `names` is tried in order (most specific first, like `GThemedIcon`), searching all
///   directories of the active theme chain (`apps/`, `mimetypes/`, `places/`, etc.) and then
///   `pixmaps/`.
/// - `theme` names the starting icon theme (for example `Some("Papirus")`); `None` detects the
///   current Linux desktop theme from GTK `settings.ini` / KDE `kdeglobals` with an
///   `Adwaita` → `hicolor` fallback.
/// - Returns `Ok(None)` when no matching icon exists (and on non-Linux platforms, where
///   freedesktop icon themes are not installed).
/// - Returns `Err(ImageError)` when `px` is outside `1..=1024`, `scale` is `0`, or a matched
///   icon file fails to read or decode.
///
/// # Example
/// ```
/// use rune_image::themed_icon_rgba;
///
/// let missing = themed_icon_rgba(&["rune-nonexistent-icon-name"], Some("hicolor"), 24, 1);
/// assert_eq!(missing, Ok(None));
/// ```
pub fn themed_icon_rgba(
    names: &[&str],
    theme: Option<&str>,
    px: u32,
    scale: u32,
) -> Result<Option<(u32, u32, Vec<u8>)>, ImageError> {
    if px == 0 || px > 1024 { return Err(ImageError("themed icon size must be 1..=1024 pixels".into())); }
    if scale == 0 { return Err(ImageError("themed icon scale must be at least 1".into())); }
    #[cfg(target_os = "linux")]
    {
        let dirs = data_dirs();
        let home = home_dir();
        themed_icon_rgba_in(names, theme, px, scale, &dirs, home.as_deref())
    }
    #[cfg(not(target_os = "linux"))]
    { let _ = (names, theme); Ok(None) }
}

impl ImageCache {
    /// Looks up a Linux freedesktop themed icon by fallback `names` (most specific first, as
    /// `GThemedIcon` provides) in `theme` (or the detected desktop theme when `None`) at
    /// `size_pt` points square and display `scale`.
    ///
    /// Lookup follows the Freedesktop Icon Theme Specification (`index.theme` metadata,
    /// `Inherits` breadth-first ending at `hicolor`, then `pixmaps`) across all theme contexts
    /// (`apps/`, `mimetypes/`, `places/`, etc.), rasterizing PNG and SVG icons off the UI thread
    /// on the cache's decoder workers. Handles are shared by `(names, theme, size_pt, scale)`.
    ///
    /// When no matching icon exists (including on non-Linux platforms), the returned [`Image`]
    /// settles with [`Image::is_not_found`] returning `true` and [`Image::error`] returning
    /// `Some`; when a matched file is unreadable or corrupt, [`Image::error`] returns `Some`
    /// while [`Image::is_not_found`] returns `false`.
    ///
    /// # Example
    /// ```
    /// use std::sync::Arc;
    /// use rune_image::ImageCache;
    ///
    /// let cache = ImageCache::new(Arc::new(|| {}));
    /// let icon = cache.themed_icon(&["rune-nonexistent-icon-name"], Some("hicolor"), 24.0, 2.0);
    /// assert!(icon.wait().is_err());
    /// assert!(icon.is_not_found());
    /// ```
    pub fn themed_icon(&self, names: &[&str], theme: Option<&str>, size_pt: f32, scale: f32) -> Image {
        let (nominal_px, scale_int, raster_px) = themed_icon_dims(size_pt, scale);
        let id = themed_icon_id(names, theme, nominal_px, scale_int, raster_px);
        self.request(id, Input::ThemedIcon(Box::new(ThemedIconRequest {
            names: names.iter().map(|s| (*s).to_owned()).collect(),
            theme: theme.map(str::to_owned),
            nominal_px, scale: scale_int, raster_px,
            #[cfg(all(test, not(target_arch = "wasm32")))]
            data_dirs: None,
        })))
    }

    #[cfg(all(test, not(target_arch = "wasm32")))]
    pub(crate) fn themed_icon_in(
        &self, names: &[&str], theme: Option<&str>, size_pt: f32, scale: f32, data_dirs: Vec<std::path::PathBuf>,
    ) -> Image {
        let (nominal_px, scale_int, raster_px) = themed_icon_dims(size_pt, scale);
        let mut hash = blake3::Hasher::new();
        hash.update(&themed_icon_id(names, theme, nominal_px, scale_int, raster_px));
        hash.update(b"\0dirs\0");
        for dir in &data_dirs { hash.update(dir.as_os_str().as_encoded_bytes()); hash.update(&[0]); }
        self.request(*hash.finalize().as_bytes(), Input::ThemedIcon(Box::new(ThemedIconRequest {
            names: names.iter().map(|s| (*s).to_owned()).collect(),
            theme: theme.map(str::to_owned),
            nominal_px, scale: scale_int, raster_px, data_dirs: Some(data_dirs),
        })))
    }
}

pub(crate) fn decode_themed_request(req: ThemedIconRequest) -> Result<super::Decoded, super::DecodeFailure> {
    #[cfg(all(test, not(target_arch = "wasm32")))]
    if let Some(dirs) = req.data_dirs {
        return decode_themed_icon_in(&req.names, req.theme.as_deref(), req.nominal_px, req.scale, req.raster_px, &dirs, None);
    }
    #[cfg(target_os = "linux")]
    {
        let dirs = data_dirs();
        let home = home_dir();
        decode_themed_icon_in(&req.names, req.theme.as_deref(), req.nominal_px, req.scale, req.raster_px, &dirs, home.as_deref())
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = req;
        Err(super::DecodeFailure::not_found("themed icons are only supported on Linux"))
    }
}

pub(crate) fn themed_icon_dims(size_pt: f32, scale: f32) -> (u32, u32, u32) {
    let nominal = size_pt.ceil();
    let nominal_px = if nominal.is_finite() { nominal.clamp(1.0, 1024.0) as u32 } else { 1 };
    let s = scale.ceil();
    let scale_int = if s.is_finite() { s.clamp(1.0, 8.0) as u32 } else { 1 };
    (nominal_px, scale_int, icon_pixels(size_pt, scale))
}

fn themed_icon_id(names: &[&str], theme: Option<&str>, nominal_px: u32, scale_int: u32, raster_px: u32) -> super::ImageId {
    let mut hash = blake3::Hasher::new();
    hash.update(b"rune-themed-icon\0");
    hash.update(&(names.len() as u64).to_le_bytes());
    for name in names { hash.update(name.as_bytes()); hash.update(&[0]); }
    match theme {
        Some(t) => { hash.update(&[1]); hash.update(t.as_bytes()); hash.update(&[0]); }
        None => { hash.update(&[0]); }
    }
    hash.update(&nominal_px.to_le_bytes());
    hash.update(&scale_int.to_le_bytes());
    hash.update(&raster_px.to_le_bytes());
    *hash.finalize().as_bytes()
}

#[cfg(any(target_os = "linux", all(test, not(target_arch = "wasm32"))))]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) use spec::*;

#[cfg(any(target_os = "linux", all(test, not(target_arch = "wasm32"))))]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod spec {
    use std::collections::{HashMap, HashSet, VecDeque};
    use std::io::Read;
    use std::path::{Component, Path, PathBuf};
    use std::sync::{Arc, Mutex, OnceLock};

    use crate::{DecodeFailure, Decoded, ImageError, has_svg_extension, is_svg, mip_chain, mip_chain_with_intrinsic, svg_rgba_with_size};

    /// Maximum number of parsed icon theme indices kept in the process cache.
    pub(crate) const MAX_CACHED_THEMES: usize = 64;
    /// Maximum approximate memory used by cached theme indices (4 MiB).
    pub(crate) const MAX_CACHE_BYTES: usize = 4 * 1024 * 1024;
    const MAX_CHAIN_LEN: usize = 64;
    const MAX_TEXT_FILE_BYTES: u64 = 2 * 1024 * 1024;
    const MAX_ICON_FILE_BYTES: u64 = 16 * 1024 * 1024;
    const MAX_ICON_SIDE: u32 = 1024;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(crate) enum DirType { Fixed, Scalable, Threshold }

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub(crate) struct ThemeDir {
        pub rel_path: String,
        pub size: u32,
        pub scale: u32,
        pub dir_type: DirType,
        pub min_size: u32,
        pub max_size: u32,
        pub threshold: u32,
        pub context: Option<String>,
    }

    impl ThemeDir {
        /// Freedesktop Icon Theme Specification `DirectoryMatchesSize`.
        pub(crate) fn matches_size(&self, size: u32, scale: u32) -> bool {
            if self.scale != scale { return false; }
            match self.dir_type {
                DirType::Fixed => self.size == size,
                DirType::Scalable => self.min_size <= size && size <= self.max_size,
                DirType::Threshold => size >= self.size.saturating_sub(self.threshold) && size <= self.size.saturating_add(self.threshold),
            }
        }

        /// Freedesktop Icon Theme Specification `DirectorySizeDistance`.
        pub(crate) fn size_distance(&self, size: u32, scale: u32) -> u32 {
            let target = size.saturating_mul(scale);
            let range_dist = |lo: u32, hi: u32| if target < lo { lo - target } else if target > hi { target - hi } else { 0 };
            match self.dir_type {
                DirType::Fixed => self.size.saturating_mul(self.scale).abs_diff(target),
                DirType::Scalable => range_dist(self.min_size.saturating_mul(self.scale), self.max_size.saturating_mul(self.scale)),
                DirType::Threshold => range_dist(self.size.saturating_sub(self.threshold).saturating_mul(self.scale), self.size.saturating_add(self.threshold).saturating_mul(self.scale)),
            }
        }

        /// Whether this directory's largest effective size covers `size * scale` without upscaling.
        pub(crate) fn covers(&self, size: u32, scale: u32) -> bool {
            let upper = match self.dir_type {
                DirType::Fixed => self.size,
                DirType::Scalable => self.max_size,
                DirType::Threshold => self.size.saturating_add(self.threshold),
            };
            upper.saturating_mul(self.scale) >= size.saturating_mul(scale)
        }

        fn approx_bytes(&self) -> usize {
            std::mem::size_of::<Self>() + self.rel_path.capacity() + self.context.as_ref().map_or(0, String::capacity)
        }
    }

    #[derive(Clone, Debug, Default)]
    pub(crate) struct ThemeIndex {
        pub inherits: Vec<String>,
        pub directories: Vec<ThemeDir>,
        pub theme_roots: Vec<PathBuf>,
        pub approx_bytes: usize,
    }

    impl ThemeIndex {
        fn recompute_bytes(&mut self) {
            let inh: usize = self.inherits.iter().map(|s| std::mem::size_of::<String>() + s.capacity()).sum();
            let dirs: usize = self.directories.iter().map(ThemeDir::approx_bytes).sum();
            let roots: usize = self.theme_roots.iter().map(|p| std::mem::size_of::<PathBuf>() + p.as_os_str().len()).sum();
            self.approx_bytes = std::mem::size_of::<Self>() + inh + dirs + roots;
        }
    }

    #[derive(Clone, Debug, PartialEq, Eq, Hash)]
    struct ThemeCacheKey { theme: String, icon_dirs: Vec<PathBuf> }
    impl ThemeCacheKey {
        fn approx_bytes(&self) -> usize {
            let dirs: usize = self.icon_dirs.iter().map(|p| std::mem::size_of::<PathBuf>() + p.as_os_str().len()).sum();
            std::mem::size_of::<Self>() + self.theme.capacity() + dirs
        }
    }

    struct CacheEntry { index: Arc<ThemeIndex>, last_used: u64, bytes: usize }
    struct CacheInner { entries: HashMap<ThemeCacheKey, CacheEntry>, total_bytes: usize, tick: u64, max_entries: usize, max_bytes: usize }

    /// Bounded LRU cache of parsed icon-theme indices (capped in both entry count and bytes).
    /// Filesystem I/O always runs outside the mutex.
    pub(crate) struct ThemeCache { inner: Mutex<CacheInner> }

    impl ThemeCache {
        pub(crate) fn new(max_entries: usize, max_bytes: usize) -> Self {
            Self { inner: Mutex::new(CacheInner { entries: HashMap::new(), total_bytes: 0, tick: 0, max_entries, max_bytes }) }
        }

        pub(crate) fn get_or_load(&self, theme: &str, icon_dirs: &[PathBuf]) -> Arc<ThemeIndex> {
            let key = ThemeCacheKey { theme: theme.to_owned(), icon_dirs: icon_dirs.to_vec() };
            {
                let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
                inner.tick = inner.tick.wrapping_add(1);
                let tick = inner.tick;
                if let Some(entry) = inner.entries.get_mut(&key) {
                    entry.last_used = tick;
                    return entry.index.clone();
                }
            }
            let loaded = Arc::new(load_theme_index(theme, icon_dirs));
            let entry_bytes = key.approx_bytes().saturating_add(loaded.approx_bytes);
            let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            inner.tick = inner.tick.wrapping_add(1);
            let tick = inner.tick;
            if let Some(existing) = inner.entries.get_mut(&key) {
                existing.last_used = tick;
                return existing.index.clone();
            }
            if inner.max_entries == 0 || entry_bytes > inner.max_bytes { return loaded; }
            while inner.entries.len() >= inner.max_entries || inner.total_bytes.saturating_add(entry_bytes) > inner.max_bytes {
                let Some(evict_key) = inner.entries.iter().min_by_key(|(_, e)| e.last_used).map(|(k, _)| k.clone()) else { break };
                if let Some(removed) = inner.entries.remove(&evict_key) {
                    inner.total_bytes = inner.total_bytes.saturating_sub(removed.bytes);
                }
            }
            inner.total_bytes = inner.total_bytes.saturating_add(entry_bytes);
            inner.entries.insert(key, CacheEntry { index: loaded.clone(), last_used: tick, bytes: entry_bytes });
            loaded
        }

        #[cfg(test)]
        pub(crate) fn len(&self) -> usize { self.inner.lock().unwrap_or_else(|e| e.into_inner()).entries.len() }
        #[cfg(test)]
        pub(crate) fn total_bytes(&self) -> usize { self.inner.lock().unwrap_or_else(|e| e.into_inner()).total_bytes }
        #[cfg(test)]
        pub(crate) fn contains(&self, theme: &str, icon_dirs: &[PathBuf]) -> bool {
            let key = ThemeCacheKey { theme: theme.to_owned(), icon_dirs: icon_dirs.to_vec() };
            self.inner.lock().unwrap_or_else(|e| e.into_inner()).entries.contains_key(&key)
        }
    }

    static THEME_CACHE: OnceLock<ThemeCache> = OnceLock::new();
    pub(crate) fn global_cache() -> &'static ThemeCache { THEME_CACHE.get_or_init(|| ThemeCache::new(MAX_CACHED_THEMES, MAX_CACHE_BYTES)) }

    pub(crate) fn home_dir() -> Option<PathBuf> { std::env::var_os("HOME").filter(|v| !v.is_empty()).map(PathBuf::from) }

    pub(crate) fn data_dirs() -> Vec<PathBuf> {
        let home = std::env::var_os("XDG_DATA_HOME").filter(|v| !v.is_empty()).map(PathBuf::from).filter(|p| p.is_absolute())
            .or_else(|| home_dir().map(|h| h.join(".local/share")));
        let system = rune_core::env::var("XDG_DATA_DIRS").filter(|d| !d.is_empty()).unwrap_or_else(|| "/usr/local/share:/usr/share".into());
        home.into_iter().chain(system.split(':').filter(|d| !d.is_empty()).map(PathBuf::from)).collect()
    }

    /// Icon search directories per spec: `$HOME/.icons` followed by `<data_dir>/icons`.
    pub(crate) fn icon_dirs_from(data_dirs: &[PathBuf], home: Option<&Path>) -> Vec<PathBuf> {
        home.map(|h| h.join(".icons")).into_iter().chain(data_dirs.iter().map(|d| d.join("icons"))).collect()
    }

    /// Pixmaps fallback directories: `<data_dir>/pixmaps`.
    pub(crate) fn pixmap_dirs_from(data_dirs: &[PathBuf]) -> Vec<PathBuf> {
        data_dirs.iter().map(|d| d.join("pixmaps")).collect()
    }

    /// Detects the active Linux desktop icon theme from user GTK / KDE config files.
    pub(crate) fn detect_desktop_theme() -> Option<String> {
        let config_home = std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()).map(PathBuf::from).filter(|p| p.is_absolute());
        let home = home_dir();
        let desktop = rune_core::env::var("XDG_CURRENT_DESKTOP");
        detect_desktop_theme_from(config_home.as_deref(), home.as_deref(), desktop.as_deref())
    }

    /// Testable core of desktop theme detection with injected config paths and desktop name.
    pub(crate) fn detect_desktop_theme_from(config_home: Option<&Path>, home: Option<&Path>, current_desktop: Option<&str>) -> Option<String> {
        let config_dir = config_home.map(PathBuf::from).or_else(|| home.map(|h| h.join(".config")));
        let prefer_kde = current_desktop.is_some_and(|d| d.split(':').any(|p| p.trim().eq_ignore_ascii_case("KDE")));
        let read_kde = || parse_ini_key(&read_bounded_text(&config_dir.as_ref()?.join("kdeglobals"))?, Some("Icons"), "Theme");
        let read_gtk = || {
            if let Some(cfg) = config_dir.as_ref() {
                for rel in ["gtk-4.0/settings.ini", "gtk-3.0/settings.ini"] {
                    if let Some(t) = read_bounded_text(&cfg.join(rel)).and_then(|s| parse_ini_key(&s, Some("Settings"), "gtk-icon-theme-name")) {
                        return Some(t);
                    }
                }
            }
            parse_ini_key(&read_bounded_text(&home?.join(".gtkrc-2.0"))?, None, "gtk-icon-theme-name")
        };
        if prefer_kde { read_kde().or_else(read_gtk) } else { read_gtk().or_else(read_kde) }
    }

    fn parse_ini_key(text: &str, wanted_group: Option<&str>, wanted_key: &str) -> Option<String> {
        let mut group = "";
        for line in text.lines().map(str::trim) {
            if line.is_empty() || line.starts_with('#') || line.starts_with(';') { continue; }
            if line.starts_with('[') && line.ends_with(']') { group = &line[1..line.len() - 1]; continue; }
            if wanted_group.is_some_and(|g| group != g) { continue; }
            if let Some((k, v)) = line.split_once('=') && k.trim() == wanted_key {
                let v = v.trim();
                let v = if v.len() >= 2 && ((v.starts_with('"') && v.ends_with('"')) || (v.starts_with('\'') && v.ends_with('\''))) { &v[1..v.len() - 1] } else { v }.trim();
                if is_safe_theme_name(v) { return Some(v.to_owned()); }
            }
        }
        None
    }

    fn is_safe_theme_name(name: &str) -> bool {
        !name.is_empty() && !name.contains('/') && !name.contains('\\') && !name.contains('\0') && name != "." && name != ".."
    }

    fn is_safe_rel_subdir(rel: &str) -> bool {
        if rel.is_empty() || rel.contains('\0') || rel.contains('\\') { return false; }
        let path = Path::new(rel);
        if path.is_absolute() { return false; }
        let mut has_normal = false;
        for comp in path.components() {
            match comp { Component::Normal(_) => has_normal = true, _ => return false }
        }
        has_normal
    }

    /// Parses a Freedesktop `index.theme` document into theme metadata.
    pub(crate) fn parse_index_theme(text: &str) -> ThemeIndex {
        let mut current_group: Option<&str> = None;
        let mut inherits = Vec::new();
        let mut dir_order = Vec::new();
        let mut dir_seen = HashSet::new();
        #[derive(Default)]
        struct RawDir { size: Option<u32>, scale: Option<u32>, dir_type: Option<DirType>, min_size: Option<u32>, max_size: Option<u32>, threshold: Option<u32>, context: Option<String> }
        let mut raw_dirs: HashMap<&str, RawDir> = HashMap::new();

        for line in text.lines().map(str::trim) {
            if line.is_empty() || line.starts_with('#') { continue; }
            if line.starts_with('[') && line.ends_with(']') { current_group = Some(&line[1..line.len() - 1]); continue; }
            let Some(group) = current_group else { continue };
            let Some((key, val)) = line.split_once('=') else { continue };
            let (key, val) = (key.trim(), val.trim());
            if group == "Icon Theme" {
                match key {
                    "Inherits" => inherits.extend(val.split(',').map(str::trim).filter(|s| is_safe_theme_name(s)).map(str::to_owned)),
                    "Directories" | "ScaledDirectories" => {
                        for item in val.split(',').map(|s| s.trim().trim_end_matches('/')) {
                            if is_safe_rel_subdir(item) && dir_seen.insert(item) { dir_order.push(item); }
                        }
                    }
                    _ => {}
                }
            } else {
                let entry = raw_dirs.entry(group).or_default();
                match key {
                    "Size" => entry.size = val.parse::<u32>().ok().filter(|&n| n > 0),
                    "Scale" => entry.scale = val.parse::<u32>().ok().filter(|&n| n > 0),
                    "Type" => entry.dir_type = match val { "Fixed" => Some(DirType::Fixed), "Scalable" => Some(DirType::Scalable), "Threshold" => Some(DirType::Threshold), _ => None },
                    "MinSize" => entry.min_size = val.parse::<u32>().ok().filter(|&n| n > 0),
                    "MaxSize" => entry.max_size = val.parse::<u32>().ok().filter(|&n| n > 0),
                    "Threshold" => entry.threshold = val.parse::<u32>().ok(),
                    "Context" if !val.is_empty() => entry.context = Some(val.to_owned()),
                    _ => {}
                }
            }
        }

        let mut directories = Vec::with_capacity(dir_order.len());
        for rel in dir_order {
            let Some(raw) = raw_dirs.remove(rel) else { continue };
            let Some(size) = raw.size else { continue };
            let min_size = raw.min_size.unwrap_or(size);
            directories.push(ThemeDir {
                rel_path: rel.to_owned(), size, scale: raw.scale.unwrap_or(1).max(1),
                dir_type: raw.dir_type.unwrap_or(DirType::Threshold), min_size,
                max_size: raw.max_size.unwrap_or(size).max(min_size), threshold: raw.threshold.unwrap_or(2), context: raw.context,
            });
        }
        let mut index = ThemeIndex { inherits, directories, theme_roots: Vec::new(), approx_bytes: 0 };
        index.recompute_bytes();
        index
    }

    fn load_theme_index(theme: &str, icon_dirs: &[PathBuf]) -> ThemeIndex {
        if !is_safe_theme_name(theme) { return ThemeIndex::default(); }
        let theme_roots: Vec<PathBuf> = icon_dirs.iter().map(|d| d.join(theme)).filter(|p| p.is_dir()).collect();
        if theme_roots.is_empty() { return ThemeIndex::default(); }
        for root in &theme_roots {
            if let Some(text) = read_bounded_text(&root.join("index.theme")) {
                let mut index = parse_index_theme(&text);
                index.theme_roots = theme_roots;
                index.recompute_bytes();
                return index;
            }
        }
        synthesize_theme_index(theme_roots)
    }

    /// Fallback when a theme directory (such as a minimal `hicolor` fixture) has no `index.theme`.
    fn synthesize_theme_index(theme_roots: Vec<PathBuf>) -> ThemeIndex {
        let mut seen = HashSet::new();
        let mut dirs = Vec::new();
        for root in &theme_roots {
            let Ok(entries) = std::fs::read_dir(root) else { continue };
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_dir() { continue; }
                let label = entry.file_name().to_string_lossy().into_owned();
                let Some((dir_type, size, scale, min_size, max_size)) = parse_dir_label(&label) else { continue };
                let mut push = |rel: String| {
                    if is_safe_rel_subdir(&rel) && seen.insert(rel.clone()) {
                        dirs.push(ThemeDir { rel_path: rel, size, scale, dir_type, min_size, max_size, threshold: 0, context: None });
                    }
                };
                let mut found_sub = false;
                if let Ok(subs) = std::fs::read_dir(&path) {
                    for sub in subs.flatten().filter(|s| s.path().is_dir()) {
                        found_sub = true;
                        push(format!("{label}/{}", sub.file_name().to_string_lossy()));
                    }
                }
                if !found_sub { push(label); }
            }
        }
        dirs.sort_by(|a, b| {
            let rank = |t: DirType| match t { DirType::Fixed | DirType::Threshold => 0u8, DirType::Scalable => 1u8 };
            (rank(a.dir_type), a.size.saturating_mul(a.scale), &a.rel_path).cmp(&(rank(b.dir_type), b.size.saturating_mul(b.scale), &b.rel_path))
        });
        let mut index = ThemeIndex { inherits: Vec::new(), directories: dirs, theme_roots, approx_bytes: 0 };
        index.recompute_bytes();
        index
    }

    fn parse_dir_label(label: &str) -> Option<(DirType, u32, u32, u32, u32)> {
        if label == "scalable" { return Some((DirType::Scalable, 128, 1, 128, 1024)); }
        let (dims, scale_str) = label.split_once('@').unwrap_or((label, "1"));
        let scale = scale_str.parse::<u32>().ok().filter(|&s| s > 0)?;
        let (w_str, h_str) = dims.split_once('x')?;
        let w = w_str.parse::<u32>().ok().filter(|&n| n > 0)?;
        let _h = h_str.parse::<u32>().ok().filter(|&n| n > 0)?;
        Some((DirType::Fixed, w, scale, w, w))
    }

    /// Builds the breadth-first `Inherits` chain starting at `start_theme`, ending at `hicolor`
    /// once, with cycle protection and a hard chain-length cap.
    pub(crate) fn theme_chain(start_theme: &str, icon_dirs: &[PathBuf], cache: &ThemeCache) -> Vec<(String, Arc<ThemeIndex>)> {
        let mut queue = VecDeque::new();
        let mut visited = HashSet::new();
        let mut order = Vec::new();
        let defer_hicolor = !start_theme.eq_ignore_ascii_case("hicolor");
        if is_safe_theme_name(start_theme) {
            queue.push_back(start_theme.to_owned());
            visited.insert(start_theme.to_owned());
        }
        while let Some(name) = queue.pop_front() {
            if order.len() >= MAX_CHAIN_LEN { break; }
            let index = cache.get_or_load(&name, icon_dirs);
            for parent in &index.inherits {
                if defer_hicolor && parent == "hicolor" { continue; }
                if visited.insert(parent.clone()) { queue.push_back(parent.clone()); }
            }
            order.push((name, index));
        }
        if defer_hicolor && order.len() < MAX_CHAIN_LEN && visited.insert("hicolor".to_owned()) {
            order.push(("hicolor".to_owned(), cache.get_or_load("hicolor", icon_dirs)));
        }
        order
    }

    fn lookup_in_theme(index: &ThemeIndex, stem: &str, size: u32, scale: u32) -> Option<PathBuf> {
        if index.theme_roots.is_empty() || index.directories.is_empty() { return None; }
        let check_dir = |dir: &ThemeDir| -> Option<PathBuf> {
            for root in &index.theme_roots {
                let base = root.join(&dir.rel_path);
                for ext in ["png", "svg"] {
                    let candidate = base.join(format!("{stem}.{ext}"));
                    if candidate.is_file() { return Some(candidate); }
                }
            }
            None
        };
        for dir in &index.directories {
            if dir.matches_size(size, scale) && let Some(found) = check_dir(dir) { return Some(found); }
        }
        let mut ranked: Vec<usize> = (0..index.directories.len()).collect();
        ranked.sort_by_key(|&i| {
            let d = &index.directories[i];
            (d.size_distance(size, scale), !d.covers(size, scale), d.scale != scale, i)
        });
        ranked.into_iter().find_map(|i| check_dir(&index.directories[i]))
    }

    fn strip_icon_extension(name: &str) -> &str {
        for ext in [".png", ".svg", ".xpm"] {
            if name.len() > ext.len() && name[name.len() - ext.len()..].eq_ignore_ascii_case(ext) {
                return &name[..name.len() - ext.len()];
            }
        }
        name
    }

    /// Resolves the first matching icon file from `names` using the Freedesktop Icon Theme
    /// Specification across `icon_dirs` and `pixmap_dirs`.
    pub(crate) fn find_icon_in_dirs(
        names: &[&str], theme: Option<&str>, size: u32, scale: u32, icon_dirs: &[PathBuf], pixmap_dirs: &[PathBuf], cache: &ThemeCache,
    ) -> Option<PathBuf> {
        let detected = if theme.is_none() { detect_desktop_theme() } else { None };
        let start = theme.map(str::trim).filter(|s| !s.is_empty()).or(detected.as_deref()).unwrap_or("Adwaita");
        let chain = theme_chain(start, icon_dirs, cache);
        let scale = scale.max(1);
        for raw in names {
            let raw = raw.trim();
            if raw.is_empty() { continue; }
            let as_path = Path::new(raw);
            if as_path.is_absolute() {
                if as_path.is_file() { return Some(as_path.to_path_buf()); }
                continue;
            }
            let stem = strip_icon_extension(raw);
            if !is_safe_theme_name(stem) { continue; }
            for (_, index) in &chain {
                if let Some(found) = lookup_in_theme(index, stem, size, scale) { return Some(found); }
            }
            for dir in pixmap_dirs {
                for ext in ["png", "svg"] {
                    let candidate = dir.join(format!("{stem}.{ext}"));
                    if candidate.is_file() { return Some(candidate); }
                }
            }
        }
        None
    }

    /// The `Icon=` key of the `[Desktop Entry]` group.
    pub(crate) fn icon_name(entry: &str) -> Option<&str> {
        let mut group = "";
        for line in entry.lines().map(str::trim) {
            if line.starts_with('[') { group = line; continue; }
            if group != "[Desktop Entry]" { continue; }
            if let Some((key, value)) = line.split_once('=') && key.trim() == "Icon" {
                return Some(value.trim()).filter(|v| !v.is_empty());
            }
        }
        None
    }

    /// Resolves a `.desktop` entry's `Icon=` in `data_dirs` (uses `hicolor` when no theme is given).
    pub(crate) fn icon_file(entry: &Path, px: u32, data_dirs: &[PathBuf]) -> Option<PathBuf> {
        icon_file_scaled(entry, px, 1, Some("hicolor"), data_dirs, None)
    }

    /// Resolves a `.desktop` entry's `Icon=` through the full theme chain in `data_dirs`.
    pub(crate) fn icon_file_scaled(
        entry: &Path, nominal_px: u32, scale: u32, theme: Option<&str>, data_dirs: &[PathBuf], home: Option<&Path>,
    ) -> Option<PathBuf> {
        let text = read_bounded_text(entry)?;
        let name = icon_name(&text)?;
        let icon_dirs = icon_dirs_from(data_dirs, home);
        let pixmap_dirs = pixmap_dirs_from(data_dirs);
        find_icon_in_dirs(&[name], theme, nominal_px, scale, &icon_dirs, &pixmap_dirs, global_cache())
    }

    /// Decodes a resolved icon file (PNG/raster or SVG at `raster_px`), distinguishing missing
    /// files (`not_found: true`) from corrupt/unreadable/oversized files (`not_found: false`).
    pub(crate) fn decode_icon_file(file: &Path, raster_px: u32) -> Result<(u32, u32, Option<[f32; 2]>, Vec<u8>), DecodeFailure> {
        let bytes = read_bounded_bytes(file, MAX_ICON_FILE_BYTES)?;
        if has_svg_extension(file) || is_svg(&bytes) {
            let (w, h, intrinsic, rgba) = svg_rgba_with_size(&bytes, raster_px).map_err(DecodeFailure::from)?;
            return Ok((w, h, Some(intrinsic), rgba));
        }
        let error = |e: image::ImageError| DecodeFailure::from(ImageError(format!("{}: {e}", file.display())));
        let mut reader = image::ImageReader::new(std::io::Cursor::new(&bytes))
            .with_guessed_format().map_err(|e| DecodeFailure::from(ImageError(format!("{}: {e}", file.display()))))?;
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(MAX_ICON_SIDE);
        limits.max_image_height = Some(MAX_ICON_SIDE);
        limits.max_alloc = Some(MAX_ICON_FILE_BYTES);
        reader.limits(limits);
        let img = reader.decode().map_err(error)?.into_rgba8();
        if img.width() > MAX_ICON_SIDE || img.height() > MAX_ICON_SIDE {
            return Err(DecodeFailure::from(ImageError(format!("{} is too large for an icon", file.display()))));
        }
        Ok((img.width(), img.height(), None, img.into_raw()))
    }

    pub(crate) fn themed_icon_rgba_in(
        names: &[&str], theme: Option<&str>, px: u32, scale: u32, data_dirs: &[PathBuf], home: Option<&Path>,
    ) -> Result<Option<(u32, u32, Vec<u8>)>, ImageError> {
        if px == 0 || px > 1024 { return Err(ImageError("themed icon size must be 1..=1024 pixels".into())); }
        if scale == 0 { return Err(ImageError("themed icon scale must be at least 1".into())); }
        let icon_dirs = icon_dirs_from(data_dirs, home);
        let pixmap_dirs = pixmap_dirs_from(data_dirs);
        let Some(file) = find_icon_in_dirs(names, theme, px, scale, &icon_dirs, &pixmap_dirs, global_cache()) else { return Ok(None); };
        let (w, h, _, rgba) = decode_icon_file(&file, px.saturating_mul(scale).clamp(1, 1024)).map_err(|f| f.error)?;
        Ok(Some((w, h, rgba)))
    }

    pub(crate) fn decode_themed_icon_in(
        names: &[String], theme: Option<&str>, nominal_px: u32, scale: u32, raster_px: u32, data_dirs: &[PathBuf], home: Option<&Path>,
    ) -> Result<Decoded, DecodeFailure> {
        let name_refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let icon_dirs = icon_dirs_from(data_dirs, home);
        let pixmap_dirs = pixmap_dirs_from(data_dirs);
        let Some(file) = find_icon_in_dirs(&name_refs, theme, nominal_px, scale, &icon_dirs, &pixmap_dirs, global_cache()) else {
            let label = if name_refs.is_empty() { "<empty>".to_owned() } else { name_refs.join(", ") };
            return Err(DecodeFailure::not_found(format!("no themed icon for {label}")));
        };
        let (w, h, intrinsic, rgba) = decode_icon_file(&file, raster_px)?;
        match intrinsic {
            Some(size) => mip_chain_with_intrinsic(w, h, &rgba, Some(size)).map_err(DecodeFailure::from),
            None => mip_chain(w, h, &rgba).map_err(DecodeFailure::from),
        }
    }

    fn read_bounded_bytes(path: &Path, max_bytes: u64) -> Result<Vec<u8>, DecodeFailure> {
        let map_io = |e: std::io::Error| {
            let msg = format!("{}: {e}", path.display());
            if e.kind() == std::io::ErrorKind::NotFound { DecodeFailure::not_found(msg) } else { DecodeFailure::from(ImageError(msg)) }
        };
        let meta = std::fs::metadata(path).map_err(map_io)?;
        if !meta.is_file() { return Err(DecodeFailure::from(ImageError(format!("{}: not a regular file", path.display())))); }
        if meta.len() > max_bytes { return Err(DecodeFailure::from(ImageError(format!("{} is too large for an icon", path.display())))); }
        let file = std::fs::File::open(path).map_err(map_io)?;
        let handle_meta = file.metadata().map_err(map_io)?;
        if !handle_meta.is_file() { return Err(DecodeFailure::from(ImageError(format!("{}: not a regular file", path.display())))); }
        let cap = handle_meta.len().min(max_bytes) as usize;
        let mut out = Vec::with_capacity(cap.min(64 * 1024));
        file.take(max_bytes + 1).read_to_end(&mut out).map_err(map_io)?;
        if out.len() as u64 > max_bytes { return Err(DecodeFailure::from(ImageError(format!("{} is too large for an icon", path.display())))); }
        Ok(out)
    }

    fn read_bounded_text(path: &Path) -> Option<String> { String::from_utf8(read_bounded_bytes(path, MAX_TEXT_FILE_BYTES).ok()?).ok() }
}
