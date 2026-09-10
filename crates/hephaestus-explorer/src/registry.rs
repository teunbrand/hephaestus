//! Self-registration, so `regsvr32` is all an installer has to run.

use windows::core::{Result, GUID, PCWSTR};
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegSetValueExW, HKEY, HKEY_CLASSES_ROOT,
    HKEY_LOCAL_MACHINE, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ,
};

/// Where a key lives.
///
/// Class registration goes under `HKEY_CLASSES_ROOT`, which is the convention
/// `regsvr32` expects; the preview-handler *list* is only read from
/// `HKEY_LOCAL_MACHINE`. Both need administrator rights — see `CLAUDE.md` for
/// the per-user alternative.
#[derive(Clone, Copy)]
pub enum Hive {
    Classes,
    LocalMachine,
}

impl Hive {
    fn key(self) -> HKEY {
        match self {
            Self::Classes => HKEY_CLASSES_ROOT,
            Self::LocalMachine => HKEY_LOCAL_MACHINE,
        }
    }
}

/// Write `value` into `subkey`.
///
/// `name` is `None` for a key's default value, which is how most of the keys
/// a shell extension needs are written.
pub fn set(hive: Hive, subkey: &str, name: Option<&str>, value: &str) -> Result<()> {
    let subkey = wide(subkey);
    let mut key = HKEY::default();
    // SAFETY: `subkey` is a NUL-terminated wide string and `key` receives the
    // handle, which is closed below.
    let status = unsafe {
        RegCreateKeyExW(
            hive.key(),
            PCWSTR(subkey.as_ptr()),
            Some(0),
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            None,
            &mut key,
            None,
        )
    };
    if status != ERROR_SUCCESS {
        return Err(windows::core::Error::from(status.to_hresult()));
    }

    let data = wide(value);
    // The length is in *bytes* and includes the terminator; a value written
    // without it reads back truncated in some consumers.
    let bytes = unsafe { std::slice::from_raw_parts(data.as_ptr().cast::<u8>(), data.len() * 2) };
    let name = name.map(wide);
    let result = unsafe {
        RegSetValueExW(
            key,
            name.as_ref()
                .map(|n| PCWSTR(n.as_ptr()))
                .unwrap_or(PCWSTR::null()),
            Some(0),
            REG_SZ,
            Some(bytes),
        )
    };
    unsafe { RegCloseKey(key).ok()? };
    if result != ERROR_SUCCESS {
        return Err(windows::core::Error::from(result.to_hresult()));
    }
    Ok(())
}

/// Remove a key and everything under it, ignoring a key that is not there.
pub fn remove(hive: Hive, subkey: &str) {
    let subkey = wide(subkey);
    // SAFETY: NUL-terminated wide string; failure is deliberately ignored,
    // since unregistering something already absent is success.
    unsafe {
        let _ = RegDeleteTreeW(hive.key(), PCWSTR(subkey.as_ptr()));
    }
}

/// A GUID in the `{XXXXXXXX-XXXX-...}` form the registry uses.
pub fn braced(guid: &GUID) -> String {
    format!("{{{guid:?}}}")
}

/// NUL-terminated UTF-16, which is what every `…W` entry point wants.
fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}
