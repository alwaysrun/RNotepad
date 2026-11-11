//! Embeds the executable icon and the version resource read by Explorer.
//!
//! `WM_SETICON` only changes the running window; Explorer, pinned shortcuts,
//! the file properties page and Task Manager read the resources stored in the
//! PE file, so both the icon and the `VS_VERSIONINFO` block are compiled here.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Icon asset next to the SVG sources.
const ICON_FILE: &str = "RNotepad.ico";
/// Relative path from this crate to the shared assets directory.
const ICON_PATH: &str = "../assets/icons/RNotepad.ico";
/// Resource identifier shared by the icon and the version resource; the
/// identifier only has to be unique per resource type.
const RESOURCE_ID: &str = "1";
/// Windows SDK resource compiler root.
const KITS_ROOT: &str = r"C:\Program Files (x86)\Windows Kits\10\bin";
/// Build layout of the resource compiler inside each SDK version folder.
const SDK_RC_SUBDIRECTORY: &str = "x64";
/// Generated resource script and object file inside `OUT_DIR`.
const RESOURCE_SCRIPT: &str = "rnotepad-resources.rc";
const RESOURCE_OBJECT: &str = "rnotepad-resources.res";

/// File description shown on the *Details* tab.
const FILE_DESCRIPTION: &str = "RNotepad - text, log and configuration workbench";
/// Product name shown on the *Details* tab.
const PRODUCT_NAME: &str = "RNotepad";
/// Vendor shown on the *Details* tab.
const COMPANY_NAME: &str = "alwaysrun";
/// Internal module name of the executable.
const INTERNAL_NAME: &str = "RNotepad";
/// Bundle file name produced by `scripts/build-crates.ps1`.
const ORIGINAL_FILENAME: &str = "rnotepad-app.exe";
/// Copyright notice; the project is distributed under GPL-3.0-only.
const LEGAL_COPYRIGHT: &str = "Copyright (C) 2026 alwaysrun. Licensed under GPL-3.0-only.";
/// `StringFileInfo` language/code-page block: en-US (0x0409) and UTF-16 (1200).
const LANGUAGE_BLOCK: &str = "040904B0";
/// Numeric language identifier used by `VarFileInfo`.
const LANGUAGE_ID: u32 = 0x0409;
/// Code page identifier used by `VarFileInfo`.
const CODE_PAGE_ID: u32 = 1200;
/// Numeric `VOS_NT_WINDOWS32` from `winver.h`; `rc.exe` does not predefine it.
const FILE_OS_NT_WINDOWS32: &str = "0x00040004L";
/// Numeric `VFT_APP` from `winver.h`; `rc.exe` does not predefine it.
const FILE_TYPE_APP: &str = "0x00000001L";

fn main() {
    println!("cargo:rerun-if-changed={ICON_PATH}");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("manifest directory"));
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("output directory"));
    let mut has_icon = manifest.join(ICON_PATH).is_file();
    if !has_icon {
        println!("cargo:warning={ICON_FILE} is missing; executable icon skipped");
    }
    if has_icon {
        if let Err(err) = std::fs::copy(manifest.join(ICON_PATH), out_dir.join(ICON_FILE)) {
            println!("cargo:warning=cannot copy {ICON_FILE}: {err}");
            has_icon = false;
        }
    }
    let Some(rc) = locate_resource_compiler() else {
        println!(
            "cargo:warning=rc.exe not found; executable icon and version information skipped"
        );
        return;
    };
    let script = out_dir.join(RESOURCE_SCRIPT);
    if let Err(err) = std::fs::write(&script, resource_script(has_icon)) {
        println!("cargo:warning=cannot write the resource script: {err}");
        return;
    }
    let resource = out_dir.join(RESOURCE_OBJECT);
    let status = Command::new(&rc)
        .current_dir(&out_dir)
        .arg("/nologo")
        .arg(format!("/fo{}", resource.display()))
        .arg(&script)
        .status();
    match status {
        Ok(status) if status.success() && resource.is_file() => {
            println!("cargo:rustc-link-arg-bins={}", resource.display());
        }
        Ok(status) => println!(
            "cargo:warning=rc.exe exited with {status}; executable resources skipped"
        ),
        Err(err) => println!("cargo:warning=cannot run rc.exe: {err}"),
    }
}

/// Renders the resource script containing the optional icon and the
/// version information consumed by the file properties *Details* tab.
fn resource_script(include_icon: bool) -> String {
    let version = std::env::var("CARGO_PKG_VERSION").expect("CARGO_PKG_VERSION is set by Cargo");
    let [major, minor, patch, build] = version_quad(&version);
    let icon = if include_icon {
        format!("{RESOURCE_ID} ICON \"{ICON_FILE}\"\n")
    } else {
        String::new()
    };
    format!(
        r#"{icon}{RESOURCE_ID} VERSIONINFO
FILEVERSION {major},{minor},{patch},{build}
PRODUCTVERSION {major},{minor},{patch},{build}
FILEOS {FILE_OS_NT_WINDOWS32}
FILETYPE {FILE_TYPE_APP}
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "{LANGUAGE_BLOCK}"
    BEGIN
      VALUE "CompanyName", "{COMPANY_NAME}"
      VALUE "FileDescription", "{FILE_DESCRIPTION}"
      VALUE "FileVersion", "{version}"
      VALUE "InternalName", "{INTERNAL_NAME}"
      VALUE "LegalCopyright", "{LEGAL_COPYRIGHT}"
      VALUE "OriginalFilename", "{ORIGINAL_FILENAME}"
      VALUE "ProductName", "{PRODUCT_NAME}"
      VALUE "ProductVersion", "{version}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", {LANGUAGE_ID:#06x}, {CODE_PAGE_ID}
  END
END
"#
    )
}

/// Converts a semver string such as `1.0.1` into the numeric quad expected by
/// `FILEVERSION`; pre-release and build suffixes are ignored.
fn version_quad(version: &str) -> [u32; 4] {
    let mut quad = [0u32; 4];
    let core = version.split(['-', '+']).next().unwrap_or(version);
    for (slot, part) in quad.iter_mut().zip(core.split('.')) {
        *slot = part
            .parse()
            .unwrap_or_else(|_| panic!("package version component {part:?} is not numeric"));
    }
    quad
}

/// Finds the newest x64 resource compiler in the Windows SDK.
fn locate_resource_compiler() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("RC_EXE") {
        let candidate = PathBuf::from(path);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    let mut versions: Vec<PathBuf> = std::fs::read_dir(KITS_ROOT)
        .ok()?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.is_dir())
        .collect();
    versions.sort_by_key(|path| version_key(path));
    versions
        .into_iter()
        .rev()
        .map(|version| version.join(SDK_RC_SUBDIRECTORY).join("rc.exe"))
        .find(|candidate| candidate.is_file())
}

/// Parses `10.0.26100.0` style folder names into comparable numbers.
fn version_key(path: &Path) -> Vec<u32> {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(|name| {
            name.split('.')
                .filter_map(|part| part.parse::<u32>().ok())
                .collect()
        })
        .unwrap_or_default()
}
