use super::*;

struct Dir(PathBuf);
impl Dir {
    fn new(name: &str) -> Dir {
        let dir = std::env::temp_dir().join(format!("rune-prefs-{name}-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        Dir(dir)
    }
}
impl Drop for Dir { fn drop(&mut self) { std::fs::remove_dir_all(&self.0).ok(); } }

#[test]
fn values_round_trip_through_the_file() {
    let dir = Dir::new("roundtrip");
    let path = dir.0.join("app").join(FILE);
    let mut prefs = Prefs::at(&path);
    assert_eq!(prefs.get::<f32>("text_size"), None);
    prefs.set("text_size", &15.5f32).unwrap();
    prefs.set("window", &(800u32, 600u32)).unwrap();
    prefs.set("recent", &["a", "b"]).unwrap();
    let again = Prefs::at(&path);
    assert_eq!(again.get::<f32>("text_size"), Some(15.5));
    assert_eq!(again.get("window"), Some((800u32, 600u32)));
    assert_eq!(again.get::<Vec<String>>("recent"), Some(vec!["a".into(), "b".into()]));
    assert_eq!(again.get::<String>("text_size"), None, "a value of another type reads as unset");
    let mut prefs = again;
    prefs.remove("text_size").unwrap();
    assert!(!Prefs::at(&path).contains("text_size"));
    assert!(!path.with_file_name("prefs.json.tmp").exists(), "no temporary file is left behind");
}

#[test]
fn a_corrupt_file_starts_empty_and_is_kept_as_bad() {
    let dir = Dir::new("corrupt");
    std::fs::create_dir_all(&dir.0).unwrap();
    let path = dir.0.join(FILE);
    std::fs::write(&path, b"{ not json").unwrap();
    let mut prefs = Prefs::at(&path);
    assert!(!prefs.contains("anything"));
    assert_eq!(std::fs::read(dir.0.join("prefs.json.bad")).unwrap(), b"{ not json");
    prefs.set("k", &1u8).unwrap();
    assert_eq!(Prefs::at(&path).get::<u8>("k"), Some(1));

    std::fs::write(&path, b"[1, 2]").unwrap();
    assert!(!Prefs::at(&path).contains("k"), "a file that isn't an object is corrupt too");
    assert_eq!(std::fs::read(dir.0.join("prefs.json.bad")).unwrap(), b"[1, 2]");
}

#[test]
fn memory_stores_touch_no_file() {
    let mut prefs = Prefs::memory();
    prefs.set("k", "v").unwrap();
    assert_eq!(prefs.get::<String>("k").as_deref(), Some("v"));
    assert!(!prefs.is_persistent() && prefs.path().is_none());
}

#[test]
fn app_ids_stay_inside_the_config_directory() {
    assert_eq!(sanitize("com.example.notes"), "com.example.notes");
    assert_eq!(sanitize("../evil/id"), ".._evil_id");
    assert_eq!(sanitize(".."), "app");
    assert_eq!(sanitize("..."), "app");
    assert_eq!(sanitize("com.example."), "com.example");
    assert_eq!(sanitize(""), "app");
}

#[test]
fn concurrent_writers_do_not_clobber_temp_files() {
    let dir = Dir::new("concurrent");
    let path = dir.0.join(FILE);
    std::thread::scope(|s| {
        for t in 0..4u32 {
            let path = &path;
            s.spawn(move || {
                let mut prefs = Prefs::at(path);
                for i in 0..20u32 {
                    prefs.set(&format!("k_{t}"), &(t * 100 + i)).unwrap();
                }
            });
        }
    });
    let final_prefs = Prefs::at(&path);
    assert!(final_prefs.is_persistent());
    assert!(!dir.0.join("prefs.json.bad").exists());
}

#[test]
fn the_config_directory_follows_the_platform_and_the_override() {
    let env = |pairs: &'static [(&str, &str)]| move |k: &str| pairs.iter().find(|p| p.0 == k).map(|p| p.1.into());
    assert_eq!(config_dir(env(&[("RUNE_PREFS_DIR", "/scratch"), ("HOME", "/home/x")])), Some(PathBuf::from("/scratch")));
    assert_eq!(config_dir(env(&[])), None);
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    assert_eq!(config_dir(env(&[("HOME", "/h")])), Some(PathBuf::from("/h/Library/Application Support")));
    #[cfg(target_os = "windows")]
    assert_eq!(config_dir(env(&[("APPDATA", "C:\\Roaming")])), Some(PathBuf::from("C:\\Roaming")));
    #[cfg(all(unix, not(any(target_os = "macos", target_os = "ios"))))]
    {
        assert_eq!(config_dir(env(&[("HOME", "/h")])), Some(PathBuf::from("/h/.config")));
        assert_eq!(config_dir(env(&[("HOME", "/h"), ("XDG_CONFIG_HOME", "/xdg")])), Some(PathBuf::from("/xdg")));
        assert_eq!(config_dir(env(&[("HOME", "/h"), ("XDG_CONFIG_HOME", "rel")])), Some(PathBuf::from("/h/.config")));
    }
}
