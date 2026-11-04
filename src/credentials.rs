//! Password persistence protected by Windows DPAPI for the current user.
const MARKER: &[u8] = b"RustDJ source password v1\0";

pub fn protect(password: &str) -> Result<Vec<u8>, String> {
    if password.is_empty() {
        return Ok(Vec::new());
    }
    let mut clear = Vec::from(MARKER);
    clear.extend_from_slice(password.as_bytes());
    let result = transform(&clear, true);
    for byte in &mut clear {
        // Do not leave an extra temporary plaintext copy in the allocator.
        unsafe {
            std::ptr::write_volatile(byte, 0);
        }
    }
    result
}

pub fn unprotect(blob: &[u8]) -> Result<String, String> {
    if blob.is_empty() {
        return Ok(String::new());
    }
    let mut clear = transform(blob, false)?;
    let result = clear
        .strip_prefix(MARKER)
        .ok_or_else(|| "The saved stream password has an invalid format.".to_owned())
        .and_then(|bytes| {
            String::from_utf8(bytes.to_vec())
                .map_err(|_| "The saved stream password is corrupted.".to_owned())
        });
    for byte in &mut clear {
        unsafe {
            std::ptr::write_volatile(byte, 0);
        }
    }
    result
}

#[cfg(windows)]
fn transform(data: &[u8], encrypt: bool) -> Result<Vec<u8>, String> {
    use windows_sys::Win32::{
        Foundation::LocalFree,
        Security::Cryptography::{
            CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
        },
    };
    if data.len() > 1_048_576 {
        return Err("The saved stream password is too large.".into());
    }
    let input = CRYPT_INTEGER_BLOB {
        cbData: data.len() as u32,
        pbData: data.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    // DPAPI reads the input and allocates output with LocalAlloc. No UI or
    // machine-wide scope is requested; only this Windows user can restore it.
    let succeeded = unsafe {
        if encrypt {
            CryptProtectData(
                &input,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        } else {
            CryptUnprotectData(
                &input,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        }
    };
    if succeeded == 0 {
        return Err(format!(
            "Windows could not {} the stream password: {}",
            if encrypt { "protect" } else { "restore" },
            std::io::Error::last_os_error()
        ));
    }
    if output.pbData.is_null() {
        return Err("Windows returned no password data.".into());
    }
    // The successful API call owns a valid cbData-byte allocation until LocalFree.
    let bytes = unsafe { std::slice::from_raw_parts_mut(output.pbData, output.cbData as usize) };
    let result = bytes.to_vec();
    if !encrypt {
        for byte in bytes {
            unsafe {
                std::ptr::write_volatile(byte, 0);
            }
        }
    }
    unsafe {
        LocalFree(output.pbData.cast());
    }
    Ok(result)
}

#[cfg(not(windows))]
fn transform(_data: &[u8], _encrypt: bool) -> Result<Vec<u8>, String> {
    Err("Protected storage of stream passwords requires Windows.".into())
}
