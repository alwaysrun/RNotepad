# RNotepad

A lightweight Windows text / log / configuration workbench written in Rust.
RNotepad pairs a pure-Rust data engine with a thin Win32 shell and a vendored
Scintilla 5.6.9 editing surface, so it can open ordinary text files for light
editing and inspect multi-gigabyte logs or raw bytes without loading them into
memory.
<img width="832" height="560" alt="RNotepad-gui" src="https://github.com/user-attachments/assets/54cb8303-46e1-45e5-823a-8df50069db8a" />


[![License: GPL v3](https://img.shields.io/badge/License-GPLv3-blue.svg)](LICENSE)
![Platform: Windows 10/11 x64](https://img.shields.io/badge/Platform-Windows%2010%2F11%20x64-lightgrey)
![Rust 1.85+](https://img.shields.io/badge/Rust-1.85%2B-orange)

## Features

- **Text and Hex views of the same document** — switch without reopening or
  re-decoding; `Text + Hex` shows both panes side by side, and a selection in
  either pane reveals the matching text or bytes. The hex view highlights BOM,
  EOL and multi-byte sequences.
- **Large-file viewing** — files above the 16 MiB editing threshold open
  read-only through an mmap-backed, virtualized line view with *Follow Tail*
  for logs that are still growing. Files that cannot be decoded as text open
  read-only in the Hex view.
- **Encoding and line-ending awareness** — detects UTF-8, UTF-16 LE/BE,
  GB18030, GBK, Big5, Shift-JIS, Windows-1252 and Latin-1, tracks BOM state and
  mixed CRLF/LF/CR statistics. *Encoding* reinterprets the bytes, *Convert*
  chooses the encoding written on save, and BOM and EOL have their own menus.
- **Find and replace** — literal search with case, whole-word and wrap options;
  *Find All* appends a collapsible result group to the results panel; *Find
  Next* (F3) walks the matches; replace one or replace all. Background searches
  are cancellable and report progress, and search and replacement text keep
  separate persistent histories.
- **Expression search** — tick *Expr* to combine terms with `AND`, `OR`, `NOT`
  and parentheses; adjacent terms mean `AND`. `re:` terms are regular
  expressions and `hex:` terms match raw bytes, all scoped to one logical line.
- **JSON and XML config tooling** — format, minify and validate JSON and XML
  configuration files (`.json`, `.xml`, `.config`, `.csproj`, `.props`,
  `.targets`, `.nuspec`, `.plist`, `.resx`, `.xaml`). Problems are located by
  line and column in the problem panel; a failed operation never replaces the
  buffer, and a successful format is a single undo step.
- **Desktop integration** — recent files with taskbar jump-list pinning,
  drag-and-drop, single-instance hand-off (a second launch with a file opens
  it in the running window), *Open Containing Folder*, external-change
  detection with reload, persisted window placement, font size and word-wrap
  state, and optional file logging toggled from the Help menu.
- **Editing aids** — *Enable Editing* toggles a document read-only, *Word Wrap*
  and font-size commands adjust the text view, and the status bar reports view
  mode, encoding confidence, EOL statistics, size, caret or selection,
  document flags, byte offset and background progress.
- **Portable bundle** — the runnable application is an executable plus
  `Scintilla.dll` and its license file; no installer and no registry writes.
  `recent.json`, `history.json`, `settings.json` and optional logs are stored
  beside the executable, so the whole folder can be moved as one unit.

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
| `Ctrl+A` | Select all (editor focus) |
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
crates/assets          Application icon
vendor/scintilla-5.6.9 Vendored Scintilla source, pinned and not tracked upstream
third_party            Prebuilt Scintilla.dll and its license
scripts                Build helpers (bundle assembly, Scintilla, constants)
docs                   Implementation specification and XML format plan
```

The dependency direction is one-way: `core` is free of OS and UI types, `win`
does not depend on `app`, and `app` depends on both.

## Documentation

- [Implementation specification](docs/RNotepad-spec.md) — scope, data model,
  subsystem rules and acceptance criteria (Chinese).
- [XML configuration format](docs/xml-config-format.md) — design of the XML
  format, minify and validate provider (Chinese).

## Third-party notices

RNotepad loads a DLL built from [Scintilla](https://www.scintilla.org/), which is
distributed under the Historical Permission Notice and Disclaimer (HPND). The
original notice is kept in `third_party/License.txt` and shipped with every
release bundle.

## License

RNotepad is licensed under the GNU General Public License v3.0. See
[LICENSE](LICENSE) for the full text.
