"""Trims Iosevka to the characters a Markdown viewer needs, for embedding in the exe.

    python assets/fonts/subset_fonts.py <folder with the unhinted Iosevka TTFs>

Source: Iosevka v34.9.0 unhinted TTF packages (https://github.com/be5invis/Iosevka/releases),
SIL Open Font License 1.1 with no Reserved Font Name, so subsets keep their names.
A full Iosevka style is ~8 MB with ~48,000 glyphs; this keeps Latin/Greek/Cyrillic, punctuation,
symbols used by the UI and box drawing, plus the default OpenType features (ligatures, kerning).
Requires fontTools (pip install fonttools).
"""
import sys
from pathlib import Path

from fontTools import subset

OUT = Path(__file__).resolve().parent

FAMILIES = ["Iosevka", "IosevkaAile", "IosevkaEtoile"]
STYLES = ["Regular", "Bold", "Italic", "BoldItalic"]

# Anything not kept here still renders: DirectWrite falls back to a system font for missing glyphs.
UNICODES = ",".join([
    "U+0020-007E",  # Basic Latin
    "U+00A0-024F",  # Latin-1, Latin Extended-A/B (includes Ɯ U+019C, the turned M)
    "U+02B0-02FF",  # spacing modifiers (ˆ ˜ ˚)
    "U+0300-036F",  # combining diacritics
    "U+0370-03FF",  # Greek
    "U+0400-045F",  # Cyrillic (basic)
    "U+2000-206F",  # general punctuation (‼ … – — ‘ ’ “ ”)
    "U+2070-209F",  # super/subscripts
    "U+20AC,U+20BD,U+20B9,U+2122,U+2116",  # € ₽ ₹ ™ №
    "U+2190-2199,U+21B5,U+21E7",  # arrows, ↵ ⇧
    "U+2200-2214,U+221A,U+221E,U+2227-222B,U+2248,U+2260-2265,U+2298",  # common math (⊘)
    "U+2318,U+2325,U+232B,U+23CE",  # ⌘ ⌥ ⌫ ⏎
    "U+24D8",  # ⓘ (callout Note)
    "U+2500-257F",  # box drawing (tree / table art in code blocks)
    "U+2580-259F",  # block elements
    "U+25A0-25A1,U+25AA-25AB,U+25B2-25BC,U+25C6-25CB,U+25CF,U+25E6",  # ■ □ ▪ ▲ ▼ ◆ ○ ● ◦
    "U+2605-2606,U+26A0",  # ★ ☆ ⚠
    "U+2713-2718,U+2731",  # ✓ ✔ ✕ ✗ ✘ ✱
    "U+FFFD",
])


def main(src: Path):
    for family in FAMILIES:
        for style in STYLES:
            name = f"{family}-{style}.ttf"
            args = [
                str(src / name),
                f"--unicodes={UNICODES}",
                f"--output-file={OUT / name}",
                "--layout-features+=zero",  # defaults keep calt/liga/kern/ccmp/mark; add slashed zero
                "--name-IDs=*",
                "--name-languages=*",
                "--no-hinting",
                "--drop-tables+=DSIG",
            ]
            subset.main(args)
            print(f"{name}: {(OUT / name).stat().st_size / 1024:.0f} KB")


if __name__ == "__main__":
    main(Path(sys.argv[1]))
