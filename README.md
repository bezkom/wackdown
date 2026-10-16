# ƜackDown 🦀⚡
**Native Windows 64-bit Direct2D Markdown Viewer**

*Ɯ is an upside-down M (U+019C): Markdown, turned on its head. Plain spelling for search, files and commands: WackDown.*

> [!NOTE]
> ƜackDown is AI-generated: its code, documentation, installer and icon were written by an AI coding
> assistant (Claude Code) under human direction and review. Free software under the
> [GPL-3.0-or-later](#license).

[![CI](https://github.com/bezkom/wackdown/actions/workflows/ci.yml/badge.svg)](https://github.com/bezkom/wackdown/actions/workflows/ci.yml)

**[Download the latest release](https://github.com/bezkom/wackdown/releases/latest)**: the installer or a portable exe (Windows 10/11 x64).

**Website:** [bezkom.github.io/wackdown](https://bezkom.github.io/wackdown/)

## Highlights
- **Instant Cold Launch**: Double clicking any `.md` file opens the window almost instantly.
- **Zero Runtime Dependencies**: No Electron, no WebView2, no Chromium, no Node.js.
- **Direct2D Hardware Rendering**: Zero HTML, zero CSS, pure GPU immediate-mode rasterization.
- **Embedded Distinctive Typography**: Built-in [Iosevka](https://github.com/be5invis/Iosevka) in three cuts: monospace, Aile (sans) and Etoile (serif). Each theme pairs its own body, heading and code faces. No system font dependencies.
- **Custom JSON Themes**: 8 built-in themes (six moods, two deliberately boring monochrome ones) that
  can follow the Windows light/dark setting, plus your own in a few lines of JSON.
- **Full Emoji Support**: DirectWrite translated color glyph runs.
- **Pictures**: local images (PNG, JPEG, GIF, BMP, WebP…) are drawn through the Windows imaging codecs
  and scaled to the column. Remote images stay alt text: there is no network code.
- **Made for agent output**: a file that is being written reloads as it grows, what changed is
  highlighted, a view at the end stays at the end, and every file reopens where you left it.
- **Print and PDF**: `Ctrl+P` goes through the Windows print dialog; "Microsoft Print to PDF" makes
  the PDF. The same Direct2D drawing that goes on screen goes on paper.
- **Rich copy**: a selection pastes into mail, Word or Teams with headings, lists, links and code intact.
- **Motion in step with your display**: a DXGI flip-model swap chain paces every frame to the monitor's
  refresh rate (60, 120, 144 Hz…), with spring-based scrolling, fluid zoom and soft hover transitions.
- **Huge files open instantly**: only the first screen is laid out before the window appears; the rest
  follows in the background a few milliseconds per frame.

## Design Philosophy: Do One Thing, Instantly

ƜackDown opens a Markdown file and shows it, beautifully and immediately. That is the whole job, and
everything it leaves out is deliberate.

### One thing, done well
In the early days of Windows, your desktop was a toolbox of small utilities. Notepad edited text.
Calculator calculated. Character Map found the glyph you needed. Paint painted. Each did one job,
started the moment you asked, and got out of your way. Nobody wanted their calculator to have a
plugin marketplace.

Many of today's apps go the other way. A note-taking tool grows a browser, a sync engine, an AI
sidebar and a plugin store, and ends up as a few hundred megabytes that take seconds to show a page
of text. Trying to do everything, they no longer do the one thing you opened them for quickly.

ƜackDown is a deliberate return to the utility model: a single small executable with nothing to
configure, no account and no background services. It doesn't phone home.

### What we don't do, and why
- **No multiplatform support.** ƜackDown talks to Win32, Direct2D and DirectWrite directly. There is
  no portability layer, no bundled runtime and no lowest-common-denominator UI toolkit between your
  file and the GPU. Being portable would mean giving up exactly what makes it fast and native: system
  font fallback for emoji, per-monitor DPI, the Windows clipboard and file associations.
- **No HTML.** Most Markdown viewers convert Markdown to HTML and hand it to a web engine, which means
  a browser engine, a DOM, CSS layout and JavaScript just to show some headings and lists. ƜackDown
  parses Markdown straight into its own layout and draws it with Direct2D. There is no browser hiding
  inside.
- **No file management.** No vaults, sidebars, tabs, sync, or editor. Windows already has an excellent
  file manager: Explorer. Double-click a file there and ƜackDown shows it. It watches only the file
  that's open, and reloads it the moment you save it in your editor of choice.

### Computers got faster. Apps didn't feel it.
A single CPU core today does orders of magnitude more work per second than the Pentium that ran
Windows 95, and there are many more cores and a GPU next to it. Yet opening a document often feels
no faster than it did then, because the extra power went into layers instead of into you.

ƜackDown spends that power on you. Press **Ctrl+T** and flick through the themes: every switch
re-lays out the whole document and redraws it on the GPU, and the picker shows the measured cost of
the last switch. On the built-in tour that's a few milliseconds, well under a single 60 Hz frame.

## Supported Markdown
CommonMark plus GitHub extensions: headings, emphasis, strikethrough, links, inline code,
fenced code blocks with syntax highlighting, block quotes, GitHub callouts (`> [!NOTE]`, `TIP`,
`IMPORTANT`, `WARNING`, `CAUTION`), bullet / ordered / task lists, tables with column alignment,
horizontal rules, `#tags` and YAML front matter (hidden).
Local images are drawn (see **Pictures** below); remote images are shown as their alt text; raw HTML
blocks are shown as source.

Further extensions:

| Extension | Syntax | Notes |
|:--|:--|:--|
| Footnotes | `Text[^1]` … `[^1]: Note` | Click a marker to jump to its note; the ↩ at the end of the note jumps back. |
| Definition lists | `Term` then `: Definition` on the next line | Bold term, indented definitions. |
| Heading attributes | `## Title {#custom-id}` | Sets the heading's link target (`[link](#custom-id)`). |
| Sub/superscript | `H~2~O`, `2^10^` | Works inside words. Uses real Unicode characters (H₂O, 2¹⁰) where they exist, smaller text otherwise. |
| Wiki links | `[[Page]]`, `[[Page\|label]]`, `[[Page#Heading]]` | Opens `Page.md` (or `Page.markdown`) next to the current file. |
| Math | `$x^2$`, `$$\sum_{i=1}^n x_i$$` | See below. |

> [!NOTE]
> **Single tildes mean subscript.** `~text~` is subscript (Pandoc style), so `H~2~O` works. GitHub
> renders single tildes as strikethrough; use `~~double tildes~~` for strikethrough, which works everywhere.

**Math.** DirectWrite has no equation layout, so ƜackDown converts TeX to Unicode: Greek letters,
operators, relations, arrows, `\frac`, `\sqrt`, `\mathbb`, accents and super/subscripts, rendered in
the serif cut (Iosevka Etoile). `$$…$$` gets its own centered line. It never guesses: when a formula
can't be expressed faithfully (matrices, alignments, letters with no Unicode super/subscript form,
unknown commands), its TeX source is shown instead, monospaced.

**Pictures.** A paragraph that is nothing but `![alt](path.png)` with a local path becomes the picture
itself, decoded by the Windows Imaging Component (PNG, JPEG, GIF, BMP, TIFF, WebP and whatever codecs
are installed), centered and scaled down to fit the text column, never enlarged past its pixel size
at 100%. Paths resolve relative to the document. A picture that can't be read shows its alt text;
images inside a sentence and images fetched over a network stay alt text, since ƜackDown has no
network code.

Not supported: Mermaid diagrams render as code blocks.

## Usage
| Action | Keys |
|:--|:--|
| Scroll | Mouse wheel, `↑` `↓`, `PgUp` `PgDn`, `Space` / `Shift+Space`, scrollbar drag |
| Top / bottom | `Home` / `End` |
| Zoom (fluid, centered on the cursor; 4% to 5000%) | `Ctrl` + wheel, `Ctrl +` / `Ctrl -`, reset with `Ctrl 0` |
| Scroll a wide code block sideways | `Shift` + wheel, tilt wheel or two-finger swipe over the block |
| Copy a code block | Hover it and click **Copy** |
| Theme picker (live preview) | `Ctrl+T`, then `↑` `↓` to preview, `Enter` to keep, `Esc` to revert |
| Next / previous theme | `T` / `Shift+T` |
| Reload file and themes | `F5` (files also reload automatically when saved) |
| Select / copy text | Drag, `Shift`+click, double-click, `Ctrl+A`; `Ctrl+C` copies plain text and formatted (HTML) text |
| Follow link | Click — `#anchors` scroll, relative `.md` links open in place, URLs open in the browser |
| Find in page | `Ctrl+F`, then type; `Enter` / `Shift+Enter` or `F3` / `Shift+F3` step through matches, `Esc` closes |
| Back / forward | `Backspace` or `Alt+←` / `Alt+→`, mouse back and forward buttons |
| Context menu | Right-click: copy, select all, find, open, print, themes, always on top, help; on a link: open it or copy its address |
| Open file | `Ctrl+O` or drag and drop |
| Print, or save as PDF | `Ctrl+P` (pick *Microsoft Print to PDF* for a PDF) |
| Always on top | `Ctrl+Shift+A` |
| Help | `F1` |

Launched without a file argument, ƜackDown opens `README.md` from the current folder, or the built-in help.

## Reading view
- **Columns.** When the window is wider than two reading columns (a wide monitor, or zoomed out), the
  view splits into side-by-side columns: text flows down one and continues at the top of the next,
  and all columns scroll together. The column width follows the theme's `measure`, so nothing is ever
  laid out wider than the window; at extreme zoom a line simply holds fewer letters.
- **Motion.** Fast scrolling is motion-blurred, and a zoom or column change crossfades from the
  previous frame instead of snapping. Both are cheap bitmap composites of the already-drawn page.
  Turn the blur off with `"motionBlur": false` in `%APPDATA%\WackDown\config.json`.
- **Live files.** A file that changes on disk is re-read within a second, and the blocks that changed
  are briefly highlighted. If the view was at the end of the file, it stays at the end as the file
  grows, like `tail -f`: open the report an agent is writing and watch it arrive.
- **Where you left off.** Every file reopens at the position you were reading; the last hundred files
  are remembered in `config.json`.
- **Always on top.** `Ctrl+Shift+A` keeps the window above the editor you are working in. Remembered.

## Printing
`Ctrl+P` opens the Windows print dialog; choose *Microsoft Print to PDF* to get a PDF. The document is
laid out afresh for the paper size at 100%, blocks move to the next page rather than being cut (unless
one is taller than a page), the theme's padding is the margin (pushed out to the printer's hardware
margin if that is larger), and each page gets a footer with the file name and page number. Light
themes print as they look on screen; dark themes print with *Monochrome Light*, so the page is not a
slab of toner. No browser engine or PDF library is involved: the pages are Direct2D command lists
handed to the Windows print pipeline.

## Custom Themes
The eight built-in themes live in `themes/` and are compiled into the binary. To add your own, drop a
`*.json` file into `%APPDATA%\WackDown\themes` (copy a built-in theme as a starting point).
Only `name`, `colors.background` and `colors.text` are required — every other color, typography and
layout value falls back to something derived from those. A custom theme with the same `id` as a
built-in one replaces it.

`layout.measure` sets the reading width in characters of the body font (the built-in themes use
96–120; lower it for a narrower, more bookish column); `layout.maxWidth` caps it and is how far code
blocks may widen when their content needs the room. Tables stay within the text column and are
centered in it; `typography.tableScale` (default `0.9`) sets table text size relative to body text.
Table columns are sized like a browser's automatic layout: each column keeps its longest word
intact, short columns are never wrapped, and long prose cells wrap.

Typography has three font roles, each a CSS-style list where the first available family wins:
`fontFamily` (body text), `headingFontFamily` (defaults to the body font) and `codeFontFamily` (defaults
to `monospace`). The embedded families are `Iosevka`, `Iosevka Aile` and `Iosevka Etoile`, also reachable
as `monospace`, `sans-serif` and `serif`. Any other name is looked up in the installed system fonts, and
anything unavailable falls back to embedded Iosevka.

| Built-in theme | Body | Headings |
|:--|:--|:--|
| Cyberpunk Terminal | Iosevka | Iosevka |
| Monokai Pro Rust | Iosevka | Iosevka Aile |
| Solarized Deep Dark | Iosevka | Iosevka Etoile |
| Tokyo Dusk | Iosevka Aile | Iosevka Aile |
| Nordic Fjord | Iosevka Aile | Iosevka Etoile |
| Paper & Gutenberg | Iosevka Etoile | Iosevka Etoile |
| Monochrome Light | Iosevka Aile | Iosevka Aile |
| Monochrome Dark | Iosevka Aile | Iosevka Aile |

The two monochrome themes are grayscale only, for boring people and for printing.

**Follow Windows.** The first row of the theme picker (`Ctrl+T`) is *Follow Windows*: it pairs a light
and a dark theme and switches between them with the Windows app color mode (Settings → Personalization →
Colors), live. Choosing it adopts the current theme as the light or dark half of the pair, whichever it
is; the other half defaults to Paper & Gutenberg or Cyberpunk Terminal. The pair is stored as
`lightTheme` / `darkTheme` in `config.json`, with `"followSystem": true`. Picking any theme explicitly
turns following off.

The selected theme, zoom level, window size and position, always-on-top state and the reading position
of the last hundred files are remembered in `%APPDATA%\WackDown\config.json`. That file is the entire
configuration.

## Building for Windows 64
Requires the Rust MSVC toolchain (`rustup`) and Visual Studio's **MSVC Build Tools for x64/x86** plus a
Windows SDK (both included in the "Desktop development with C++" workload).

```bash
# Build optimized release binary
cargo build --release --target x86_64-pc-windows-msvc

# The resulting binary is at:
# target/x86_64-pc-windows-msvc/release/wackdown.exe

# Run the unit tests
cargo test --target x86_64-pc-windows-msvc
```

Run cargo from PowerShell or cmd. In Git Bash, Git's own `link.exe` can shadow the MSVC linker.

## Testing
Besides unit tests, `src/resilience_tests.rs` checks that no input can crash, hang or slow down the
parser, highlighter or rendering engine (a real Direct2D renderer drawing to a hidden window):
- **Spec corpus:** all 1,324 CommonMark 0.31.2 and GFM 0.29 examples (`tests/fixtures`), each parsed,
  structurally validated, laid out and drawn in every theme, at several zoom levels and window widths.
- **Pathological inputs:** about 35 generators after cmark's `pathological_tests.py` (nested emphasis,
  unclosed brackets, backtick runs and so on), plus cases aimed at ƜackDown's own code. Each must
  finish within a time budget, which catches hangs and super-linear blowups.
- **Mutation fuzzing:** a deterministic fuzzer that mutates spec examples. Failing inputs are saved
  to `target/resilience-failures/`. For a longer run:
  `$env:WACKDOWN_FUZZ_ITERS=100000; cargo test --release --target x86_64-pc-windows-msvc -- mutation_fuzz`

## Installer
```powershell
winget install JRSoftware.InnoSetup   # once: open-source installer builder
.\installer\build.ps1                 # builds the release exe + dist\wackdown-setup-<version>.exe
```
The setup installs per-user (no admin prompt) to `%LOCALAPPDATA%\Programs\WackDown`, adds a Start menu
entry, an optional desktop shortcut and an uninstaller, and registers `.md` / `.markdown`.
Machine-wide install: `wackdown-setup-<version>.exe /ALLUSERS`. Silent install: add `/VERYSILENT`.

## Releases
Pushing a tag that matches the `Cargo.toml` version (`git tag v1.0.0 && git push origin v1.0.0`) runs
`.github/workflows/release.yml`: tests, release build, installer, then a GitHub release with the installer,
a portable exe and `SHA256SUMS.txt`. Every push and pull request runs the tests via `ci.yml`.

## Instant File Association
The installer makes ƜackDown the handler when no other app owns `.md`; otherwise it adds ƜackDown to
*Open with*, and Windows lets you pick it once via *Open with → Choose another app → Always*.
Uninstalling never removes another app's association.

Without the installer: copy `wackdown.exe` to `C:\Program Files\WackDown\` and run `wackdown-register.reg`
(edit the paths in the file if you put it elsewhere).

## Icon
**Ɯ**, the turned M, beside Markdown's down arrow. The letter is the Ɯ glyph of the embedded
Iosevka Aile Bold, the same character the docs use, in the family's wider sans cut so it stays legible at 16 px.
`assets/icon/make_icon.py` (Pillow) reads the outline straight from the embedded font, draws every size
separately so small sizes stay sharp, and writes
`wackdown.ico` (app) and `document.ico` (`.md` files). `build.rs` embeds both into the exe, together with
version info taken from `Cargo.toml`.

## Code signing policy
Free code signing provided by [SignPath.io](https://about.signpath.io/), certificate by
[SignPath Foundation](https://signpath.org/) *(application in progress; releases are unsigned until
it is approved)*.

- **What is signed:** `wackdown.exe` and the installer `wackdown-setup-<version>.exe`, built from this
  repository's source by the [Release workflow](.github/workflows/release.yml) on GitHub Actions.
  The installer is produced with [Inno Setup](https://jrsoftware.org/isinfo.php) (open source).
- **Team:** this is a single-maintainer project.
  - Committers and reviewers: [bezkom](https://github.com/bezkom)
  - Approvers: [bezkom](https://github.com/bezkom)
- **Approval:** every signing request is approved manually by an approver; all team members use
  multi-factor authentication for GitHub and SignPath.

### Privacy policy
This program will not transfer any information to other networked systems unless specifically
requested by the user or the person installing or operating it. ƜackDown has no network code: it
reads the files you open and its own settings in `%APPDATA%\WackDown`, and only opens a web link in
your default browser when you click it.

## License
ƜackDown is free software: you can redistribute it and/or modify it under the terms of the
GNU General Public License as published by the Free Software Foundation, either version 3 of the
License, or (at your option) any later version. It is distributed in the hope that it will be useful,
but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS FOR A
PARTICULAR PURPOSE. See [`LICENSE`](LICENSE) for the full text.

ƜackDown is AI-generated, created with Claude Code (an AI coding assistant) under human direction.

Iosevka (© Renzhi Li) is embedded under its own license, the SIL Open Font License 1.1
(`assets/fonts/Iosevka-LICENSE.md`). The embedded files are subsets of the v34.9.0 release made with
`assets/fonts/subset_fonts.py` (Latin, Greek, Cyrillic, punctuation and the symbols the UI uses);
other characters render through Windows font fallback.
