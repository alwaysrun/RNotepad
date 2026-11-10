//! Taskbar jump list shown when the taskbar button is right-clicked.
//!
//! Two application-managed categories are published: the files the user
//! pinned and the most recently opened files.  Pinning is tracked by the
//! application instead of the shell because Windows offers its own pin command
//! only for the known Recent category, which requires a registered file-type
//! association.  Every entry is a shortcut that relaunches this executable with
//! the file path as its only argument.

use std::ffi::OsStr;
use std::mem::size_of;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use windows::core::{GUID, IUnknown, Interface, PWSTR};
use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::System::Com::StructuredStorage::{PROPVARIANT, PropVariantClear};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
    CoTaskMemAlloc,
};
use windows::Win32::System::Variant::VT_LPWSTR;
use windows::Win32::UI::Shell::Common::{IObjectArray, IObjectCollection};
use windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore;
use windows::Win32::UI::Shell::{
    DestinationList, EnumerableObjectCollection, ICustomDestinationList, IShellLinkW, ShellLink,
    SetCurrentProcessExplicitAppUserModelID,
};

/// Taskbar identity shared by the process and its jump list.
const APP_USER_MODEL_ID: &str = "RNotepad.Application";
/// Category title for the files the user pinned.
const PINNED_CATEGORY: &str = "Pinned";
/// Category title for the most recently opened files.
const RECENT_CATEGORY: &str = "Recent Files";
/// `System.Title`, which jump-list entries use as their display text.
const TITLE_PROPERTY: PROPERTYKEY = PROPERTYKEY {
    fmtid: GUID::from_u128(0xf29f85e0_4ff9_1068_ab91_08002b27b3d9),
    pid: 2,
};

/// Initialises COM and assigns the taskbar identity.
///
/// The identity must be set before the main window is created so the shell
/// associates the window, and later its jump list, with this application.
pub fn initialise() -> Result<(), String> {
    // SAFETY: called once on the UI thread before any shell object exists.
    let result = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    if result.is_err() {
        // A different apartment model was already selected.  Shell calls may
        // still succeed, so the failure is logged rather than made fatal.
        tracing::warn!(error = %windows::core::Error::from(result), "COM initialisation failed");
    }
    let identifier = wide(OsStr::new(APP_USER_MODEL_ID));
    // SAFETY: the identifier buffer outlives the call; the shell copies it.
    unsafe { SetCurrentProcessExplicitAppUserModelID(PWSTR(identifier.as_ptr().cast_mut())) }
        .map_err(|err| format!("taskbar identity rejected: {err}"))
}

/// Publishes the pinned and recent files to the taskbar jump list.
///
/// Failures are returned so the caller can log them; the application keeps
/// running because the jump list is optional shell integration.
pub fn update(pinned: &[PathBuf], recent: &[PathBuf]) -> Result<(), String> {
    if pinned.is_empty() && recent.is_empty() {
        return Ok(());
    }
    // SAFETY: COM is initialised on this thread by [`initialise`].
    unsafe { publish(pinned, recent) }
}

/// Rebuilds both categories in a single destination-list transaction.
unsafe fn publish(pinned: &[PathBuf], recent: &[PathBuf]) -> Result<(), String> {
    // SAFETY: COM objects stay alive until the transaction ends.
    let list: ICustomDestinationList =
        unsafe { CoCreateInstance(&DestinationList, None::<&IUnknown>, CLSCTX_INPROC_SERVER) }
            .map_err(|err| format!("destination list unavailable: {err}"))?;
    let identifier = wide(OsStr::new(APP_USER_MODEL_ID));
    unsafe { list.SetAppID(PWSTR(identifier.as_ptr().cast_mut())) }
        .map_err(|err| format!("destination list identity rejected: {err}"))?;

    let mut minimum_slots: u32 = 0;
    // The shell reports destinations the user removed through the taskbar.
    // Application-managed pin state wins over that report, so it is discarded.
    let _removed: IObjectArray = unsafe { list.BeginList(&mut minimum_slots) }
        .map_err(|err| format!("destination list could not be opened: {err}"))?;

    let appended = append_categories(&list, pinned, recent);
    match appended {
        Ok(()) => unsafe { list.CommitList() }
            .map_err(|err| format!("destination list could not be committed: {err}")),
        Err(err) => {
            // SAFETY: an open transaction must be aborted so the previous list
            // stays in effect.
            let _ = unsafe { list.AbortList() };
            Err(err)
        }
    }
}

