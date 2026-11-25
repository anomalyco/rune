use super::{CredentialError, Credentials};

#[test]
fn memory_store_writes_reads_replaces_and_deletes() {
    let store = Credentials::memory();
    assert!(!store.is_persistent());
    assert_eq!(store.read("com.example.api"), Ok(None));

    store.write("com.example.api", "me", b"first").unwrap();
    let found = store.read("com.example.api").unwrap().unwrap();
    assert_eq!((found.account(), found.secret()), ("me", &b"first"[..]));

    store.write("com.example.api", "you", b"second").unwrap();
    let found = store.read("com.example.api").unwrap().unwrap();
    assert_eq!((found.account(), found.secret()), ("you", &b"second"[..]));

    store.delete("com.example.api").unwrap();
    assert_eq!(store.read("com.example.api"), Ok(None));
    store.delete("com.example.api").unwrap();
}

#[test]
fn clones_share_a_store_and_separate_stores_do_not() {
    let store = Credentials::memory();
    store.clone().write("svc", "a", b"x").unwrap();
    assert!(store.read("svc").unwrap().is_some());
    assert_eq!(Credentials::memory().read("svc"), Ok(None));
}

#[test]
fn refuses_names_a_store_cannot_hold() {
    let store = Credentials::memory();
    assert_eq!(store.write("", "a", b"x"), Err(CredentialError::Empty));
    assert_eq!(store.read(""), Err(CredentialError::Empty));
    assert_eq!(store.delete(""), Err(CredentialError::Empty));
    assert_eq!(store.write("s\0", "a", b"x"), Err(CredentialError::Invalid));
    assert_eq!(store.write("s", "a\0", b"x"), Err(CredentialError::Invalid));
}

#[test]
fn debug_never_shows_the_secret() {
    let store = Credentials::memory();
    store.write("svc", "me", b"hunter2").unwrap();
    let shown = format!("{:?}", store.read("svc").unwrap());
    assert!(shown.contains("me") && shown.contains("<redacted>") && !shown.contains("104, 117"), "{shown}");
}

#[test]
fn a_refused_write_keeps_the_old_credential() {
    let store = Credentials::memory();
    store.write("svc", "me", b"old").unwrap();
    assert_eq!(store.write("svc", "bad\0", b"new"), Err(CredentialError::Invalid));
    let found = store.read("svc").unwrap().unwrap();
    assert_eq!((found.account(), found.secret()), ("me", &b"old"[..]));
}

#[test]
fn windows_and_an_apps_own_runtimes_keep_the_system_store() {
    use crate::{AppSpec, Cx, El, Env, Runtime, el};
    fn view(_: &(), _: &mut Cx<()>) -> El { el().size(20.0, 20.0) }
    // A thread of its own: the flag is per thread in tests, and other tests make headless runs.
    let noted = std::thread::spawn(|| {
        let spec = AppSpec::new("credentials", (20.0, 20.0), || (), view);
        let _window = crate::tooling::runtime_for(&spec, (), spec.size, Env::new(true, 1.0), std::sync::Arc::new(|| {}));
        let _offscreen = Runtime::new((), view, (20.0, 20.0), Env::new(true, 1.0));
        super::headless_runtime_noted()
    });
    assert!(!noted.join().unwrap());
}

#[test]
fn an_apps_headless_runtime_keeps_the_system_store_in_memory() {
    use crate::{App, Cx, El, Env, el};
    fn view(_: &(), _: &mut Cx<()>) -> El { el().size(20.0, 20.0) }
    let spec = crate::AppSpec::new("credentials", (20.0, 20.0), || (), view);
    let _rt = App::new(spec).headless_runtime((20.0, 20.0), Env::new(true, 1.0));
    assert!(super::headless_runtime_noted());
}

