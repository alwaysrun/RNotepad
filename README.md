# RNotepad

A lightweight Windows text / log / configuration workbench written in Rust.
RNotepad pairs a pure-Rust data engine with a thin Win32 shell and a vendored
Scintilla 5.6.9 editing surface, so it can open ordinary text files for light
editing and inspect multi-gigabyte logs or raw bytes without loading them into
memory.

[![License: GPL v3](https://img.shields.io/badge/License-GPLv3-blue.svg)](LICENSE)
![Platform: Windows 10/11 x64](https://img.shields.io/badge/Platform-Windows%2010%2F11%20x64-lightgrey)
![Rust 1.85+](https://img.shields.io/badge/Rust-1.85%2B-orange)

## Features

- **Text and Hex views of the same document** — switch without reopening or
  re-decoding; `Text + Hex` shows both panes side by side with synchronized
  navigation and byte highlighting of BOM, EOL and multi-byte sequences.
- **Large-file viewing** — files above the 16 MiB editing threshold open
  read-only through an mmap-backed, virtualized line view with *Follow Tail*
  for logs that are still growing.
- **Encoding and line-ending awareness** — detects UTF-8, UTF-16 LE/BE,
  GB18030, GBK, Big5, Shift-JIS, Windows-1252 and Latin-1, tracks BOM state and
  CRLF/LF/CR statistics, and converts encoding or EOLs when the document is
  editable.
- **Find and replace** — literal search with case and wrap options, *Find All*
  into a results panel, *Find Next* (F3) navigation, cancellable background
  searches, and separate history for search and replacement text.
- **JSON tooling** — format, minify and validate JSON with a problem panel that
  locates the first error by line and column; a failed operation never replaces
  the original buffer.
- **Desktop integration** — recent files (pinnable in the taskbar jump list),
  external-change detection with reload, status bar showing encoding, EOL,
  size and caret position, and optional file logging.
- **Portable bundle** — the runnable application is an executable plus
  `Scintilla.dll` and its license file; no installer and no registry writes.
  Per-user data (recent files, find/replace history, optional logs) lives under
  `%APPDATA%\RNotepad` or next to the executable for logs.

## Requirements

- Windows 10 or Windows 11, x64
- Rust 1.85 or newer with the MSVC toolchain
- PowerShell 7+
- `third_party/Scintilla.dll` (tracked in the repository) — regenerate it with
  `scripts/build-scintilla.ps1` only when upgrading Scintilla; that script needs
  the Visual Studio C++ build tools, while `cargo build` itself stays pure Rust.

## Build and run

```powershell
# Release build, assembles the runnable bundle in output\
pwsh -File scripts/build-crates.ps1

# Debug build
pwsh -File scripts/build-crates.ps1 -Profile Debug
```

The bundle contains:

```
output\rnotepad-app.exe
output\Scintilla.dll
output\License.txt   # Scintilla / Lexilla / SciTE notice
```

For development, `cargo run -p rnotepad-app` works directly: the Scintilla DLL
is located under `third_party\` relative to the executable. Run the test suite
with `cargo test --workspace`.

## Keyboard shortcuts

| Shortcut | Action |
| --- | --- |
| `Ctrl+N` / `Ctrl+O` | New / open document |
| `Ctrl+S` | Save |
| `Ctrl+R` | Reload from disk |
| `Ctrl+Z` / `Ctrl+Y` | Undo / redo |
| `Ctrl+X` / `Ctrl+C` / `Ctrl+V` | Cut / copy / paste |
| `Ctrl+F` / `Ctrl+H` | Find / replace |
| `F3` | Find next |

## Project layout

```
crates/rnotepad-core   Pure-Rust data engine: bytes, encoding, EOL, line index,
                       coordinates, search, formatting, document model
crates/rnotepad-win    Win32 shell and custom views: Scintilla host, large-file
                       view, hex view, results list, status bar, save/notify
crates/rnotepad-app    Application: message loop, main window, commands,
                       session state and background tasks
vendor/scintilla-5.6.9 Vendored Scintilla source, pinned and not tracked upstream
third_party            Prebuilt Scintilla.dll and its license
scripts                Build helpers (bundle assembly, Scintilla, constants)
docs                   Implementation specification
```

The dependency direction is one-way: `core` is free of OS and UI types, `win`
does not depend on `app`, and `app` depends on both.

## Documentation

- [Implementation specification](docs/RNotepad-spec.md) — scope, data model,
  subsystem rules and acceptance criteria (Chinese).

## Third-party notices

RNotepad loads a DLL built from [Scintilla](https://www.scintilla.org/), which is
distributed under the Historical Permission Notice and Disclaimer (HPND). The
original notice is kept in `third_party/License.txt` and shipped with every
release bundle.

## License

RNotepad is licensed under the GNU General Public License v3.0. See
[LICENSE](LICENSE) for the full text.
