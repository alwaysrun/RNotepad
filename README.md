# RNotepad

A lightweight, portable Windows text, log and configuration workbench written in
Rust. RNotepad pairs a pure-Rust data engine with a thin Win32 shell and a
vendored Scintilla 5.6.9 editing surface. Small files open in an editable text
view; multi-gigabyte logs and raw bytes open read-only without loading the whole
file into memory.

<img width="832" height="560" alt="RNotepad-gui" src="https://github.com/user-attachments/assets/1f7d6d66-348f-4e63-acc6-9c40a9ad569b" />


[![License: GPL v3](https://img.shields.io/badge/License-GPLv3-blue.svg)](LICENSE)
![Platform: Windows 10/11 x64](https://img.shields.io/badge/Platform-Windows%2010%2F11%20x64-lightgrey)
![Rust 1.85+](https://img.shields.io/badge/Rust-1.85%2B-orange)

## Features

- **One document, two views** — Text and Hex are two views of the same bytes;
  switch without reopening or re-decoding. *Text + Hex* shows both panes side by
  side with a draggable divider, and the caret or selection in one pane is
  mirrored in the other. The hex view highlights BOM, EOL bytes, search hits and
  multi-byte sequences.
- **Large files and binaries** — files above the 16 MiB editing threshold open
  read-only through a memory-mapped, virtualized text view with *Follow Tail*
  for logs that are still growing. A file that cannot be decoded as text opens
  read-only in the Hex view instead.
- **Encoding and line endings** — detects UTF-8, UTF-16 LE/BE, GB18030, GBK,
  Big5, Shift-JIS and Windows-1252 by BOM, UTF-8 validity, a UTF-16 heuristic,
  `chardetng` and a final fallback; Latin-1 is available for manual
  reinterpretation. Tracks BOM state and mixed CRLF/LF/CR counts. Choosing an
  entry in the *Encoding* menu reinterprets the current bytes and sets the
  charset written on save; the *EOL* menu converts the buffer immediately and
  keeps that choice on save; BOM has its own Keep / Add / Strip entries.
- **Find, expression search and replace** — literal search with Case, Word and
  Wrap options. *Find All* streams matches into collapsible groups in the
  results panel; double-click a row to jump to that match. Background searches
  are cancellable and report progress in the status bar. Tick *Expr* to combine
  terms with `AND`, `OR`, `NOT` and parentheses (adjacent terms mean `AND`);
  `re:` terms are regular expressions, `hex:` terms match raw bytes, and every
  term is scoped to one logical line. Replace swaps the current match and
  Replace All rewrites every match. Pattern and replacement text keep separate
  persistent histories.
- **Text tools** — Remove Blank Lines, Trim Leading Whitespace, Trim Trailing
  Whitespace, Tabs to Spaces and Spaces to Tabs. A non-empty selection is
  transformed in place; otherwise the whole buffer is rewritten, both as a
  single undo step.
- **JSON and XML config tooling** — Format, Minify and Validate JSON and XML
  configuration files (`.json`, `.xml`, `.config`, `.csproj`, `.props`,
  `.targets`, `.nuspec`, `.plist`, `.resx`, `.xaml`). Problems are reported with
  line and column in the problem panel; a failed operation never replaces the
  buffer, and a successful format is one undo step. The same actions appear in
  the editor context menu for a detected document.
- **Desktop integration** — recent files (up to 10) with taskbar jump-list
  pinning, drag-and-drop, second-launch hand-off (reuses the window that already
  owns the file or an untouched window), Open Containing Folder,
  external-change detection with title and status markers plus reload, persisted
  window placement, font size and word-wrap state, and optional file logging
  toggled from the Help menu.
- **Editing aids** — editable files open in an editable text view; *Enable
  Editing* toggles it read-only, while large and binary documents stay read-only.
  Enter keeps the previous line's indentation while editing. The status bar
  reports view mode, encoding (plus BOM and confidence when it is low), EOL
  statistics, size, caret or selection, document flags, byte offset and
  background progress.
- **Portable bundle** — the runnable application is an executable plus
  `Scintilla.dll` and its license file; no installer and no registry writes.
  Preferences, history and optional logs are stored beside the executable, so
  the whole folder can be moved as one unit. Saves are written to a temporary
  file and swapped in, so a failed save leaves the original intact.

## Requirements

- Windows 10 or Windows 11, x64
- Rust 1.85 or newer with the MSVC toolchain
- PowerShell 7+
- `third_party/Scintilla.dll` (tracked in the repository). Regenerate it with
  `scripts/build-scintilla.ps1` only when upgrading Scintilla; that script needs
  the Visual Studio C++ build tools, while `cargo build` itself stays pure Rust.

## Build and run

```powershell
# Release build, assembles the runnable bundle in output\
pwsh -File scripts/build-crates.ps1

# Debug build
pwsh -File scripts/build-crates.ps1 -Profile Debug

# Optional: clean the selected profile first
pwsh -File scripts/build-crates.ps1 -Clean
```

The bundle contains:

```
output\rnotepad-app.exe
output\Scintilla.dll
output\License.txt   # Scintilla / Lexilla / SciTE notice
```

For development, `cargo run -p rnotepad-app` works directly: the Scintilla DLL
is searched for next to the executable and under `third_party\` in its ancestor
directories. Run the test suite with `cargo test --workspace`.

The build also embeds the application icon and version resource with the Windows
SDK `rc.exe` when one is installed; without it the build warns and still
succeeds.

## Using RNotepad

The window opens with an untitled editable buffer. Files can be opened from
*File > Open*, the recent-files list, a command-line path or by dropping them on
the window.

| Menu | Entries |
| --- | --- |
| File | New, Open, Recent Files, Pin Current File to Taskbar List, Save, Save As, Reload, Open Containing Folder, Close, Exit |
| Edit | Undo, Redo, Cut, Copy, Paste, Enable Editing, Find, Replace, Find Next, Find Previous, Find All, Cancel Search |
| View | Text, Hex, Text + Hex, Text + Results, Word Wrap, Increase/Decrease/Reset Font Size, Follow Tail, Float Results Panel |
| Tools | Remove Blank Lines, Trim Leading Whitespace, Trim Trailing Whitespace, Tabs to Spaces, Spaces to Tabs |
| Encoding | UTF-8, UTF-16 LE/BE, GBK, GB18030, Big5, Shift-JIS, Windows-1252, Latin-1, Keep BOM, Add BOM, Strip BOM |
| EOL | CRLF, LF, CR |
| Format | Format, Minify and Validate JSON Config; Format, Minify and Validate XML Config |
| Help | Enable Logging, Quick Usage, About RNotepad |

Worth knowing:

- The hex view is always read-only; *Enable Editing* only affects a Scintilla
  text view.
- Replace and Replace All require an editable text view and are disabled while
  *Expr* is ticked.
- Ctrl+F opens the find bar; Enter searches from the pattern field, Shift+Enter
  walks backwards, and Escape closes the bar.
- *Help > Quick Usage* opens a read-only reference with the same command map
  inside the application.

## Data files

These files live beside the executable; deleting them simply resets the related
state.

| File | Contents |
| --- | --- |
| `settings.json` | Window placement, editor font size and word-wrap state |
| `recent.json` | Recent files and taskbar-pinned files |
| `history.json` | Separate find and replacement histories |
| `logs\rnotepad.<date>.log` | Daily rolling log, created only after *Help > Enable Logging* |

## Keyboard shortcuts

| Shortcut | Action |
| --- | --- |
| `Ctrl+N` / `Ctrl+O` | New / open document |
| `Ctrl+S` | Save |
| `Ctrl+R` | Reload from disk |
| `Ctrl+Z` / `Ctrl+Y` | Undo / redo |
| `Ctrl+X` / `Ctrl+C` / `Ctrl+V` | Cut / copy / paste in the editor or focused field |
| `Ctrl+A` | Select all in the focused field |
| `Ctrl+F` / `Ctrl+H` | Find / replace |
| `F3` | Find next |
| `Shift+F3` | Find previous |

## Project layout

```
crates/rnotepad-core   Pure-Rust data engine: bytes, encoding, EOL, line index,
                       coordinates, search, formatting, document model
crates/rnotepad-win    Win32 shell and custom views: Scintilla host, large-file
                       view, hex view, results list, status bar, save/notify
crates/rnotepad-app    Application: message loop, main window, menus, commands,
                       session state and background tasks
crates/assets          Application icons embedded in the executable
vendor/scintilla-5.6.9 Vendored Scintilla 5.6.9 sources used to rebuild the DLL
third_party            Prebuilt Scintilla.dll and its license
scripts                Build helpers (bundle assembly, Scintilla build, constants)
docs                   Implementation specification and XML config format plan
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