#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
mod windows_unit_tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use windows_sys::Win32::Foundation::{ERROR_ACCESS_DENIED, ERROR_INVALID_PARAMETER};
    use windows_sys::Win32::Security::Credentials::{
        CRED_MAX_GENERIC_TARGET_NAME_LENGTH, CRED_MAX_USERNAME_LENGTH, CREDENTIALW,
    };

    use super::super::windows::{CredGuard, encode_wide, error_from_code};
    use super::CredentialError;

    static FREE_COUNT: AtomicUsize = AtomicUsize::new(0);

    /// # Safety
    /// Test callback that records the free without deallocating the stack-owned struct.
    unsafe fn record_free(_: *mut CREDENTIALW) {
        FREE_COUNT.fetch_add(1, Ordering::SeqCst);
    }

    #[test]
    fn encode_wide_enforces_utf16_limits_including_nul() {
        let max_user = CRED_MAX_USERNAME_LENGTH as usize;
        assert_eq!(encode_wide(&"a".repeat(max_user - 1), max_user).unwrap().len(), max_user);
        assert_eq!(encode_wide(&"a".repeat(max_user), max_user), Err(CredentialError::Invalid));

        // Surrogate pairs take 2 UTF-16 code units each: 256 pairs + NUL = 513 (exact max); 257 fails.
        let pairs_ok = "\u{1F511}".repeat(256);
        assert_eq!(encode_wide(&pairs_ok, max_user).unwrap().len(), max_user);
        let pairs_over = "\u{1F511}".repeat(257);
        assert_eq!(encode_wide(&pairs_over, max_user), Err(CredentialError::Invalid));

        let max_target = CRED_MAX_GENERIC_TARGET_NAME_LENGTH as usize;
        assert_eq!(encode_wide(&"t".repeat(max_target - 1), max_target).unwrap().len(), max_target);
        assert_eq!(encode_wide(&"t".repeat(max_target), max_target), Err(CredentialError::Invalid));
    }

    #[test]
    fn error_from_code_maps_access_denied_to_denied() {
        assert_eq!(error_from_code("CredReadW", ERROR_ACCESS_DENIED), CredentialError::Denied);
        assert_eq!(
            error_from_code("CredWriteW", ERROR_INVALID_PARAMETER),
            CredentialError::Failed(format!("CredWriteW failed ({ERROR_INVALID_PARAMETER})")),
        );
    }

    #[test]
    fn cred_guard_wipes_and_frees_on_drop_and_unwind() {
        let before = FREE_COUNT.load(Ordering::SeqCst);
        let mut blob = *b"top-secret-bytes";
        let mut cred = CREDENTIALW {
            CredentialBlobSize: blob.len() as u32,
            CredentialBlob: blob.as_mut_ptr(),
            ..CREDENTIALW::default()
        };
        {
            let _guard = CredGuard::new(&mut cred, record_free);
        }
        assert_eq!(blob, [0u8; 16]);
        assert_eq!(FREE_COUNT.load(Ordering::SeqCst), before + 1);

        // Must also wipe and free when unwinding from a panic.
        let mut unwind_blob = *b"unwind-secret-99";
        let mut unwind_cred = CREDENTIALW {
            CredentialBlobSize: unwind_blob.len() as u32,
            CredentialBlob: unwind_blob.as_mut_ptr(),
            ..CREDENTIALW::default()
        };
        let ptr = &mut unwind_cred as *mut CREDENTIALW;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = CredGuard::new(ptr, record_free);
            panic!("simulated allocation failure");
        }));
        assert!(result.is_err());
        assert_eq!(unwind_blob, [0u8; 16]);
        assert_eq!(FREE_COUNT.load(Ordering::SeqCst), before + 2);
    }
}

#[cfg(target_os = "windows")]
mod windows_live_tests {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};

    use super::super::windows;
    use super::CredentialError;

    struct Cleanup(Vec<String>);

    impl Cleanup {
        fn new() -> Self { Self(Vec::new()) }
        fn target(&mut self, tag: &str) -> String {
            let mut h = RandomState::new().build_hasher();
            h.write_u32(std::process::id());
            h.write(tag.as_bytes());
            let name = format!("rune-test-{tag}-{:016x}", h.finish());
            self.0.push(name.clone());
            name
        }
        fn register(&mut self, name: String) -> String {
            self.0.push(name.clone());
            name
        }
    }

    impl Drop for Cleanup {
        fn drop(&mut self) {
            for target in &self.0 {
                if let Err(err) = windows::delete(target) {
                    eprintln!("cleanup delete failed for {target}: {err}");
                }
            }
        }
    }

    /// Opt-in test against the real Windows Credential Manager. Never runs in the default test
    /// suite; invoke with `cargo test -p rune credentials::tests::windows_live_tests -- --ignored`.
    #[test]
    #[ignore = "writes to the real Windows Credential Manager; run explicitly with -- --ignored"]
    fn windows_credential_manager_round_trip_and_limits() {
        let mut cleanup = Cleanup::new();
        let target = cleanup.target("rt");

        assert_eq!(windows::read(&target), Ok(None));

        // Write, read back, overwrite.
        windows::write(&target, "alice", b"first-secret").unwrap();
        let found = windows::read(&target).unwrap().unwrap();
        assert_eq!((found.account(), found.secret()), ("alice", &b"first-secret"[..]));

        windows::write(&target, "bob", b"second-secret").unwrap();
        let found = windows::read(&target).unwrap().unwrap();
        assert_eq!((found.account(), found.secret()), ("bob", &b"second-secret"[..]));

        // Every byte value 0x00..=0xFF.
        let all_bytes: Vec<u8> = (0u8..=255u8).collect();
        windows::write(&target, "bytes", &all_bytes).unwrap();
        let found = windows::read(&target).unwrap().unwrap();
        assert_eq!(found.secret(), all_bytes.as_slice());

        // Empty secret.
        windows::write(&target, "empty", b"").unwrap();
        let found = windows::read(&target).unwrap().unwrap();
        assert_eq!((found.account(), found.secret()), ("empty", &b""[..]));

        // Secret at exact limit (2,560 bytes) and one past it (2,561 bytes).
        let max_secret = vec![0x5Au8; 2560];
        windows::write(&target, "max-blob", &max_secret).unwrap();
        assert_eq!(windows::read(&target).unwrap().unwrap().secret(), max_secret.as_slice());
        assert_eq!(windows::write(&target, "over-blob", &[0x5Au8; 2561]), Err(CredentialError::Invalid));
        assert_eq!(windows::read(&target).unwrap().unwrap().account(), "max-blob");

        // UserName at exact limit (512 UTF-16 units + NUL = 513) and one past it (513 units).
        let max_user = "u".repeat(512);
        windows::write(&target, &max_user, b"ok").unwrap();
        assert_eq!(windows::read(&target).unwrap().unwrap().account(), max_user);
        let over_user = "u".repeat(513);
        assert_eq!(windows::write(&target, &over_user, b"no"), Err(CredentialError::Invalid));
        assert_eq!(windows::read(&target).unwrap().unwrap().account(), max_user);

        // Delete and read after delete; deleting again is ok.
        windows::delete(&target).unwrap();
        assert_eq!(windows::read(&target), Ok(None));
        windows::delete(&target).unwrap();

        // TargetName at exact limit (32,766 UTF-16 units + NUL = 32,767) and one past it (32,767 units).
        let prefix = cleanup.target("maxt");
        let max_target = cleanup.register(format!("{prefix}{}", "t".repeat(32766 - prefix.len())));
        windows::write(&max_target, "a", b"ok").unwrap();
        assert_eq!(windows::read(&max_target).unwrap().unwrap().secret(), b"ok");
        windows::delete(&max_target).unwrap();
        assert_eq!(windows::read(&max_target), Ok(None));

        let over_target = format!("{prefix}{}", "t".repeat(32767 - prefix.len()));
        assert_eq!(windows::write(&over_target, "a", b"no"), Err(CredentialError::Invalid));
        assert_eq!(windows::read(&over_target), Err(CredentialError::Invalid));
        assert_eq!(windows::delete(&over_target), Err(CredentialError::Invalid));
    }
}

