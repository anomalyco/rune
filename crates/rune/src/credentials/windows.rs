//! Credential Manager: one generic credential per service, its target name the service. `CredWriteW`
//! replaces it whole or not at all.
#![allow(unsafe_code)]

use windows_sys::Win32::Foundation::{ERROR_ACCESS_DENIED, ERROR_NOT_FOUND, GetLastError};
use windows_sys::Win32::Security::Credentials::{
    CRED_MAX_CREDENTIAL_BLOB_SIZE, CRED_MAX_GENERIC_TARGET_NAME_LENGTH, CRED_MAX_USERNAME_LENGTH,
    CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC, CREDENTIALW, CredDeleteW, CredFree, CredReadW,
    CredWriteW,
};
use zeroize::Zeroize;

use super::{Credential, CredentialError};

/// Encodes `s` as NUL-terminated UTF-16 and checks that its length (including the NUL terminator,
/// matching the Win32 Credential Manager RPC buffer bounds `CRED_MAX_GENERIC_TARGET_NAME_LENGTH`
/// and `CRED_MAX_USERNAME_LENGTH`) fits in `max_units_with_nul`.
pub(super) fn encode_wide(s: &str, max_units_with_nul: usize) -> Result<Vec<u16>, CredentialError> {
    let wide: Vec<u16> = s.encode_utf16().chain(Some(0)).collect();
    if wide.len() > max_units_with_nul {
        return Err(CredentialError::Invalid);
    }
    Ok(wide)
}

pub(super) fn error_from_code(call: &str, code: u32) -> CredentialError {
    match code {
        ERROR_ACCESS_DENIED => CredentialError::Denied,
        _ => CredentialError::Failed(format!("{call} failed ({code})")),
    }
}

/// Owns a `CREDENTIALW` buffer returned by `CredReadW`: wipes its secret blob with a volatile
/// zeroing write and frees the block on every exit path, including panics and allocation failures.
pub(super) struct CredGuard {
    ptr: *mut CREDENTIALW,
    free: unsafe fn(*mut CREDENTIALW),
}

impl CredGuard {
    pub(super) fn new(ptr: *mut CREDENTIALW, free: unsafe fn(*mut CREDENTIALW)) -> Self {
        Self { ptr, free }
    }

    fn as_ref(&self) -> &CREDENTIALW {
        // SAFETY: `CredGuard` is only constructed with a valid, non-null `CREDENTIALW` pointer
        // owned until `Drop`.
        unsafe { &*self.ptr }
    }
}

impl Drop for CredGuard {
    fn drop(&mut self) {
        if self.ptr.is_null() {
            return;
        }
        // SAFETY: `self.ptr` is a valid, exclusively owned `CREDENTIALW` block until `free` runs.
        // `Zeroize::zeroize` performs volatile writes with a compiler fence so the wipe cannot be
        // eliminated as dead before `CredFree`.
        unsafe {
            let c = &*self.ptr;
            if !c.CredentialBlob.is_null() && c.CredentialBlobSize > 0 {
                std::slice::from_raw_parts_mut(c.CredentialBlob, c.CredentialBlobSize as usize).zeroize();
            }
            (self.free)(self.ptr);
        }
    }
}

/// # Safety
/// `ptr` must be a buffer allocated by `CredReadW`, freed at most once.
unsafe fn cred_free(ptr: *mut CREDENTIALW) {
    // SAFETY: caller upholds that `ptr` came from `CredReadW` and has not been freed.
    unsafe { CredFree(ptr.cast()) };
}

pub(super) fn write(service: &str, account: &str, secret: &[u8]) -> Result<(), CredentialError> {
    if secret.len() > CRED_MAX_CREDENTIAL_BLOB_SIZE as usize {
        return Err(CredentialError::Invalid);
    }
    let mut target = encode_wide(service, CRED_MAX_GENERIC_TARGET_NAME_LENGTH as usize)?;
    let mut user = encode_wide(account, CRED_MAX_USERNAME_LENGTH as usize)?;
    let blob = if secret.is_empty() { std::ptr::null_mut() } else { secret.as_ptr().cast_mut() };
    let credential = CREDENTIALW {
        Type: CRED_TYPE_GENERIC,
        TargetName: target.as_mut_ptr(),
        CredentialBlobSize: secret.len() as u32,
        CredentialBlob: blob,
        Persist: CRED_PERSIST_LOCAL_MACHINE,
        UserName: user.as_mut_ptr(),
        ..CREDENTIALW::default()
    };
    // SAFETY: every pointer in `credential` outlives the call, which only reads them.
    if unsafe { CredWriteW(&credential, 0) } == 0 {
        // SAFETY: reads this thread's last error code immediately after the failing Win32 call.
        let code = unsafe { GetLastError() };
        return Err(error_from_code("CredWriteW", code));
    }
    Ok(())
}

pub(super) fn read(service: &str) -> Result<Option<Credential>, CredentialError> {
    let target = encode_wide(service, CRED_MAX_GENERIC_TARGET_NAME_LENGTH as usize)?;
    let mut found: *mut CREDENTIALW = std::ptr::null_mut();
    // SAFETY: `target` is NUL-terminated; on success `found` points at a block owned by `CredGuard`.
    if unsafe { CredReadW(target.as_ptr(), CRED_TYPE_GENERIC, 0, &mut found) } == 0 {
        // SAFETY: reads this thread's last error code immediately after the failing Win32 call.
        let code = unsafe { GetLastError() };
        if code == ERROR_NOT_FOUND {
            return Ok(None);
        }
        return Err(error_from_code("CredReadW", code));
    }
    if found.is_null() {
        return Ok(None);
    }
    let guard = CredGuard::new(found, cred_free);
    let c = guard.as_ref();
    // SAFETY: `c` points into the single allocated block owned by `guard`, which outlives both
    // slices and wipes + frees the block on drop.
    let (account, secret) = unsafe {
        let secret = if c.CredentialBlob.is_null() || c.CredentialBlobSize == 0 {
            Vec::new()
        } else {
            std::slice::from_raw_parts(c.CredentialBlob, c.CredentialBlobSize as usize).to_vec()
        };
        let account = if c.UserName.is_null() {
            String::new()
        } else {
            let len = (0..).take_while(|&i| *c.UserName.add(i) != 0).count();
            String::from_utf16_lossy(std::slice::from_raw_parts(c.UserName, len))
        };
        (account, secret)
    };
    drop(guard);
    Ok(Some(Credential::new(account, secret)))
}

pub(super) fn delete(service: &str) -> Result<(), CredentialError> {
    let target = encode_wide(service, CRED_MAX_GENERIC_TARGET_NAME_LENGTH as usize)?;
    // SAFETY: `target` is NUL-terminated.
    if unsafe { CredDeleteW(target.as_ptr(), CRED_TYPE_GENERIC, 0) } == 0 {
        // SAFETY: reads this thread's last error code immediately after the failing Win32 call.
        let code = unsafe { GetLastError() };
        if code == ERROR_NOT_FOUND {
            return Ok(());
        }
        return Err(error_from_code("CredDeleteW", code));
    }
    Ok(())
}
