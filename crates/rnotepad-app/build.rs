//! Embeds the executable icon compiled from the SVG-derived `RNotepad.ico`.
//!
//! `WM_SETICON` only changes the running window; Explorer, pinned shortcuts and
//! the file properties page read the icon resource stored in the PE file.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Icon asset next to the SVG sources.
const ICON_FILE: &str = "RNotepad.ico";
/// Relative path from this crate to the shared assets directory.
const ICON_PATH: &str = "../assets/icons/RNotepad.ico";
/// Resource identifier used inside the generated `.rc` file.
const RESOURCE_ID: &str = "1";
/// Windows SDK resource compiler root.
const KITS_ROOT: &str = r"C:\Program Files (x86)\Windows Kits\10\bin";
/// Build layout of the resource compiler inside each SDK version folder.
const SDK_RC_SUBDIRECTORY: &str = "x64";

fn main() {
    println!("cargo:rerun-if-changed={ICON_PATH}");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("manifest directory"));
    if !manifest.join(ICON_PATH).is_file() {
        println!("cargo:warning={ICON_FILE} is missing; executable icon skipped");
        return;
    }
    let Some(rc) = locate_resource_compiler() else {
        println!("cargo:warning=rc.exe not found; executable icon skipped");
        return;
    };
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("output directory"));
    if let Err(err) = std::fs::copy(manifest.join(ICON_PATH), out_dir.join(ICON_FILE)) {
        println!("cargo:warning=cannot copy {ICON_FILE}: {err}");
        return;
    }
    let script = out_dir.join("rnotepad-icon.rc");
    if let Err(err) = std::fs::write(&script, format!("{RESOURCE_ID} ICON \"{ICON_FILE}\"")) {
        println!("cargo:warning=cannot write the resource script: {err}");
        return;
    }
    let resource = out_dir.join("rnotepad-icon.res");
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
            "cargo:warning=rc.exe exited with {status}; executable icon skipped"
        ),
        Err(err) => println!("cargo:warning=cannot run rc.exe: {err}"),
    }
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
