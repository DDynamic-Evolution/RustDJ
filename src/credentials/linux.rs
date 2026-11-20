//! Secret Service access through the system libsecret.
use std::{
    ffi::{CStr, CString, c_char, c_int, c_void},
    ptr::{null, null_mut},
};

#[link(name = "libsecret-1.so.0", kind = "dylib", modifiers = "+verbatim")]
unsafe extern "C" {
    fn secret_schema_new(name: *const c_char, flags: c_int, ...) -> *mut c_void;
    fn secret_schema_unref(schema: *mut c_void);
    fn secret_password_lookup_sync(
        schema: *const c_void,
        cancellable: *mut c_void,
        error: *mut *mut c_void,
        ...
    ) -> *mut c_char;
    fn secret_password_store_sync(
        schema: *const c_void,
        collection: *const c_char,
        label: *const c_char,
        password: *const c_char,
        cancellable: *mut c_void,
        error: *mut *mut c_void,
        ...
    ) -> c_int;
    fn secret_password_clear_sync(
        schema: *const c_void,
        cancellable: *mut c_void,
        error: *mut *mut c_void,
        ...
    ) -> c_int;
    fn secret_password_free(password: *mut c_char);
}

#[link(name = "libglib-2.0.so.0", kind = "dylib", modifiers = "+verbatim")]
unsafe extern "C" {
    fn g_error_free(error: *mut c_void);
}

const KEYRING_ERROR: &str = "Could not access the system keyring. Open or unlock it and try again.";
const STRING_ATTRIBUTE: c_int = 0;
const ATTRIBUTE: &CStr = c"credential";
fn entry(id: u64) -> CString {
    // The first profile owns the entry created by RustDJ 0.23.3 and 0.23.4.
    let name = if id == 0 {
        "shoutcast-source".to_owned()
    } else {
        format!("shoutcast-source-{id}")
    };
    CString::new(name).expect("keyring entry contains only ASCII without NUL")
}

struct Schema(*mut c_void);

impl Schema {
    fn new() -> Result<Self, String> {
        // Use the default schema flags and a single string attribute.
        let schema = unsafe {
            secret_schema_new(
                c"io.github.DDynamicEvolution.RustDJ".as_ptr(),
                0,
                ATTRIBUTE.as_ptr(),
                STRING_ATTRIBUTE,
                null::<c_char>(),
            )
        };
        if schema.is_null() {
            return Err(KEYRING_ERROR.into());
        }
        Ok(Self(schema))
    }
}

impl Drop for Schema {
    fn drop(&mut self) {
        unsafe { secret_schema_unref(self.0) };
    }
}

fn check_error(error: *mut c_void) -> Result<(), String> {
    if error.is_null() {
        return Ok(());
    }
    // Keep provider diagnostics out of the UI and free the owned GError.
    unsafe { g_error_free(error) };
    Err(KEYRING_ERROR.into())
}

pub fn load(id: u64) -> Result<String, String> {
    let entry = entry(id);
    let schema = Schema::new()?;
    let mut error = null_mut();
    // Libsecret owns the returned password allocation until secret_password_free.
    let password = unsafe {
        secret_password_lookup_sync(
            schema.0,
            null_mut(),
            &mut error,
            ATTRIBUTE.as_ptr(),
            entry.as_ptr(),
            null::<c_char>(),
        )
    };
    check_error(error)?;
    if password.is_null() {
        return Ok(String::new());
    }
    let result = unsafe { CStr::from_ptr(password) }
        .to_str()
        .map(str::to_owned)
        .map_err(|_| "The saved stream password has an invalid format.".to_owned());
    unsafe { secret_password_free(password) };
    result
}

pub fn save(id: u64, password: String) -> Result<(), String> {
    if password.is_empty() || password.chars().any(char::is_control) {
        return Err("Enter a valid source password.".into());
    }
    let password =
        CString::new(password).map_err(|_| "Enter a valid source password.".to_owned())?;
    let entry = entry(id);
    let schema = Schema::new()?;
    let mut error = null_mut();
    let saved = unsafe {
        secret_password_store_sync(
            schema.0,
            c"default".as_ptr(),
            c"RustDJ SHOUTcast source password".as_ptr(),
            password.as_ptr(),
            null_mut(),
            &mut error,
            ATTRIBUTE.as_ptr(),
            entry.as_ptr(),
            null::<c_char>(),
        )
    };
    check_error(error)?;
    if saved == 0 {
        return Err(KEYRING_ERROR.into());
    }
    Ok(())
}

pub fn clear(id: u64) -> Result<(), String> {
    let entry = entry(id);
    let schema = Schema::new()?;
    let mut error = null_mut();
    let removed = unsafe {
        secret_password_clear_sync(
            schema.0,
            null_mut(),
            &mut error,
            ATTRIBUTE.as_ptr(),
            entry.as_ptr(),
            null::<c_char>(),
        )
    };
    check_error(error)?;
    if removed == 0 {
        return Err(
            "No password was removed. If the keyring is locked, unlock it and try again.".into(),
        );
    }
    Ok(())
}
