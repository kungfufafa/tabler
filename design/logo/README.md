# Tabler logo

**Gajah Sel**, an elephant made of table cells. Each cell of an 8 × 7 grid is either on or off. The
light-blue ear is a block of "selected" cells, and the eye is a cell with a hole in it. The mark says
"tables" and "Tabler's elephant" in one shape. Tabler's elephant is its own: it is not the
PostgreSQL elephant and not the TablePlus icon.

Everything here comes from [`build.py`](build.py). The grid at the top of that file is the single
source of truth, so edit it there and rebuild. Do not edit the SVG or PNG files by hand.

```bash
python3 design/logo/build.py
```

The script needs `fonttools` and `pillow`, the Inter Tight variable font in `~/Library/Fonts`,
Google Chrome (for rendering) and macOS `iconutil`. It rewrites everything in this folder plus
`design/app-icon.svg`, `src-tauri/app-icon.png`, `src-tauri/icons/*` and `public/favicon.png`.

## Files

| File                         | Use                                                        |
| ---------------------------- | ---------------------------------------------------------- |
| `tabler-logo-horizontal.svg` | Default logo (README, website header, about screen)        |
| `tabler-logo-stacked.svg`    | Square-ish spaces (splash, social cards)                   |
| `tabler-symbol.svg`          | The mark alone, 48 px and up                               |
| `tabler-symbol-small.svg`    | The mark at 16 / 32 / 48 px (one unit per cell, no gaps)   |
| `*-dark.svg`                 | Colour version for dark backgrounds                        |
| `*-black.svg`, `*-white.svg` | One-colour versions (print, engraving, single-ink, photos) |
| `../app-icon.svg`            | macOS app icon master (tile on the Big Sur 1024 grid)      |

The app icons in `src-tauri/icons/` are not scaled down from 1024. Each size is drawn on its own
whole-pixel grid. 16 and 32 px are gapless pixel elephants, and from 64 px up the cells get gaps
and the eye gets a hole. The `.icns` and `.ico` files are assembled from those drawings.

## Colour

| Name           | HEX                   | RGB           | Where                                 |
| -------------- | --------------------- | ------------- | ------------------------------------- |
| Tabler Blue    | `#2563EB`             | 37, 99, 235   | Body cells                            |
| Selection Blue | `#8FB3F9`             | 143, 179, 249 | Ear cells                             |
| Ink            | `#0F1A3A`             | 15, 26, 58    | Wordmark                              |
| Blue on dark   | `#3B82F6`             | 59, 130, 246  | Body cells on dark backgrounds        |
| Select on dark | `#A5C4FF`             | 165, 196, 255 | Ear cells on dark backgrounds         |
| Tile           | `#FFFFFF` → `#E8EEFB` |               | App-icon tile gradient (top → bottom) |

The two blues separate by value as well as hue, so the ear still reads in greyscale. No Pantone or
CMYK matching has been done yet, so proof the colours before anything goes to print.

## Wordmark and lockups

The wordmark is "Tabler" set in **Inter Tight Bold** (SIL Open Font License 1.1, which allows logo
use). It uses the font's own kerning with slight display tracking, and it is outlined to paths.

In the horizontal lockup the elephant stands on the text baseline. The cap height equals the height
of the elephant's back (grid rows 3–7), so the head rises above the word. The gap between the mark
and the word is 1½ grid steps (cell + gap).

## Clear space and minimum size

- Clear space: at least one cell's width on every side. The lockup files already include it. The
  square symbol file is tight left and right, so add the space when you place it.
- `tabler-symbol.svg` needs at least 48 px. Below that, use `tabler-symbol-small.svg` at 16 or 32 px
  (whole multiples of 16 px keep every cell on the pixel grid).
- The horizontal lockup needs at least 140 px of width. In print, the symbol needs at least 12 mm.

## Don't

- Rearrange, add or remove cells, or close the gaps in the large mark. Only the small cut drops the
  gaps.
- Recolour single cells or add more colours. A highlighted cell belongs in UI, not in the logo.
- Place the colour mark on Tabler Blue. Use the white version there.
- Stretch the logo, rotate it, outline it, or add shadows or glows (the app-icon tile shadow is the
  only exception).
- Set "Tabler" in another font next to the mark.

## Open items

- No trademark search has been done. Run one (trademark databases plus a reverse image search) before
  a public launch.
- There are no macOS 26 dark or tinted icon variants (Icon Composer `.icon`). The current icon is a
  standard light tile.
