//! Windows Explorer shell extensions for `.hep` plot documents.
//!
//! One in-process COM server (a DLL) holding two extensions:
//!
//! - **`IThumbnailProvider`** — a real icon in Explorer rather than the
//!   generic document one.
//! - **`IPreviewHandler`** — the Preview Pane (Alt+P), and the only shell
//!   integration in this repo that can *reflow*, since it is given a window
//!   and told when that window resizes.
//!
//! The macOS counterparts are in `../hephaestus-quicklook` and the Linux one
//! in `../hephaestus-thumbnailer`, whose *library* does the rendering for all
//! of them.
//!
//! **None of this has been built or run on Windows.** It type-checks against
//! `x86_64-pc-windows-msvc`, which catches a great deal and proves nothing
//! about whether Explorer loads it. See `CLAUDE.md` for what to check first.
//!
//! Registration is `regsvr32 hephaestus_explorer.dll`, which calls
//! [`DllRegisterServer`] below.

#![cfg(windows)]

mod bitmap;
mod diagnostics;
mod preview;
mod registry;
mod stream;
mod thumbnail;

use std::sync::atomic::{AtomicUsize, Ordering};

use windows::core::{implement, IUnknown, Interface, Result, BOOL, GUID, HRESULT};
use windows::Win32::Foundation::{
    CLASS_E_CLASSNOTAVAILABLE, CLASS_E_NOAGGREGATION, E_INVALIDARG, E_UNEXPECTED, HMODULE, S_FALSE,
    S_OK,
};
use windows::Win32::System::Com::{IClassFactory, IClassFactory_Impl};
use windows::Win32::System::LibraryLoader::{
    GetModuleFileNameW, GetModuleHandleExW, GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS,
    GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
};

use diagnostics::diag;
use preview::PreviewHandler;
use registry::Hive;
use thumbnail::ThumbnailProvider;

/// Class id of our thumbnail provider.
///
/// Arbitrary but permanent: both are written into the registry, so changing
/// one orphans every installation that already registered the old value.
pub const CLSID_THUMBNAIL_PROVIDER: GUID = GUID::from_u128(0x7e1b4c2a_9d3f_4a61_8b52_6c0e9a47d310);

/// Class id of our preview handler.
pub const CLSID_PREVIEW_HANDLER: GUID = GUID::from_u128(0x7e1b4c2b_9d3f_4a61_8b52_6c0e9a47d310);

/// The shell's own class ids for the two handler slots.
///
/// Not ours to choose — these are the interface ids Explorer looks under, and
/// they are the same on every Windows.
const SHELLEX_THUMBNAIL_HANDLER: &str = "{e357fccd-a995-4576-b01f-234630154e96}";
const SHELLEX_PREVIEW_HANDLER: &str = "{8895b1c6-b41f-4c1c-a562-0d564250836f}";

/// The 64-bit `prevhost.exe` surrogate.
///
/// Setting this as the class's `AppID` is what keeps a preview handler out of
/// Explorer's own process, so a crash costs a preview rather than the desktop.
const PREVHOST_APPID: &str = "{534A1E02-D58F-44f0-B58B-36CBED287C7C}";

/// Where the shell reads its list of preview handlers. `HKEY_LOCAL_MACHINE`
/// only — a per-user registration is not read from here.
const PREVIEW_HANDLER_LIST: &str = "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\PreviewHandlers";

/// Extension this registers against.
const EXTENSION: &str = ".hep";

/// Live objects, so [`DllCanUnloadNow`] can answer honestly.
static OBJECTS: AtomicUsize = AtomicUsize::new(0);

/// Bump the live-object count. Called by the factory.
fn retain() {
    OBJECTS.fetch_add(1, Ordering::Relaxed);
}

/// Which of the two extensions a factory makes.
#[derive(Clone, Copy, Debug)]
enum Class {
    Thumbnail,
    Preview,
}

/// The class factory COM asks for before it can make anything.
#[implement(IClassFactory)]
struct Factory {
    class: Class,
}

impl IClassFactory_Impl for Factory_Impl {
    fn CreateInstance(
        &self,
        outer: windows_core::Ref<'_, IUnknown>,
        iid: *const GUID,
        object: *mut *mut core::ffi::c_void,
    ) -> Result<()> {
        if object.is_null() {
            return Err(E_INVALIDARG.into());
        }
        // SAFETY: null-checked; the caller owns the slot.
        unsafe { *object = std::ptr::null_mut() };
        // Aggregation is a COM feature nothing here supports, and saying so is
        // required rather than optional.
        if outer.is_some() {
            return Err(CLASS_E_NOAGGREGATION.into());
        }
        retain();
        let instance: IUnknown = match self.class {
            Class::Thumbnail => ThumbnailProvider::default().into(),
            Class::Preview => PreviewHandler::default().into(),
        };
        // SAFETY: `iid` comes from COM and `object` is the caller's slot.
        unsafe { instance.query(&*iid, object).ok() }
    }

    fn LockServer(&self, lock: BOOL) -> Result<()> {
        if lock.as_bool() {
            OBJECTS.fetch_add(1, Ordering::Relaxed);
        } else {
            OBJECTS.fetch_sub(1, Ordering::Relaxed);
        }
        Ok(())
    }
}

