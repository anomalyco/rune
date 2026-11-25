//! The Keychain: one generic password per service (`kSecAttrService`), its account
//! `kSecAttrAccount`.
#![allow(unsafe_code)]

use std::ffi::c_void;

use super::{Credential, CredentialError};

pub(super) type CFTypeRef = *const c_void;
pub(super) type CFStringRef = *const c_void;
pub(super) type CFDataRef = *const c_void;
pub(super) type CFDictionaryRef = *const c_void;
pub(super) type CFIndex = isize;
pub(super) type OSStatus = i32;

pub(super) const ERR_SEC_ITEM_NOT_FOUND: OSStatus = -25300;
pub(super) const ERR_SEC_USER_CANCELED: OSStatus = -128;
pub(super) const ERR_SEC_AUTH_FAILED: OSStatus = -25293;
pub(super) const ERR_SEC_INTERACTION_NOT_ALLOWED: OSStatus = -25308;
pub(super) const ERR_SEC_DUPLICATE_ITEM: OSStatus = -25299;

/// Keychain and CoreFoundation constants used to build query and attribute dictionaries.
pub(super) struct Constants {
    pub(super) k_bool_true: CFTypeRef,
    pub(super) k_class: CFStringRef,
    pub(super) k_class_generic_password: CFStringRef,
    pub(super) k_attr_service: CFStringRef,
    pub(super) k_attr_account: CFStringRef,
    pub(super) k_value_data: CFStringRef,
    pub(super) k_return_data: CFStringRef,
    pub(super) k_return_attributes: CFStringRef,
    pub(super) k_match_limit: CFStringRef,
    pub(super) k_match_limit_one: CFStringRef,
    /// `Some(kSecMatchLimitAll)` on macOS (the file-based keychain defaults `SecItemDelete` to
    /// `kSecMatchLimitOne`; Apple TN3137 / r. 105800863), `None` on iOS where `SecItemDelete`
    /// already deletes all matches and rejects `kSecMatchLimit`.
    pub(super) k_match_limit_all: Option<CFStringRef>,
}

/// Minimal seam over CoreFoundation and Security.framework so unit tests can exercise null
/// handling, duplicate deletion, and signed length bounds without calling the real Keychain.
pub(super) trait KeychainOps {
    fn constants(&self) -> Constants;
    fn cf_release(&self, cf: CFTypeRef);
    fn cf_string_create(&self, bytes: &[u8]) -> CFStringRef;
    fn cf_data_create(&self, bytes: &[u8]) -> CFDataRef;
    fn cf_dictionary_create(&self, keys: &[CFTypeRef], values: &[CFTypeRef]) -> CFDictionaryRef;
    fn cf_dictionary_get_value(&self, dict: CFDictionaryRef, key: CFTypeRef) -> CFTypeRef;
    fn cf_string_to_rust(&self, s: CFStringRef) -> String;
    fn cf_data_length(&self, d: CFDataRef) -> CFIndex;
    fn cf_data_copy_bytes(&self, d: CFDataRef, len: usize) -> Vec<u8>;
    fn sec_item_update(&self, query: CFDictionaryRef, changes: CFDictionaryRef) -> OSStatus;
    fn sec_item_add(&self, attributes: CFDictionaryRef) -> OSStatus;
    fn sec_item_copy_matching(&self, query: CFDictionaryRef) -> (OSStatus, CFTypeRef);
    fn sec_item_delete(&self, query: CFDictionaryRef) -> OSStatus;
}

/// A Core Foundation object this code made or copied, released on drop.
struct Owned<'a, B: KeychainOps> {
    ops: &'a B,
    ptr: CFTypeRef,
}

impl<'a, B: KeychainOps> Owned<'a, B> {
    fn new(ops: &'a B, ptr: CFTypeRef) -> Self {
        Self { ops, ptr }
    }
}

impl<B: KeychainOps> Drop for Owned<'_, B> {
    fn drop(&mut self) {
        if !self.ptr.is_null() {
            self.ops.cf_release(self.ptr);
        }
    }
}

fn make_string<'a, B: KeychainOps>(ops: &'a B, s: &str) -> Result<Owned<'a, B>, CredentialError> {
    if CFIndex::try_from(s.len()).is_err() {
        return Err(CredentialError::Invalid);
    }
    let ptr = ops.cf_string_create(s.as_bytes());
    if ptr.is_null() {
        return Err(CredentialError::Failed("CFStringCreateWithBytes returned null".into()));
    }
    Ok(Owned::new(ops, ptr))
}

fn make_data<'a, B: KeychainOps>(ops: &'a B, bytes: &[u8]) -> Result<Owned<'a, B>, CredentialError> {
    if CFIndex::try_from(bytes.len()).is_err() {
        return Err(CredentialError::Invalid);
    }
    let ptr = ops.cf_data_create(bytes);
    if ptr.is_null() {
        return Err(CredentialError::Failed("CFDataCreate returned null".into()));
    }
    Ok(Owned::new(ops, ptr))
}

