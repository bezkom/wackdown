# ƜackDown

A native Direct2D Markdown viewer. Drop a `.md` file onto this window, press **Ctrl+O**, or double-click a markdown file once the file association is installed.

## Keyboard & mouse

| Action | Keys |
|:--|:--|
| Scroll | Mouse wheel, `↑` `↓`, `PgUp` `PgDn`, `Space` / `Shift+Space` |
| Top / bottom | `Home` / `End` |
| Zoom, from 4% to 5000% | `Ctrl` + wheel, `Ctrl +` / `Ctrl -`, reset with `Ctrl 0` |
| Scroll a wide code block sideways | `Shift` + wheel, tilt wheel or two-finger swipe |
| Copy a code block | Hover it and click **Copy** |
| Theme picker (live preview) | `Ctrl+T`, then `↑` `↓`, `Enter` keep, `Esc` revert |
| Next / previous theme | `T` / `Shift+T` |
| Reload file and themes | `F5` (files also reload automatically when saved) |
| Select text | Drag, `Shift`+click, double-click a word, `Ctrl+A` |
| Copy selection | `Ctrl+C` (plain text, and formatted for mail or Word) |
| Follow link | Click; links to other `.md` files open here |
| Find in page | `Ctrl+F`, then type; `Enter` / `Shift+Enter` or `F3` / `Shift+F3` step through matches, `Esc` closes |
| Back / forward | `Backspace` or `Alt+←` / `Alt+→`, mouse back and forward buttons |
| Context menu | Right-click: copy, select all, find, open, print, themes, always on top, help; on a link: open or copy its address |
| Open file | `Ctrl+O` or drag and drop |
| Print, or save as PDF | `Ctrl+P`, then pick *Microsoft Print to PDF* for a PDF |
| Always on top | `Ctrl+Shift+A` |
| Help | `F1` |
| Close | `Ctrl+W` |

## Reading view

Wide window, or zoomed out? The page splits into side-by-side columns that scroll together; text flows down one column and continues at the top of the next. Fast scrolling is motion-blurred (`"motionBlur": false` in `config.json` turns that off), and zoom changes crossfade instead of snapping.

Files that change on disk are re-read within a second, and what changed is briefly highlighted. If you were at the end of the file, you stay at the end as it grows: open the report an agent is writing and watch it arrive. Every file reopens where you left it.

Local images (`![alt](picture.png)` on a line of its own) are drawn, scaled to the column; remote images show their alt text, because ƜackDown has no network code.

## Themes {#themes-section}

Eight themes are built in: Cyberpunk Terminal, Tokyo Dusk, Monokai Pro Rust, Nordic Fjord, Solarized Deep Dark and Paper & Gutenberg, plus Monochrome Light and Monochrome Dark for boring people. The first row of the picker, **Follow Windows**, switches between a light and a dark theme with the Windows color mode. Your choice is remembered in `%APPDATA%\WackDown\config.json`. Dark themes print with Monochrome Light.

> [!TIP]
> Put your own `*.json` themes in `%APPDATA%\WackDown\themes`. Copy one of the built-in themes as a starting point — only `name`, `colors.background` and `colors.text` are required. A custom theme with the same `id` as a built-in one replaces it. Press **F5** to reload after editing.

## Feature tour

Text can be **bold**, *italic*, ***both***, ~~struck~~, `inline code`, a [link](https://github.com), or a #tag. Footnotes work too: click the marker to jump, and the ↩ to come back.[^1] Emoji render in full color: 🦀 ⚡ 🎨 ✅

Chemistry and exponents: H~2~O, CO~2~, 2^10^ = 1024. Math becomes Unicode: $\alpha^2 + \beta^2 \leq \gamma^2$, and display math gets a line of its own:

$$\sum_{i=1}^{n} x_i^2 \geq 0$$

Formulas Unicode can't express show their TeX source instead: $\begin{matrix} a & b \end{matrix}$

Definition list
: A term followed by its description.

Heading attribute
: `## Title {#custom-id}` sets a heading's link target, like [this jump to the themes section](#themes-section).

Wiki link
: `[[Another Note]]` opens `Another Note.md` next to the current file, and `[[Note#Heading]]` jumps to a section.

- Bullet lists
  - nest
    - deeply
1. Ordered lists
2. keep their numbers

- [x] Task lists
- [ ] with checkboxes

> Block quotes can contain *anything*.
>
> > Even other quotes.

> [!NOTE]
> GitHub-style callouts: NOTE, TIP, IMPORTANT, WARNING and CAUTION.

> [!WARNING]
> Like this one.

```rust
// Syntax highlighting for common languages
fn main() {
    let greeting = "Hello, ƜackDown!";
    println!("{greeting} {}", 42);
}
```

| Left | Center | Right |
|:-----|:------:|------:|
| a    | b      | 1.00  |
| c    | d      | 20.50 |

---

## About

ƜackDown {version} is AI-generated (built with Claude Code under human direction) and is free software under the GNU GPL v3 or later. It comes with absolutely no warranty. The Iosevka fonts are embedded under the SIL Open Font License 1.1.

[^1]: Footnote definitions appear where they are written.