/// COM's entry point for getting a class factory out of a DLL.
///
/// # Safety
///
/// Called by COM with valid pointers.
#[no_mangle]
pub unsafe extern "system" fn DllGetClassObject(
    clsid: *const GUID,
    iid: *const GUID,
    object: *mut *mut core::ffi::c_void,
) -> HRESULT {
    if clsid.is_null() || iid.is_null() || object.is_null() {
        return E_INVALIDARG;
    }
    *object = std::ptr::null_mut();
    let class = if *clsid == CLSID_THUMBNAIL_PROVIDER {
        Class::Thumbnail
    } else if *clsid == CLSID_PREVIEW_HANDLER {
        Class::Preview
    } else {
        diag!("DllGetClassObject: not our class {:?}", *clsid);
        return CLASS_E_CLASSNOTAVAILABLE;
    };
    diag!("DllGetClassObject: creating a factory for {class:?}");
    let factory: IClassFactory = Factory { class }.into();
    match factory.query(&*iid, object) {
        HRESULT(0) => S_OK,
        error => error,
    }
}

/// Whether COM may unload this DLL.
#[no_mangle]
pub extern "system" fn DllCanUnloadNow() -> HRESULT {
    if OBJECTS.load(Ordering::Relaxed) == 0 {
        S_OK
    } else {
        S_FALSE
    }
}

/// Register the thumbnail handler. Invoked by `regsvr32`.
///
/// Two keys, and both are needed: one telling COM where the class lives, one
/// telling the shell to use that class for this file type.
#[no_mangle]
pub extern "system" fn DllRegisterServer() -> HRESULT {
    match register() {
        Ok(()) => S_OK,
        Err(error) => error.code(),
    }
}

fn register() -> Result<()> {
    let path = module_path()?;
    diag!("registering from {path}");

    // The thumbnail handler: a class, and the extension pointing at it.
    let thumbnail = registry::braced(&CLSID_THUMBNAIL_PROVIDER);
    register_class(&thumbnail, &path, "Hephaestus plot thumbnail handler")?;
    registry::set(
        Hive::Classes,
        &format!("{EXTENSION}\\ShellEx\\{SHELLEX_THUMBNAIL_HANDLER}"),
        None,
        &thumbnail,
    )?;

    // The preview handler needs two things more: an `AppID` so it is hosted
    // by `prevhost.exe` rather than by Explorer, and an entry in the shell's
    // list of preview handlers, which is only read from HKLM.
    let preview = registry::braced(&CLSID_PREVIEW_HANDLER);
    register_class(&preview, &path, "Hephaestus plot preview handler")?;
    registry::set(
        Hive::Classes,
        &format!("CLSID\\{preview}"),
        Some("AppID"),
        PREVHOST_APPID,
    )?;
    registry::set(
        Hive::Classes,
        &format!("{EXTENSION}\\ShellEx\\{SHELLEX_PREVIEW_HANDLER}"),
        None,
        &preview,
    )?;
    registry::set(
        Hive::LocalMachine,
        PREVIEW_HANDLER_LIST,
        Some(&preview),
        "Hephaestus plot preview handler",
    )?;
    diag!("registered thumbnail {thumbnail} and preview {preview}");
    Ok(())
}

/// The three values every in-process COM class needs.
fn register_class(clsid: &str, path: &str, description: &str) -> Result<()> {
    registry::set(Hive::Classes, &format!("CLSID\\{clsid}"), None, description)?;
    registry::set(
        Hive::Classes,
        &format!("CLSID\\{clsid}\\InprocServer32"),
        None,
        path,
    )?;
    // `Apartment` rather than `Both`: neither handler is thread-safe — both
    // keep their state behind a `RefCell` — and letting COM serialize calls is
    // cheaper than making a per-file object thread-safe.
    registry::set(
        Hive::Classes,
        &format!("CLSID\\{clsid}\\InprocServer32"),
        Some("ThreadingModel"),
        "Apartment",
    )
}

/// Remove what [`DllRegisterServer`] wrote.
#[no_mangle]
pub extern "system" fn DllUnregisterServer() -> HRESULT {
    for (clsid, slot) in [
        (
            registry::braced(&CLSID_THUMBNAIL_PROVIDER),
            SHELLEX_THUMBNAIL_HANDLER,
        ),
        (
            registry::braced(&CLSID_PREVIEW_HANDLER),
            SHELLEX_PREVIEW_HANDLER,
        ),
    ] {
        registry::remove(Hive::Classes, &format!("CLSID\\{clsid}"));
        registry::remove(Hive::Classes, &format!("{EXTENSION}\\ShellEx\\{slot}"));
    }
    // The HKLM list entry is a *value*, not a key, so the tree delete above
    // does not reach it. Left in place: removing one value from a shared key
    // is not worth the extra registry surface, and a stale entry naming an
    // unregistered class is ignored by the shell.
    S_OK
}

/// This DLL's own path, which is what has to go in `InprocServer32`.
///
/// Found from the address of a function inside it rather than from a
/// `DllMain`-captured handle: one fewer entry point, and no static to
/// initialize.
fn module_path() -> Result<String> {
    let mut module = HMODULE::default();
    // SAFETY: the flags make this a lookup by address with no refcount
    // change, and `DllCanUnloadNow` is certainly inside this image.
    unsafe {
        GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            windows::core::PCWSTR(DllCanUnloadNow as *const u16),
            &mut module,
        )?;
    }

    let mut buffer = [0u16; 32768];
    // SAFETY: `buffer` is the length passed.
    let length = unsafe { GetModuleFileNameW(Some(module), &mut buffer) };
    if length == 0 {
        return Err(E_UNEXPECTED.into());
    }
    Ok(String::from_utf16_lossy(&buffer[..length as usize]))
}