mod apple_seam_tests {
    use std::cell::RefCell;
    use std::collections::BTreeMap;
    use std::ffi::c_void;

    use super::super::apple::{
        CFDataRef, CFDictionaryRef, CFIndex, CFStringRef, CFTypeRef, Constants,
        ERR_SEC_AUTH_FAILED, ERR_SEC_DUPLICATE_ITEM, ERR_SEC_INTERACTION_NOT_ALLOWED,
        ERR_SEC_ITEM_NOT_FOUND, ERR_SEC_USER_CANCELED, KeychainOps, OSStatus, delete_with,
        read_with, write_with,
    };
    use super::CredentialError;

    const K_BOOL_TRUE: CFTypeRef = 1usize as CFTypeRef;
    const K_CLASS: CFStringRef = 2usize as CFStringRef;
    const K_CLASS_GENERIC: CFStringRef = 3usize as CFStringRef;
    const K_ATTR_SERVICE: CFStringRef = 4usize as CFStringRef;
    const K_ATTR_ACCOUNT: CFStringRef = 5usize as CFStringRef;
    const K_VALUE_DATA: CFStringRef = 6usize as CFStringRef;
    const K_RETURN_DATA: CFStringRef = 7usize as CFStringRef;
    const K_RETURN_ATTRS: CFStringRef = 8usize as CFStringRef;
    const K_MATCH_LIMIT: CFStringRef = 9usize as CFStringRef;
    const K_MATCH_LIMIT_ONE: CFStringRef = 10usize as CFStringRef;
    const K_MATCH_LIMIT_ALL: CFStringRef = 11usize as CFStringRef;
    const ERR_SEC_PARAM: OSStatus = -50;

    #[derive(Clone)]
    enum Obj {
        Str(String),
        Data(Vec<u8>),
        Dict(BTreeMap<usize, usize>),
    }

    #[derive(Default)]
    struct FakeState {
        next_handle: usize,
        objects: BTreeMap<usize, Obj>,
        items: Vec<(String, String, Vec<u8>)>,
        fail_string_on: Option<usize>,
        string_calls: usize,
        fail_data: bool,
        fail_dict_on: Option<usize>,
        dict_calls: usize,
        dict_received_null: bool,
        sec_calls: usize,
        override_data_len: Option<CFIndex>,
        copy_bytes_called: bool,
        forced_status: Option<OSStatus>,
    }

    struct FakeKeychain {
        macos_mode: bool,
        state: RefCell<FakeState>,
    }

    impl FakeKeychain {
        fn new(macos_mode: bool) -> Self {
            Self {
                macos_mode,
                state: RefCell::new(FakeState { next_handle: 100, ..FakeState::default() }),
            }
        }

        fn alloc(&self, obj: Obj) -> CFTypeRef {
            let mut st = self.state.borrow_mut();
            let id = st.next_handle;
            st.next_handle += 1;
            st.objects.insert(id, obj);
            id as *const c_void
        }