/// Adds the pinned and recent categories, skipping the empty ones.
unsafe fn append_categories(
    list: &ICustomDestinationList,
    pinned: &[PathBuf],
    recent: &[PathBuf],
) -> Result<(), String> {
    for (title, paths) in [(PINNED_CATEGORY, pinned), (RECENT_CATEGORY, recent)] {
        if paths.is_empty() {
            continue;
        }
        let items = unsafe { link_collection(paths) }?;
        let title = wide(OsStr::new(title));
        unsafe { list.AppendCategory(PWSTR(title.as_ptr().cast_mut()), &items) }
            .map_err(|err| format!("category could not be added: {err}"))?;
    }
    Ok(())
}

/// Builds a collection of shortcuts that reopen `paths` with this executable.
unsafe fn link_collection(paths: &[PathBuf]) -> Result<IObjectArray, String> {
    let executable = std::env::current_exe()
        .map_err(|err| format!("own executable path unavailable: {err}"))?;
    let executable = wide(executable.as_os_str());
    // SAFETY: the shell collection is created on the COM-initialised thread.
    let collection: IObjectCollection = unsafe {
        CoCreateInstance(
            &EnumerableObjectCollection,
            None::<&IUnknown>,
            CLSCTX_INPROC_SERVER,
        )
    }
    .map_err(|err| format!("shortcut collection unavailable: {err}"))?;

    for path in paths {
        // SAFETY: each shortcut owns its COM resources and is kept alive by
        // the collection after it is added.
        unsafe {
            let link: IShellLinkW =
                CoCreateInstance(&ShellLink, None::<&IUnknown>, CLSCTX_INPROC_SERVER)
                    .map_err(|err| format!("shortcut unavailable: {err}"))?;
            link.SetPath(PWSTR(executable.as_ptr().cast_mut()))
                .map_err(|err| format!("shortcut target rejected: {err}"))?;
            // Quoting keeps paths with spaces a single argument.
            let argument = wide(OsStr::new(&format!("\"{}\"", path.display())));
            link.SetArguments(PWSTR(argument.as_ptr().cast_mut()))
                .map_err(|err| format!("shortcut argument rejected: {err}"))?;
            set_title(&link, &file_label(path))?;
            collection
                .AddObject(&link)
                .map_err(|err| format!("shortcut could not be added: {err}"))?;
        }
    }
    collection
        .cast::<IObjectArray>()
        .map_err(|err| format!("shortcut collection could not be read: {err}"))
}

/// Sets `System.Title`, which jump-list entries use as their visible label.
unsafe fn set_title(link: &IShellLinkW, title: &str) -> Result<(), String> {
    let store: IPropertyStore = link
        .cast()
        .map_err(|err| format!("shortcut properties unavailable: {err}"))?;
    let text = wide(OsStr::new(title));
    // The property store copies the string, but the source buffer must still be
    // COM-allocated so `PropVariantClear` can release it below.
    let buffer = unsafe { CoTaskMemAlloc(text.len() * size_of::<u16>()) } as *mut u16;
    if buffer.is_null() {
        return Err("shortcut title allocation failed".to_owned());
    }
    unsafe {
        std::ptr::copy_nonoverlapping(text.as_ptr(), buffer, text.len());
    }
    let mut value = PROPVARIANT::default();
    unsafe {
        (*value.Anonymous.Anonymous).vt = VT_LPWSTR;
        (*value.Anonymous.Anonymous).Anonymous.pwszVal = PWSTR(buffer);
    }
    let stored = unsafe { store.SetValue(&TITLE_PROPERTY, &value) }
        .map_err(|err| format!("shortcut title rejected: {err}"));
    // SAFETY: the value is copied by `SetValue`, so the local copy is released
    // on both paths and no buffer leaks.
    unsafe {
        let _ = PropVariantClear(&mut value);
    }
    stored?;
    unsafe { store.Commit() }.map_err(|err| format!("shortcut title not committed: {err}"))
}

/// Label shown for one file; the file name is enough to identify it.
fn file_label(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

/// NUL-terminated UTF-16 copy of `text` for Win32 APIs.
fn wide(text: &OsStr) -> Vec<u16> {
    text.encode_wide().chain(std::iter::once(0)).collect()
}

