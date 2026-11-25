//! Credential Manager: one generic credential per service, its target name the service. `CredWriteW`
//! replaces it whole or not at all.
#![allow(unsafe_code)]

use windows_sys::Win32::Foundation::{ERROR_NOT_FOUND, GetLastError};
use windows_sys::Win32::Security::Credentials::{
    CRED_MAX_CREDENTIAL_BLOB_SIZE, CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC, CREDENTIALW, CredDeleteW, CredFree,
    CredReadW, CredWriteW,
};

use super::{Credential, CredentialError};

fn wide(s: &str) -> Vec<u16> { s.encode_utf16().chain(Some(0)).collect() }

fn failed(call: &str) -> CredentialError {
    // SAFETY: reads this thread's last error code.
    let code = unsafe { GetLastError() };
    CredentialError::Failed(format!("{call} failed ({code})"))
}

pub(super) fn write(service: &str, account: &str, secret: &[u8]) -> Result<(), CredentialError> {
    if secret.len() > CRED_MAX_CREDENTIAL_BLOB_SIZE as usize { return Err(CredentialError::Invalid) }
    let mut target = wide(service);
    let mut user = wide(account);
    let credential = CREDENTIALW {
        Type: CRED_TYPE_GENERIC,
        TargetName: target.as_mut_ptr(),
        CredentialBlobSize: secret.len() as u32,
        CredentialBlob: secret.as_ptr().cast_mut(),
        Persist: CRED_PERSIST_LOCAL_MACHINE,
        UserName: user.as_mut_ptr(),
        ..CREDENTIALW::default()
    };
    // SAFETY: every pointer in `credential` outlives the call, which only reads them.
    if unsafe { CredWriteW(&credential, 0) } == 0 { return Err(failed("CredWriteW")) }
    Ok(())
}

pub(super) fn read(service: &str) -> Result<Option<Credential>, CredentialError> {
    let target = wide(service);
    let mut found: *mut CREDENTIALW = std::ptr::null_mut();
    // SAFETY: `target` is NUL-terminated; on success `found` points at a block freed below.
    if unsafe { CredReadW(target.as_ptr(), CRED_TYPE_GENERIC, 0, &mut found) } == 0 {
        // SAFETY: reads this thread's last error code.
        if unsafe { GetLastError() } == ERROR_NOT_FOUND { return Ok(None) }
        return Err(failed("CredReadW"));
    }
    // SAFETY: `found` is the valid credential CredReadW returned, ours to write until it is freed,
    // once, after its secret is copied and wiped.
    let credential = unsafe {
        let c = &*found;
        let secret = if c.CredentialBlob.is_null() {
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
        if !c.CredentialBlob.is_null() { std::ptr::write_bytes(c.CredentialBlob, 0, c.CredentialBlobSize as usize) }
        CredFree(found.cast());
        Credential::new(account, secret)
    };
    Ok(Some(credential))
}

pub(super) fn delete(service: &str) -> Result<(), CredentialError> {
    let target = wide(service);
    // SAFETY: `target` is NUL-terminated.
    if unsafe { CredDeleteW(target.as_ptr(), CRED_TYPE_GENERIC, 0) } == 0 {
        // SAFETY: reads this thread's last error code.
        if unsafe { GetLastError() } == ERROR_NOT_FOUND { return Ok(()) }
        return Err(failed("CredDeleteW"));
    }
    Ok(())
}