        fn live_refs(&self) -> usize {
            self.state.borrow().objects.len()
        }

        fn get_str(&self, ptr: CFTypeRef) -> Option<String> {
            match self.state.borrow().objects.get(&(ptr as usize)) {
                Some(Obj::Str(s)) => Some(s.clone()),
                _ => None,
            }
        }

        fn get_data(&self, ptr: CFTypeRef) -> Option<Vec<u8>> {
            match self.state.borrow().objects.get(&(ptr as usize)) {
                Some(Obj::Data(d)) => Some(d.clone()),
                _ => None,
            }
        }

        fn get_dict(&self, ptr: CFTypeRef) -> BTreeMap<usize, usize> {
            match self.state.borrow().objects.get(&(ptr as usize)) {
                Some(Obj::Dict(m)) => m.clone(),
                _ => panic!("expected fake CFDictionary"),
            }
        }
    }

    impl KeychainOps for FakeKeychain {
        fn constants(&self) -> Constants {
            Constants {
                k_bool_true: K_BOOL_TRUE,
                k_class: K_CLASS,
                k_class_generic_password: K_CLASS_GENERIC,
                k_attr_service: K_ATTR_SERVICE,
                k_attr_account: K_ATTR_ACCOUNT,
                k_value_data: K_VALUE_DATA,
                k_return_data: K_RETURN_DATA,
                k_return_attributes: K_RETURN_ATTRS,
                k_match_limit: K_MATCH_LIMIT,
                k_match_limit_one: K_MATCH_LIMIT_ONE,
                k_match_limit_all: self.macos_mode.then_some(K_MATCH_LIMIT_ALL),
            }
        }

        fn cf_release(&self, cf: CFTypeRef) {
            let removed = self.state.borrow_mut().objects.remove(&(cf as usize));
            assert!(removed.is_some(), "double or unknown CFRelease");
        }

        fn cf_string_create(&self, bytes: &[u8]) -> CFStringRef {
            let mut st = self.state.borrow_mut();
            st.string_calls += 1;
            if st.fail_string_on == Some(st.string_calls) {
                return std::ptr::null();
            }
            drop(st);
            self.alloc(Obj::Str(String::from_utf8_lossy(bytes).into_owned()))
        }

        fn cf_data_create(&self, bytes: &[u8]) -> CFDataRef {
            if self.state.borrow().fail_data {
                return std::ptr::null();
            }
            self.alloc(Obj::Data(bytes.to_vec()))
        }

        fn cf_dictionary_create(&self, keys: &[CFTypeRef], values: &[CFTypeRef]) -> CFDictionaryRef {
            let mut st = self.state.borrow_mut();
            if keys.iter().any(|p| p.is_null()) || values.iter().any(|p| p.is_null()) {
                st.dict_received_null = true;
                panic!("CFDictionaryCreate received null pointer");
            }
            st.dict_calls += 1;
            if st.fail_dict_on == Some(st.dict_calls) {
                return std::ptr::null();
            }
            let mut map = BTreeMap::new();
            for (&k, &v) in keys.iter().zip(values.iter()) {
                map.insert(k as usize, v as usize);
            }
            drop(st);
            self.alloc(Obj::Dict(map))
        }

        fn cf_dictionary_get_value(&self, dict: CFDictionaryRef, key: CFTypeRef) -> CFTypeRef {
            self.get_dict(dict).get(&(key as usize)).copied().unwrap_or(0) as CFTypeRef
        }

        fn cf_string_to_rust(&self, s: CFStringRef) -> String {
            self.get_str(s).unwrap_or_default()
        }

        fn cf_data_length(&self, d: CFDataRef) -> CFIndex {
            if let Some(forced) = self.state.borrow().override_data_len {
                return forced;
            }
            self.get_data(d).map_or(0, |b| b.len() as CFIndex)
        }

        fn cf_data_copy_bytes(&self, d: CFDataRef, len: usize) -> Vec<u8> {
            self.state.borrow_mut().copy_bytes_called = true;
            let bytes = self.get_data(d).unwrap_or_default();
            bytes[..len.min(bytes.len())].to_vec()
        }

        fn sec_item_update(&self, query: CFDictionaryRef, changes: CFDictionaryRef) -> OSStatus {
            self.state.borrow_mut().sec_calls += 1;
            if let Some(code) = self.state.borrow().forced_status {
                return code;
            }
            let q = self.get_dict(query);
            let ch = self.get_dict(changes);
            let svc = self.get_str(q[&(K_ATTR_SERVICE as usize)] as CFTypeRef).unwrap();
            let acct = self.get_str(ch[&(K_ATTR_ACCOUNT as usize)] as CFTypeRef).unwrap();
            let data = self.get_data(ch[&(K_VALUE_DATA as usize)] as CFTypeRef).unwrap();
            let mut st = self.state.borrow_mut();
            let matches: Vec<usize> = st.items.iter().enumerate().filter(|(_, (s, _, _))| s == &svc).map(|(i, _)| i).collect();
            if matches.is_empty() {
                return ERR_SEC_ITEM_NOT_FOUND;
            }
            if matches.len() > 1 {
                return ERR_SEC_DUPLICATE_ITEM;
            }
            st.items[matches[0]] = (svc, acct, data);
            0
        }

