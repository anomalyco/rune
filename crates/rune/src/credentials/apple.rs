//! The Keychain: one generic password per service (`kSecAttrService`), its account
//! `kSecAttrAccount`.
#![allow(unsafe_code)]

use std::ffi::c_void;

use super::{Credential, CredentialError};

type CFTypeRef = *const c_void;
type CFStringRef = *const c_void;
type CFDataRef = *const c_void;
type CFDictionaryRef = *const c_void;
type CFAllocatorRef = *const c_void;
type CFIndex = isize;
type OSStatus = i32;

const UTF8: u32 = 0x0800_0100; // kCFStringEncodingUTF8
const ERR_SEC_ITEM_NOT_FOUND: OSStatus = -25300;
const ERR_SEC_USER_CANCELED: OSStatus = -128;
const ERR_SEC_AUTH_FAILED: OSStatus = -25293;
const ERR_SEC_INTERACTION_NOT_ALLOWED: OSStatus = -25308;
const ERR_SEC_DUPLICATE_ITEM: OSStatus = -25299;

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    static kCFBooleanTrue: CFTypeRef;
    // Opaque: only their addresses are passed on.
    static kCFTypeDictionaryKeyCallBacks: c_void;
    static kCFTypeDictionaryValueCallBacks: c_void;
    fn CFRelease(cf: CFTypeRef);
    fn CFStringCreateWithBytes(a: CFAllocatorRef, bytes: *const u8, len: CFIndex, encoding: u32, bom: u8) -> CFStringRef;
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
    fn SecItemAdd(attributes: CFDictionaryRef, result: *mut CFTypeRef) -> OSStatus;
    fn SecItemCopyMatching(query: CFDictionaryRef, result: *mut CFTypeRef) -> OSStatus;
    fn SecItemDelete(query: CFDictionaryRef) -> OSStatus;
    fn SecItemUpdate(query: CFDictionaryRef, attributes: CFDictionaryRef) -> OSStatus;
}

/// A Core Foundation object this code made or copied, released on drop.
struct Owned(CFTypeRef);

impl Drop for Owned {
    fn drop(&mut self) {
        // SAFETY: `self.0` is a +1 reference this value owns.
        if !self.0.is_null() { unsafe { CFRelease(self.0) } }
    }
}

fn string(s: &str) -> Owned {
    // SAFETY: the bytes are valid UTF-8 of the given length.
    Owned(unsafe { CFStringCreateWithBytes(std::ptr::null(), s.as_ptr(), s.len() as CFIndex, UTF8, 0) })
}

fn data(bytes: &[u8]) -> Owned {
    // SAFETY: CFDataCreate copies `bytes`.
    Owned(unsafe { CFDataCreate(std::ptr::null(), bytes.as_ptr(), bytes.len() as CFIndex) })
}

fn dictionary(pairs: &[(CFTypeRef, CFTypeRef)]) -> Owned {
    let keys: Vec<CFTypeRef> = pairs.iter().map(|p| p.0).collect();
    let values: Vec<CFTypeRef> = pairs.iter().map(|p| p.1).collect();
    // SAFETY: keys and values are live CF objects; the type callbacks retain them.
    Owned(unsafe {
        CFDictionaryCreate(
            std::ptr::null(),
            keys.as_ptr(),
            values.as_ptr(),
            pairs.len() as CFIndex,
            &kCFTypeDictionaryKeyCallBacks,
            &kCFTypeDictionaryValueCallBacks,
        )
    })
}