fn make_dictionary<'a, B: KeychainOps>(
    ops: &'a B,
    pairs: &[(CFTypeRef, CFTypeRef)],
) -> Result<Owned<'a, B>, CredentialError> {
    if pairs.iter().any(|(k, v)| k.is_null() || v.is_null()) {
        return Err(CredentialError::Failed("CFDictionaryCreate received null key or value".into()));
    }
    let keys: Vec<CFTypeRef> = pairs.iter().map(|p| p.0).collect();
    let values: Vec<CFTypeRef> = pairs.iter().map(|p| p.1).collect();
    let ptr = ops.cf_dictionary_create(&keys, &values);
    if ptr.is_null() {
        return Err(CredentialError::Failed("CFDictionaryCreate returned null".into()));
    }
    Ok(Owned::new(ops, ptr))
}

fn status(call: &str, code: OSStatus) -> Result<(), CredentialError> {
    match code {
        0 => Ok(()),
        ERR_SEC_USER_CANCELED | ERR_SEC_AUTH_FAILED | ERR_SEC_INTERACTION_NOT_ALLOWED => Err(CredentialError::Denied),
        ERR_SEC_DUPLICATE_ITEM => Err(CredentialError::Failed(format!(
            "{call} failed (errSecDuplicateItem): the keychain holds several items for this service; delete them first"
        ))),
        code => Err(CredentialError::Failed(format!("{call} failed ({code})"))),
    }
}

pub(super) fn write_with<B: KeychainOps>(
    ops: &B,
    service: &str,
    account: &str,
    secret: &[u8],
) -> Result<(), CredentialError> {
    let c = ops.constants();
    let service = make_string(ops, service)?;
    let account = make_string(ops, account)?;
    let secret = make_data(ops, secret)?;
    let query = make_dictionary(ops, &[(c.k_class, c.k_class_generic_password), (c.k_attr_service, service.ptr)])?;
    let changes = make_dictionary(ops, &[(c.k_attr_account, account.ptr), (c.k_value_data, secret.ptr)])?;
    let code = ops.sec_item_update(query.ptr, changes.ptr);
    if code != ERR_SEC_ITEM_NOT_FOUND {
        return status("SecItemUpdate", code);
    }
    let attributes = make_dictionary(
        ops,
        &[
            (c.k_class, c.k_class_generic_password),
            (c.k_attr_service, service.ptr),
            (c.k_attr_account, account.ptr),
            (c.k_value_data, secret.ptr),
        ],
    )?;
    status("SecItemAdd", ops.sec_item_add(attributes.ptr))
}

pub(super) fn read_with<B: KeychainOps>(ops: &B, service: &str) -> Result<Option<Credential>, CredentialError> {
    let c = ops.constants();
    let service = make_string(ops, service)?;
    let query = make_dictionary(
        ops,
        &[
            (c.k_class, c.k_class_generic_password),
            (c.k_attr_service, service.ptr),
            (c.k_return_data, c.k_bool_true),
            (c.k_return_attributes, c.k_bool_true),
            (c.k_match_limit, c.k_match_limit_one),
        ],
    )?;
    let (code, raw_result) = ops.sec_item_copy_matching(query.ptr);
    if code == ERR_SEC_ITEM_NOT_FOUND {
        return Ok(None);
    }
    status("SecItemCopyMatching", code)?;
    let result = Owned::new(ops, raw_result);
    if result.ptr.is_null() {
        return Ok(None);
    }
    let account = ops.cf_string_to_rust(ops.cf_dictionary_get_value(result.ptr, c.k_attr_account));
    let bytes = ops.cf_dictionary_get_value(result.ptr, c.k_value_data);
    let len = if bytes.is_null() { 0 } else { usize::try_from(ops.cf_data_length(bytes)).unwrap_or(0) };
    let secret = if len == 0 { Vec::new() } else { ops.cf_data_copy_bytes(bytes, len) };
    Ok(Some(Credential::new(account, secret)))
}

pub(super) fn delete_with<B: KeychainOps>(ops: &B, service: &str) -> Result<(), CredentialError> {
    let c = ops.constants();
    let service = make_string(ops, service)?;
    let mut pairs = vec![(c.k_class, c.k_class_generic_password), (c.k_attr_service, service.ptr)];
    if let Some(limit_all) = c.k_match_limit_all {
        pairs.push((c.k_match_limit, limit_all));
    }
    let query = make_dictionary(ops, &pairs)?;
    let code = ops.sec_item_delete(query.ptr);
    if code == ERR_SEC_ITEM_NOT_FOUND {
        return Ok(());
    }
    status("SecItemDelete", code)
}