        fn sec_item_add(&self, attributes: CFDictionaryRef) -> OSStatus {
            self.state.borrow_mut().sec_calls += 1;
            if let Some(code) = self.state.borrow().forced_status {
                return code;
            }
            let a = self.get_dict(attributes);
            let svc = self.get_str(a[&(K_ATTR_SERVICE as usize)] as CFTypeRef).unwrap();
            let acct = self.get_str(a[&(K_ATTR_ACCOUNT as usize)] as CFTypeRef).unwrap();
            let data = self.get_data(a[&(K_VALUE_DATA as usize)] as CFTypeRef).unwrap();
            let mut st = self.state.borrow_mut();
            if st.items.iter().any(|(s, u, _)| s == &svc && u == &acct) {
                return ERR_SEC_DUPLICATE_ITEM;
            }
            st.items.push((svc, acct, data));
            0
        }

        fn sec_item_copy_matching(&self, query: CFDictionaryRef) -> (OSStatus, CFTypeRef) {
            self.state.borrow_mut().sec_calls += 1;
            if let Some(code) = self.state.borrow().forced_status {
                return (code, std::ptr::null());
            }
            let q = self.get_dict(query);
            let svc = self.get_str(q[&(K_ATTR_SERVICE as usize)] as CFTypeRef).unwrap();
            let found = self.state.borrow().items.iter().find(|(s, _, _)| s == &svc).cloned();
            let Some((_, acct, data)) = found else {
                return (ERR_SEC_ITEM_NOT_FOUND, std::ptr::null());
            };
            let acct_ref = self.alloc(Obj::Str(acct));
            let data_ref = self.alloc(Obj::Data(data));
            let mut map = BTreeMap::new();
            map.insert(K_ATTR_ACCOUNT as usize, acct_ref as usize);
            map.insert(K_VALUE_DATA as usize, data_ref as usize);
            let dict = self.alloc(Obj::Dict(map));
            (0, dict)
        }

        fn sec_item_delete(&self, query: CFDictionaryRef) -> OSStatus {
            self.state.borrow_mut().sec_calls += 1;
            if let Some(code) = self.state.borrow().forced_status {
                return code;
            }
            let q = self.get_dict(query);
            let svc = self.get_str(q[&(K_ATTR_SERVICE as usize)] as CFTypeRef).unwrap();
            let match_limit = q.get(&(K_MATCH_LIMIT as usize)).copied();
            if !self.macos_mode && match_limit.is_some() {
                // iOS rejects kSecMatchLimit in SecItemDelete with errSecParam.
                return ERR_SEC_PARAM;
            }
            let delete_all = !self.macos_mode || match_limit == Some(K_MATCH_LIMIT_ALL as usize);
            let mut st = self.state.borrow_mut();
            let before = st.items.len();
            if delete_all {
                st.items.retain(|(s, _, _)| s != &svc);
            } else if let Some(pos) = st.items.iter().position(|(s, _, _)| s == &svc) {
                // macOS file-based keychain deletes only one matching item without kSecMatchLimitAll.
                st.items.remove(pos);
            }
            if st.items.len() == before { ERR_SEC_ITEM_NOT_FOUND } else { 0 }
        }
    }

    #[test]
    fn apple_null_cf_results_fail_cleanly_without_retaining_null() {
        // Null from CFStringCreateWithBytes (service, then account).
        for call in [1, 2] {
            let fake = FakeKeychain::new(true);
            fake.state.borrow_mut().fail_string_on = Some(call);
            assert!(matches!(write_with(&fake, "svc", "acct", b"sec"), Err(CredentialError::Failed(_))));
            assert!(!fake.state.borrow().dict_received_null);
            assert_eq!(fake.state.borrow().sec_calls, 0);
            assert_eq!(fake.live_refs(), 0);
        }

        // Null from CFDataCreate.
        let fake = FakeKeychain::new(true);
        fake.state.borrow_mut().fail_data = true;
        assert!(matches!(write_with(&fake, "svc", "acct", b"sec"), Err(CredentialError::Failed(_))));
        assert_eq!(fake.state.borrow().sec_calls, 0);
        assert_eq!(fake.live_refs(), 0);

        // Null from CFDictionaryCreate on query, changes, and attributes.
        for call in [1, 2, 3] {
            let fake = FakeKeychain::new(true);
            fake.state.borrow_mut().fail_dict_on = Some(call);
            assert!(matches!(write_with(&fake, "svc", "acct", b"sec"), Err(CredentialError::Failed(_))));
            assert_eq!(fake.live_refs(), 0);
        }

        // Null on read and delete.
        let fake = FakeKeychain::new(true);
        fake.state.borrow_mut().fail_string_on = Some(1);
        assert!(matches!(read_with(&fake, "svc"), Err(CredentialError::Failed(_))));
        assert_eq!(fake.state.borrow().sec_calls, 0);
        assert_eq!(fake.live_refs(), 0);

        let fake = FakeKeychain::new(true);
        fake.state.borrow_mut().fail_dict_on = Some(1);
        assert!(matches!(delete_with(&fake, "svc"), Err(CredentialError::Failed(_))));
        assert_eq!(fake.state.borrow().sec_calls, 0);
        assert_eq!(fake.live_refs(), 0);
    }