fn to_string(s: CFStringRef) -> String {
    if s.is_null() { return String::new() }
    // SAFETY: `s` is a live CFString; the buffer is as large as CF says the UTF-8 can be.
    unsafe {
        let size = CFStringGetMaximumSizeForEncoding(CFStringGetLength(s), UTF8) + 1;
        let mut buf = vec![0u8; size.max(1) as usize];
        if CFStringGetCString(s, buf.as_mut_ptr(), size, UTF8) == 0 { return String::new() }
        let len = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
        buf.truncate(len);
        String::from_utf8(buf).unwrap_or_default()
    }
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

/// Updates the service's item in place, or adds one when there is none, so a failure (a locked
/// keychain, a declined prompt) leaves the old credential as it was.
///
/// Rune keeps one item per service. When another tool left several for it under different
/// accounts, giving them all one account would make them duplicates, and the Keychain refuses:
/// that is [`CredentialError::Failed`] naming `errSecDuplicateItem`, and nothing changes. A
/// [`delete`] first clears them.
pub(super) fn write(service: &str, account: &str, secret: &[u8]) -> Result<(), CredentialError> {
    let (service, account, secret) = (string(service), string(account), data(secret));
    // SAFETY: the keys and values are the Security framework's constants and live CF objects.
    let (query, changes) = unsafe {
        (
            dictionary(&[(kSecClass, kSecClassGenericPassword), (kSecAttrService, service.0)]),
            dictionary(&[(kSecAttrAccount, account.0), (kSecValueData, secret.0)]),
        )
    };
    // SAFETY: both dictionaries are live.
    let code = unsafe { SecItemUpdate(query.0, changes.0) };
    if code != ERR_SEC_ITEM_NOT_FOUND { return status("SecItemUpdate", code) }
    // SAFETY: as above.
    let attributes = unsafe {
        dictionary(&[
            (kSecClass, kSecClassGenericPassword),
            (kSecAttrService, service.0),
            (kSecAttrAccount, account.0),
            (kSecValueData, secret.0),
        ])
    };
    // SAFETY: `attributes` is a live dictionary; no result is asked for.
    status("SecItemAdd", unsafe { SecItemAdd(attributes.0, std::ptr::null_mut()) })
}

pub(super) fn read(service: &str) -> Result<Option<Credential>, CredentialError> {
    let service = string(service);
    // SAFETY: the keys and values are the Security framework's constants and live CF objects.
    let query = unsafe {
        dictionary(&[
            (kSecClass, kSecClassGenericPassword),
            (kSecAttrService, service.0),
            (kSecReturnData, kCFBooleanTrue),
            (kSecReturnAttributes, kCFBooleanTrue),
            (kSecMatchLimit, kSecMatchLimitOne),
        ])
    };
    let mut result: CFTypeRef = std::ptr::null();
    // SAFETY: `query` is a live dictionary; a found item comes back as a +1 dictionary.
    let code = unsafe { SecItemCopyMatching(query.0, &mut result) };
    if code == ERR_SEC_ITEM_NOT_FOUND { return Ok(None) }
    status("SecItemCopyMatching", code)?;
    let result = Owned(result);
    if result.0.is_null() { return Ok(None) }
    // SAFETY: with both return flags the result is a dictionary of the item's attributes and data.
    unsafe {
        let account = to_string(CFDictionaryGetValue(result.0, kSecAttrAccount));
        let bytes = CFDictionaryGetValue(result.0, kSecValueData);
        let len = if bytes.is_null() { 0 } else { CFDataGetLength(bytes) as usize };
        let start = if len == 0 { std::ptr::null() } else { CFDataGetBytePtr(bytes) };
        let secret = if start.is_null() {
            Vec::new()
        } else {
            std::slice::from_raw_parts(start, len).to_vec()
        };
        Ok(Some(Credential::new(account, secret)))
    }
}

pub(super) fn delete(service: &str) -> Result<(), CredentialError> {
    let service = string(service);
    // SAFETY: the keys are the Security framework's constants.
    let query = unsafe { dictionary(&[(kSecClass, kSecClassGenericPassword), (kSecAttrService, service.0)]) };
    // SAFETY: `query` is a live dictionary.
    let code = unsafe { SecItemDelete(query.0) };
    if code == ERR_SEC_ITEM_NOT_FOUND { return Ok(()) }
    status("SecItemDelete", code)
}