#[cfg(target_vendor = "apple")]
mod native {
    use std::ffi::c_void;

    use super::{
        CFDataRef, CFDictionaryRef, CFIndex, CFStringRef, CFTypeRef, Constants, KeychainOps, OSStatus,
        delete_with, read_with, write_with,
    };
    use crate::credentials::{Credential, CredentialError};

    type CFAllocatorRef = *const c_void;

    const UTF8: u32 = 0x0800_0100; // kCFStringEncodingUTF8

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        static kCFBooleanTrue: CFTypeRef;
        // Opaque: only their addresses are passed on.
        static kCFTypeDictionaryKeyCallBacks: c_void;
        static kCFTypeDictionaryValueCallBacks: c_void;
        fn CFRelease(cf: CFTypeRef);
        fn CFStringCreateWithBytes(
            a: CFAllocatorRef,
            bytes: *const u8,
            len: CFIndex,
            encoding: u32,
            bom: u8,
        ) -> CFStringRef;
        fn CFStringGetLength(s: CFStringRef) -> CFIndex;
        fn CFStringGetMaximumSizeForEncoding(len: CFIndex, encoding: u32) -> CFIndex;
        fn CFStringGetCString(s: CFStringRef, buf: *mut u8, size: CFIndex, encoding: u32) -> u8;
        fn CFDataCreate(a: CFAllocatorRef, bytes: *const u8, len: CFIndex) -> CFDataRef;
        fn CFDataGetBytePtr(d: CFDataRef) -> *const u8;
        fn CFDataGetLength(d: CFDataRef) -> CFIndex;
        fn CFDictionaryCreate(
            a: CFAllocatorRef,
            keys: *const CFTypeRef,
            values: *const CFTypeRef,
            n: CFIndex,
            key_callbacks: *const c_void,
            value_callbacks: *const c_void,
        ) -> CFDictionaryRef;
        fn CFDictionaryGetValue(d: CFDictionaryRef, key: CFTypeRef) -> CFTypeRef;
    }

    #[link(name = "Security", kind = "framework")]
    unsafe extern "C" {
        static kSecClass: CFStringRef;
        static kSecClassGenericPassword: CFStringRef;
        static kSecAttrService: CFStringRef;
        static kSecAttrAccount: CFStringRef;
        static kSecValueData: CFStringRef;
        static kSecReturnData: CFStringRef;
        static kSecReturnAttributes: CFStringRef;
        static kSecMatchLimit: CFStringRef;
        static kSecMatchLimitOne: CFStringRef;
        #[cfg(target_os = "macos")]
        static kSecMatchLimitAll: CFStringRef;
        fn SecItemAdd(attributes: CFDictionaryRef, result: *mut CFTypeRef) -> OSStatus;
        fn SecItemCopyMatching(query: CFDictionaryRef, result: *mut CFTypeRef) -> OSStatus;
        fn SecItemDelete(query: CFDictionaryRef) -> OSStatus;
        fn SecItemUpdate(query: CFDictionaryRef, attributes: CFDictionaryRef) -> OSStatus;
    }

    struct RealKeychain;

    impl KeychainOps for RealKeychain {
        fn constants(&self) -> Constants {
            // SAFETY: framework-exported CoreFoundation and Security constant pointers.
            unsafe {
                Constants {
                    k_bool_true: kCFBooleanTrue,
                    k_class: kSecClass,
                    k_class_generic_password: kSecClassGenericPassword,
                    k_attr_service: kSecAttrService,
                    k_attr_account: kSecAttrAccount,
                    k_value_data: kSecValueData,
                    k_return_data: kSecReturnData,
                    k_return_attributes: kSecReturnAttributes,
                    k_match_limit: kSecMatchLimit,
                    k_match_limit_one: kSecMatchLimitOne,
                    #[cfg(target_os = "macos")]
                    k_match_limit_all: Some(kSecMatchLimitAll),
                    #[cfg(not(target_os = "macos"))]
                    k_match_limit_all: None,
                }
            }
        }

        fn cf_release(&self, cf: CFTypeRef) {
            // SAFETY: `Owned` only calls `cf_release` on a non-null +1 CF object it owns.
            unsafe { CFRelease(cf) };
        }

        fn cf_string_create(&self, bytes: &[u8]) -> CFStringRef {
            let ptr = if bytes.is_empty() { std::ptr::null() } else { bytes.as_ptr() };
            // SAFETY: `bytes` is valid UTF-8 with length already checked to fit `CFIndex`.
            unsafe { CFStringCreateWithBytes(std::ptr::null(), ptr, bytes.len() as CFIndex, UTF8, 0) }
        }

        fn cf_data_create(&self, bytes: &[u8]) -> CFDataRef {
            let ptr = if bytes.is_empty() { std::ptr::null() } else { bytes.as_ptr() };
            // SAFETY: `CFDataCreate` copies `bytes` whose length fits `CFIndex`.
            unsafe { CFDataCreate(std::ptr::null(), ptr, bytes.len() as CFIndex) }
        }

        fn cf_dictionary_create(&self, keys: &[CFTypeRef], values: &[CFTypeRef]) -> CFDictionaryRef {
            debug_assert_eq!(keys.len(), values.len());
            // SAFETY: `make_dictionary` verified all keys and values are non-null live CF objects;
            // the predefined callbacks retain them.
            unsafe {
                CFDictionaryCreate(
                    std::ptr::null(),
                    keys.as_ptr(),
                    values.as_ptr(),
                    keys.len() as CFIndex,
                    &kCFTypeDictionaryKeyCallBacks,
                    &kCFTypeDictionaryValueCallBacks,
                )
            }
        }

        fn cf_dictionary_get_value(&self, dict: CFDictionaryRef, key: CFTypeRef) -> CFTypeRef {
            // SAFETY: `dict` is a live CFDictionary returned by `SecItemCopyMatching` and `key` is a
            // live framework constant.
            unsafe { CFDictionaryGetValue(dict, key) }
        }

        fn cf_string_to_rust(&self, s: CFStringRef) -> String {
            if s.is_null() {
                return String::new();
            }
            // SAFETY: `s` is a live CFString; the buffer is sized from CoreFoundation's UTF-8 bound.
            unsafe {
                let len = CFStringGetLength(s);
                if len <= 0 {
                    return String::new();
                }
                let max = CFStringGetMaximumSizeForEncoding(len, UTF8);
                let Some(size) = max.checked_add(1).filter(|&n| n > 0) else {
                    return String::new();
                };
                let mut buf = vec![0u8; size as usize];
                if CFStringGetCString(s, buf.as_mut_ptr(), size, UTF8) == 0 {
                    return String::new();
                }
                let nul = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
                buf.truncate(nul);
                String::from_utf8(buf).unwrap_or_default()
            }
        }

        fn cf_data_length(&self, d: CFDataRef) -> CFIndex {
            // SAFETY: `d` is a non-null CFData obtained from `result`.
            unsafe { CFDataGetLength(d) }
        }

        fn cf_data_copy_bytes(&self, d: CFDataRef, len: usize) -> Vec<u8> {
            // SAFETY: `d` is a non-null CFData with length `len > 0` checked by the caller.
            unsafe {
                let start = CFDataGetBytePtr(d);
                if start.is_null() {
                    Vec::new()
                } else {
                    std::slice::from_raw_parts(start, len).to_vec()
                }
            }
        }

        fn sec_item_update(&self, query: CFDictionaryRef, changes: CFDictionaryRef) -> OSStatus {
            // SAFETY: both dictionaries are live and non-null.
            unsafe { SecItemUpdate(query, changes) }
        }

        fn sec_item_add(&self, attributes: CFDictionaryRef) -> OSStatus {
            // SAFETY: `attributes` is a live, non-null dictionary; no output reference is requested.
            unsafe { SecItemAdd(attributes, std::ptr::null_mut()) }
        }

        fn sec_item_copy_matching(&self, query: CFDictionaryRef) -> (OSStatus, CFTypeRef) {
            let mut result: CFTypeRef = std::ptr::null();
            // SAFETY: `query` is a live, non-null dictionary; on success `result` is a +1 CF object.
            let code = unsafe { SecItemCopyMatching(query, &mut result) };
            (code, result)
        }

        fn sec_item_delete(&self, query: CFDictionaryRef) -> OSStatus {
            // SAFETY: `query` is a live, non-null dictionary.
            unsafe { SecItemDelete(query) }
        }
    }

    /// Updates the service's item in place, or adds one when there is none, so a failure (a locked
    /// keychain, a declined prompt) leaves the old credential as it was.
    ///
    /// Rune keeps one item per service. When another tool left several for it under different
    /// accounts, giving them all one account would make them duplicates, and the Keychain refuses:
    /// that is [`CredentialError::Failed`] naming `errSecDuplicateItem`, and nothing changes. A
    /// [`delete`] first clears them.
    pub(in super::super) fn write(service: &str, account: &str, secret: &[u8]) -> Result<(), CredentialError> {
        write_with(&RealKeychain, service, account, secret)
    }

    pub(in super::super) fn read(service: &str) -> Result<Option<Credential>, CredentialError> {
        read_with(&RealKeychain, service)
    }

    pub(in super::super) fn delete(service: &str) -> Result<(), CredentialError> {
        delete_with(&RealKeychain, service)
    }
}

#[cfg(target_vendor = "apple")]
pub(super) use native::{delete, read, write};