    #[test]
    fn apple_macos_delete_passes_match_limit_all_and_clears_duplicates() {
        let fake = FakeKeychain::new(true);
        fake.state.borrow_mut().items.push(("svc".into(), "a".into(), b"one".to_vec()));
        fake.state.borrow_mut().items.push(("svc".into(), "b".into(), b"two".to_vec()));

        // Writing over duplicates fails with errSecDuplicateItem and leaves both intact.
        let err = write_with(&fake, "svc", "c", b"three").unwrap_err();
        assert!(matches!(err, CredentialError::Failed(ref m) if m.contains("errSecDuplicateItem")));
        assert_eq!(fake.state.borrow().items.len(), 2);

        // delete_with must pass kSecMatchLimitAll on macOS and clear BOTH duplicates in one call.
        delete_with(&fake, "svc").unwrap();
        assert!(fake.state.borrow().items.is_empty());

        // Subsequent write and read succeed.
        write_with(&fake, "svc", "c", b"three").unwrap();
        let found = read_with(&fake, "svc").unwrap().unwrap();
        assert_eq!((found.account(), found.secret()), ("c", &b"three"[..]));
    }

    #[test]
    fn apple_ios_delete_omits_match_limit_and_clears_duplicates() {
        let fake = FakeKeychain::new(false);
        fake.state.borrow_mut().items.push(("svc".into(), "a".into(), b"one".to_vec()));
        fake.state.borrow_mut().items.push(("svc".into(), "b".into(), b"two".to_vec()));

        delete_with(&fake, "svc").unwrap();
        assert!(fake.state.borrow().items.is_empty());
    }

    #[test]
    fn apple_negative_cf_data_length_floors_at_zero() {
        let fake = FakeKeychain::new(true);
        write_with(&fake, "svc", "me", b"secret").unwrap();
        fake.state.borrow_mut().override_data_len = Some(-1);

        let found = read_with(&fake, "svc").unwrap().unwrap();
        assert_eq!((found.account(), found.secret()), ("me", &b""[..]));
        assert!(!fake.state.borrow().copy_bytes_called);
    }

    #[test]
    fn apple_maps_denied_and_not_found_statuses() {
        for code in [ERR_SEC_USER_CANCELED, ERR_SEC_AUTH_FAILED, ERR_SEC_INTERACTION_NOT_ALLOWED] {
            let fake = FakeKeychain::new(true);
            fake.state.borrow_mut().forced_status = Some(code);
            assert_eq!(write_with(&fake, "svc", "me", b"x"), Err(CredentialError::Denied));
            assert_eq!(read_with(&fake, "svc"), Err(CredentialError::Denied));
            assert_eq!(delete_with(&fake, "svc"), Err(CredentialError::Denied));
        }

        let fake = FakeKeychain::new(true);
        assert_eq!(read_with(&fake, "missing"), Ok(None));
        assert_eq!(delete_with(&fake, "missing"), Ok(()));
    }
}

mod secret_service_seam_tests {
    use std::cell::RefCell;
    use std::collections::BTreeMap;
    use std::future::Future;
    use std::pin::pin;
    use std::task::{Context, Poll, Waker};

    use super::super::secret_service::{
        SCHEMA, SecretOps, SecretPayload, delete_all_with, read_with, write_with,
    };
    use super::CredentialError;

    fn block_on_ready<F: Future>(future: F) -> F::Output {
        let mut future = pin!(future);
        let waker = Waker::noop();
        let mut cx = Context::from_waker(waker);
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(out) => out,
            Poll::Pending => panic!("fake SecretOps future unexpectedly returned Pending"),
        }
    }

    #[derive(Clone)]
    struct Item {
        collection: String,
        service: String,
        account: String,
        secret: Vec<u8>,
        modified: u64,
        locked: bool,
    }

    struct FakeSecretService {
        default_collection: Option<String>,
        default_locked: bool,
        unlock_immediate: Option<Vec<String>>,
        unlock_prompt_path: Option<String>,
        unlock_prompt_result: Option<Result<Vec<String>, CredentialError>>,
        create_via_prompt: bool,
        next_id: u64,
        clock: u64,
        items: BTreeMap<String, Item>,
        unlock_calls: Vec<Vec<String>>,
        pending_prompt_item: Option<String>,
    }

    impl Default for FakeSecretService {
        fn default() -> Self {
            Self {
                default_collection: Some("/org/freedesktop/secrets/collection/login".into()),
                default_locked: false,
                unlock_immediate: None,
                unlock_prompt_path: None,
                unlock_prompt_result: None,
                create_via_prompt: false,
                next_id: 1,
                clock: 100,
                items: BTreeMap::new(),
                unlock_calls: Vec::new(),
                pending_prompt_item: None,
            }
        }
    }

    struct FakeOps(RefCell<FakeSecretService>);

    impl FakeOps {
        fn new(state: FakeSecretService) -> Self { Self(RefCell::new(state)) }
    }

    impl SecretOps for FakeOps {
        async fn search_items(&self, service: &str) -> Result<(Vec<String>, Vec<String>), CredentialError> {
            let st = self.0.borrow();
            let (mut unlocked, mut locked) = (Vec::new(), Vec::new());
            for (path, item) in &st.items {
                if item.service == service {
                    if item.locked { locked.push(path.clone()); } else { unlocked.push(path.clone()); }
                }
            }
            Ok((unlocked, locked))
        }

        async fn unlock(&self, objects: &[String]) -> Result<(Vec<String>, String), CredentialError> {
            let mut st = self.0.borrow_mut();
            st.unlock_calls.push(objects.to_vec());
            if let Some(immediate) = st.unlock_immediate.clone() {
                let prompt = st.unlock_prompt_path.clone().unwrap_or_else(|| "/".into());
                return Ok((immediate, prompt));
            }
            for obj in objects {
                if st.default_collection.as_deref() == Some(obj.as_str()) {
                    st.default_locked = false;
                }
                if let Some(item) = st.items.get_mut(obj) {
                    item.locked = false;
                }
            }
            Ok((objects.to_vec(), "/".into()))
        }

        async fn prompt_unlock(&self, _prompt: &str) -> Result<Vec<String>, CredentialError> {
            let mut st = self.0.borrow_mut();
            let res = st.unlock_prompt_result.clone().unwrap_or_else(|| Ok(Vec::new()))?;
            for obj in &res {
                if st.default_collection.as_deref() == Some(obj.as_str()) {
                    st.default_locked = false;
                }
                if let Some(item) = st.items.get_mut(obj) {
                    item.locked = false;
                }
            }
            Ok(res)
        }

        async fn read_alias(&self, _alias: &str) -> Result<String, CredentialError> {
            Ok(self.0.borrow().default_collection.clone().unwrap_or_else(|| "/".into()))
        }

        async fn create_collection(&self, _label: &str, _alias: &str) -> Result<(String, String), CredentialError> {
            let path = "/org/freedesktop/secrets/collection/login".to_string();
            self.0.borrow_mut().default_collection = Some(path.clone());
            Ok((path, "/".into()))
        }

        async fn prompt_object_path(&self, _prompt: &str) -> Result<String, CredentialError> {
            Ok(self.0.borrow_mut().pending_prompt_item.take().unwrap_or_else(|| "/".into()))
        }

        async fn create_item(
            &self,
            collection: &str,
            service: &str,
            account: &str,
            _label: &str,
            secret: &SecretPayload,
        ) -> Result<(String, String), CredentialError> {
            let mut st = self.0.borrow_mut();
            st.clock += 1;
            let modified = st.clock;
            // Replace only within the target collection, matching the Secret Service spec.
            let existing = st
                .items
                .iter()
                .find(|(_, it)| it.collection == collection && it.service == service && it.account == account)
                .map(|(p, _)| p.clone());
            let path = existing.unwrap_or_else(|| {
                let id = st.next_id;
                st.next_id += 1;
                format!("{collection}/{id}")
            });
            st.items.insert(
                path.clone(),
                Item {
                    collection: collection.to_owned(),
                    service: service.to_owned(),
                    account: account.to_owned(),
                    secret: secret.as_slice().to_vec(),
                    modified,
                    locked: false,
                },
            );
            if st.create_via_prompt {
                st.pending_prompt_item = Some(path);
                Ok(("/".into(), "/org/freedesktop/secrets/prompt/create".into()))
            } else {
                Ok((path, "/".into()))
            }
        }

        async fn item_modified(&self, item: &str) -> Result<u64, CredentialError> {
            Ok(self.0.borrow().items[item].modified)
        }

        async fn item_account(&self, item: &str) -> Result<String, CredentialError> {
            Ok(self.0.borrow().items[item].account.clone())
        }

        async fn get_secret(&self, item: &str) -> Result<SecretPayload, CredentialError> {
            let it = &self.0.borrow().items[item];
            assert!(!it.locked, "GetSecret called on a still-locked item");
            Ok(SecretPayload::new(it.secret.clone()))
        }

        async fn delete_item(&self, item: &str) -> Result<String, CredentialError> {
            self.0.borrow_mut().items.remove(item);
            Ok("/".into())
        }

        async fn prompt_unit(&self, _prompt: &str) -> Result<(), CredentialError> {
            Ok(())
        }
    }

    #[test]
    fn secret_service_unlock_rejects_unconfirmed_unlock_without_prompt() {
        assert_eq!(SCHEMA, "dev.rune.Credential");
        let mut st = FakeSecretService::default();
        st.items.insert(
            "/org/freedesktop/secrets/collection/login/1".into(),
            Item {
                collection: "/org/freedesktop/secrets/collection/login".into(),
                service: "svc".into(),
                account: "me".into(),
                secret: b"locked-secret".to_vec(),
                modified: 10,
                locked: true,
            },
        );
        // Daemon returns empty unlocked list and "/" (no prompt) -> must fail with Denied.
        st.unlock_immediate = Some(Vec::new());
        let ops = FakeOps::new(st);

        assert_eq!(block_on_ready(read_with(&ops, "svc")), Err(CredentialError::Denied));
        assert_eq!(block_on_ready(write_with(&ops, "svc", "me", b"new")), Err(CredentialError::Denied));
    }

    #[test]
    fn secret_service_unlock_checks_prompt_result_paths() {
        let mut st = FakeSecretService::default();
        let p1 = "/org/freedesktop/secrets/collection/login/1".to_string();
        let p2 = "/org/freedesktop/secrets/collection/other/2".to_string();
        for (p, col, m) in [(&p1, "/org/freedesktop/secrets/collection/login", 10), (&p2, "/org/freedesktop/secrets/collection/other", 20)] {
            st.items.insert(
                p.clone(),
                Item {
                    collection: col.into(),
                    service: "svc".into(),
                    account: "me".into(),
                    secret: b"sec".to_vec(),
                    modified: m,
                    locked: true,
                },
            );
        }
        // Only p1 unlocked immediately; prompt returns empty list (p2 still locked) -> Denied.
        st.unlock_immediate = Some(vec![p1.clone()]);
        st.unlock_prompt_path = Some("/org/freedesktop/secrets/prompt/unlock".into());
        st.unlock_prompt_result = Some(Ok(Vec::new()));
        let ops = FakeOps::new(st);
        assert_eq!(block_on_ready(read_with(&ops, "svc")), Err(CredentialError::Denied));

        // When prompt unlocks p2 as well, read succeeds and returns the newest item.
        ops.0.borrow_mut().unlock_prompt_result = Some(Ok(vec![p2]));
        let found = block_on_ready(read_with(&ops, "svc")).unwrap().unwrap();
        assert_eq!((found.account(), found.secret()), ("me", &b"sec"[..]));
    }

    #[test]
    fn secret_service_write_removes_same_account_duplicate_in_other_collection() {
        for create_via_prompt in [false, true] {
            let mut st = FakeSecretService { create_via_prompt, ..FakeSecretService::default() };
            let other_item = "/org/freedesktop/secrets/collection/other/99".to_string();
            st.items.insert(
                other_item.clone(),
                Item {
                    collection: "/org/freedesktop/secrets/collection/other".into(),
                    service: "svc".into(),
                    account: "same-account".into(),
                    secret: b"stale".to_vec(),
                    modified: 50,
                    locked: false,
                },
            );
            let ops = FakeOps::new(st);

            block_on_ready(write_with(&ops, "svc", "same-account", b"fresh")).unwrap();

            // Duplicate with the SAME account in the other collection must be gone; only the new
            // default-collection item remains.
            let remaining: Vec<String> = ops.0.borrow().items.keys().cloned().collect();
            assert_eq!(remaining.len(), 1);
            assert!(!remaining.contains(&other_item));
            assert!(remaining[0].starts_with("/org/freedesktop/secrets/collection/login/"));

            let found = block_on_ready(read_with(&ops, "svc")).unwrap().unwrap();
            assert_eq!((found.account(), found.secret()), ("same-account", &b"fresh"[..]));
        }
    }

    #[test]
    fn secret_service_write_skips_locked_other_collection_and_delete_clears_all() {
        let mut st = FakeSecretService::default();
        let other_locked = "/org/freedesktop/secrets/collection/other/7".to_string();
        st.items.insert(
            other_locked.clone(),
            Item {
                collection: "/org/freedesktop/secrets/collection/other".into(),
                service: "svc".into(),
                account: "me".into(),
                secret: b"old".to_vec(),
                modified: 10,
                locked: true,
            },
        );
        let ops = FakeOps::new(st);

        block_on_ready(write_with(&ops, "svc", "me", b"new")).unwrap();
        // Tidy-up must not have asked to unlock the locked item in the other collection.
        assert!(ops.0.borrow().unlock_calls.iter().all(|call| !call.contains(&other_locked)));
        // read unlocks all and returns the newer item.
        let found = block_on_ready(read_with(&ops, "svc")).unwrap().unwrap();
        assert_eq!(found.secret(), b"new");
        // delete removes every item across collections.
        block_on_ready(delete_all_with(&ops, "svc")).unwrap();
        assert!(ops.0.borrow().items.is_empty());
    }
}
