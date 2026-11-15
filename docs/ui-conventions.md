# UI conventions

This page sets the look for every Tabler window region and dialog. Each rule gives the Tailwind classes to write and a short reason. Where a rule picks one of several variants already in the code, it takes the variant most of the code uses or the one that matches TablePlus. Shadcn primitives (`src/components/ui/**`) keep their defaults; these rules only cover what we pass to them. Do not retune `src/index.css`.

**Direction.** Tabler is a desktop database client for developers who work all day in tables and SQL. It should feel exact, dense and quiet, like an instrument panel or a workbench, where the data is the thing to look at. The signature detail is the keycap for shortcuts (`Kbd`). The chrome stays neutral. `--brand` is the only accent, and it marks four things: the primary action, the selection, the focus ring and dialog title icons. Dense desktop sizes are correct: `h-6`/`h-7`/`h-8` controls and `text-xs`/`text-sm` text. Never inflate them to landing-page sizes.

Method: "unslop" by Damir Sokolovsky (@heydamir), CC BY 4.0.

## 1. Type

The scale is defined in `index.css`. Text sizes set with rem tokens follow Settings ▸ Text size (`applyPrefs.ts` scales the root size). Text set in pixels does not, so prefer the tokens.

| Class                              | Size / line height | Use for                                                                                                       |
| ---------------------------------- | ------------------ | ------------------------------------------------------------------------------------------------------------- |
| `text-xs`                          | 11px / 16px        | Labels, hints, descriptions, captions, bar text, menu labels, section headings, counts, errors                |
| `text-sm`                          | 13px / 18px        | Body text, dialog titles, list item names, menu items (the primitive's size), settings row labels, h-8 fields |
| `text-base`                        | 14px               | The command palette input only                                                                                |
| `text-lg` / `text-xl` / `text-2xl` |                    | The Launcher heading and metric KPI figures only                                                              |

Pixel sizes are allowed only in these cases:

- **`text-[12px]` for window chrome text only.** This covers tab titles (TabItem), result tabs and the result info strip, editor-bar controls, the bottom-bar row summary, the status pill, Launcher secondary lines, inspector tabs and field names, and metric card titles. Do not use it anywhere else.
  - On grid cells and headers, `--grid-font-size` overrides it anyway. `GridCell.tsx:68` and the id header are locked by tests, so leave them.
- **`text-[10px]` for keycaps (`Kbd`), chips and badges, sidebar counts and the inspector "hidden" marker.** Never use it for hints or captions; those take `text-xs`.
- **`text-[9px]` only inside a sidebar badge or a count bubble.** The sidebar font-size hook (`index.css:345-353`) shrinks `[10px]` and `[9px]` there.
- **Code surfaces follow their own font.**
  - Markdown, plan and SQL code blocks use `text-[11.5px]`.
  - The multiline cell editor uses `text-[12.5px]` to match the cell.
  - CodeMirror and the grid use their settings.
  - Diagram SVG text is exempt.
- **Two leftovers.** The WorkspaceRail tag initial (`text-[7px]`) is a glyph inside a dot, not text. `font-semibold` is the heaviest weight it may take.

Spell `text-[11px]` as `text-xs` and `text-[13px]` as `text-sm`. The pixel size does not change, but the line height does (16px and 18px), so check fixed-height rows after swapping. The sidebar hook matches both spellings, so the swap is safe there.

Remove `text-[8px]` and `text-[12.5px]` from menu items; menu items use the primitive's `text-sm`.

**Weights:**

- `font-normal` (400) for body text, side labels and counts.
- `font-medium` (500) for titles of every kind (dialog, sheet, panel, popover), stacked labels, section headings, buttons (the primitive's weight), the active tab and tree item, keycaps and chips.
  - Reason: hierarchy comes from size and colour.
- `font-semibold` (600) only to emphasise data: the Launcher connection name, metric card titles and KPIs, palette match highlights, the selected column, and markdown headings.
- Never use `font-semibold` on dialog, panel or section titles.
- Never use `font-bold`.

**Other type rules:**

- **Case.** No `uppercase`, except a hex value input. Write acronyms in capitals in the source ("MCP"). Lowercase data stays as data ("explain").
- **Tracking.** No `tracking-*`. The `EMPTY` grid marker's `tracking-wide` is a data convention and stays. The menu shortcut keeps the primitive's `tracking-widest`; do not reset it.
- **Italic.** Italic is reserved for these cases:
  - NULL, DEFAULT and placeholder values (`text-null italic`), plus absent values shown in place of data, such as "none", "auto" and "(anonymous)".
  - SQL comments.
  - The preview tab title.
  - Never use italic for empty-state messages or for emphasis.
- **Numbers.** Counts, sizes, durations and row numbers take `tabular-nums`.

## 2. Colour

Use tokens only. Tailwind palette colours (`amber-*`, `emerald-*`, `black`, `white`) and hex values are banned outside these places: theme definitions, `charts/svg.ts`, `badgeContrast.ts`, `lib/theme.ts`, the colour-swatch rainbow, and the Windows close-button red `#c42b1c` (`WindowControls`, `SettingsWindow`).

| Role                                                          | Token                                                                                                                    |
| ------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------ |
| Primary text                                                  | `text-foreground`                                                                                                        |
| Secondary text, meta, labels, icons                           | `text-muted-foreground`                                                                                                  |
| NULL, DEFAULT and absent values                               | `text-null`                                                                                                              |
| Link or accent text, title icons                              | `text-brand` (not `text-primary`)                                                                                        |
| Primary button, the selected tile, the Data/Structure segment | `bg-primary text-primary-foreground`                                                                                     |
| Captions on a selected brand tile                             | `text-primary-foreground/80`                                                                                             |
| Error text, icon, invalid field                               | `text-error` / `border-error`; write `text-error`, never `text-destructive` (same colour, one name)                      |
| Error or warning tint                                         | `bg-error/10`, `bg-warning/10`                                                                                           |
| Warning, success                                              | `text-warning`, `text-success` (replaces `amber-*` and `emerald-*`)                                                      |
| Text on an error fill                                         | `text-primary-foreground` (replaces `text-white`)                                                                        |
| Page, content, bars                                           | `bg-background`, `bg-surface`, `bg-surface-2` (toolbar, tab strip, bottom bar, editor bar, find and filter bars, tracks) |
| Sidebar, overlays                                             | `bg-sidebar`, `bg-popover`                                                                                               |
| Hairlines                                                     | `border-border`; grid lines `border-gridline`; an emphasised edge `border-border-strong`                                 |
| Staged changes                                                | `bg-inserted`, `bg-changed`, `bg-deleted`                                                                                |

- **No alpha on text colours.** Do not write `text-muted-foreground/50…/80` or `text-foreground/60…/90`. Use one of the three text roles.
  - The exceptions are `text-primary-foreground/80` on brand tiles, `text-error/80` for the second line of an error block, and the EmptyState icon (inside the component).
  - Placeholders use the primitive's plain muted colour.
- **Disabled.** Use `opacity-50`, the primitives' value. Do not use 40, 45, 60 or 70.

**State tokens.** Use one token for each role.

| State                                                                                                        | Classes                                                                     | Why                                                                                                             |
| ------------------------------------------------------------------------------------------------------------ | --------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------- |
| Row hover (lists, trees, tables)                                                                             | `hover:bg-row-hover`                                                        | Most rows already use it                                                                                        |
| Chrome control hover and open state (icon buttons, bar buttons, tabs, window captions, sidebar pane buttons) | `hover:bg-accent`, `data-[state=open]:bg-accent`, `aria-expanded:bg-accent` | `accent` is the only neutral that shows on both `surface` and `surface-2`, and menu items already focus with it |
| Ghost `<Button>` on a `bg-surface-2` bar                                                                     | Add `hover:bg-accent dark:hover:bg-accent aria-expanded:bg-accent`          | The primitive's `hover:bg-muted` is invisible there, because `--muted` is `--surface-2`                         |
| Selected row, option or active palette or combobox item                                                      | `bg-brand-soft` (`bg-sidebar-accent` is the same colour in the sidebar)     | Selection is a brand role                                                                                       |
| Selection while the pane is not focused                                                                      | `bg-accent`, which turns `bg-brand-soft` on focus                           | macOS inactive selection; ConnectionRow and SchemaTree already do this                                          |
| Current item in the sidebar tree or Launcher                                                                 | `bg-brand-soft` plus the 2px brand `before:` bar                            | Only these two places                                                                                           |
| Pressed or on toggle (Filters, find options, `data-[state=on]`)                                              | `bg-brand-soft text-brand`                                                  | A pressed toggle is a selection                                                                                 |
| Drop target                                                                                                  | `bg-brand-soft ring-1 ring-inset ring-brand/50`                             | One recipe instead of three                                                                                     |
| Brand hover                                                                                                  | Only for the FK jump arrow in a cell and the column resize handle           | Everywhere else hover is neutral                                                                                |

**Focus.**

- Standard focus ring: `outline-none focus-visible:ring-2 focus-visible:ring-ring/50`. Add `ring-inset` inside lists and clipped groups.
- Thin chrome ring: the sidebar, tab strip, data grid and list rows use `focus-visible:ring-1 focus-visible:ring-inset focus-visible:ring-ring`. This is TablePlus's thin ring.
- Never show focus as a fill. This affects QueriesPane, HistoryPane and WindowControls, which use `focus-visible:bg-*` today.
- Every `role="listbox"` must be focusable and show a ring, either on the list (with `aria-activedescendant`) or on the row. UserManagement's users list and BoardList lack one.
- An `outline-none` with no replacement ring is a bug.

## 3. Dialogs

### Anatomy (`Dialog`)

| Part                          | Write                                                                                                                                                                                                           | Why                                                                                                                                |
| ----------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------- |
| Title                         | `<DialogTitle className="text-sm">` (13px, medium)                                                                                                                                                              | Majority (46 of 83); dense                                                                                                         |
| Title with icon               | `<DialogTitle className="flex items-center gap-2 text-sm">` with `<Icon aria-hidden className="size-4 shrink-0 text-brand" />`                                                                                  |                                                                                                                                    |
| Description                   | `<DialogDescription className="text-xs">`; do not restate `text-muted-foreground`                                                                                                                               | Majority (50 of 83)                                                                                                                |
| Header, padded dialog         | `<DialogHeader>` with no class (gap 8px)                                                                                                                                                                        | Default                                                                                                                            |
| Header bar, full-bleed dialog | `<DialogHeader className="gap-1 border-b px-4 py-3 pr-12">`                                                                                                                                                     | Majority bar; `pr-12` keeps the title clear of the close X                                                                         |
| Body, padded dialog           | The default `p-4 gap-4`; do not override the gap                                                                                                                                                                | 63 of 76                                                                                                                           |
| Body, full-bleed dialog       | `DialogContent className="gap-0 p-0 …"`; inner gutters `px-4`, a form body `p-4`, lists run edge to edge                                                                                                        | One 16px gutter (`px-5` goes)                                                                                                      |
| Footer, padded dialog         | `<DialogFooter>` / `<AlertDialogFooter>` with no padding or background override. Allowed extras: `items-center`, `sm:justify-between`                                                                           | 54 of 67; the `py-3` overrides go                                                                                                  |
| Footer, full-bleed dialog     | `<DialogFooter className="m-0 items-center">`                                                                                                                                                                   | Same bar: `border-t bg-muted/50 p-4 rounded-b-xl`. Replaces the `bg-surface-2/60 min-h-12` tool footers and the `h-11`/`h-10` bars |
| Footer status slot            | First child `<div className="min-w-0 flex-1">`, holding a status line, progress, or `<FormError clamp>`                                                                                                         | Export, import and backup                                                                                                          |
| Close X                       | Keep the primitive's X. Pass `showCloseButton={false}` only on prompts that must be answered (password), palettes, and while a run is in progress. Do not pass `showCloseButton` when it is already the default | Current majority                                                                                                                   |

**Title icons.**

- Tool windows (Export, Import, Backup, Restore, History) and credential prompts use `text-brand`.
- Risk prompts (host key, deep-link filter, plugin permission) use `text-warning`.
- A changed host key uses `text-error` on the icon only; the title text stays foreground.
- A finished-run popup uses `text-success`.
- Every other dialog, including destructive confirmations, has no icon.

**Footer buttons.**

- **Size.** Every footer button is `size="sm"` (h-7, 138 of 153 already are). `AlertDialogCancel` and `AlertDialogAction` need `size="sm"` passed explicitly.
- **Cancel.**
  - `variant="outline"`. This is the majority and matches the macOS bordered push button; the ghost Cancels go.
  - `AlertDialogCancel` is already outline.
  - Labels: "Cancel"; "Close" when nothing would be lost; the sole button of a finished window is variant `default`.
- **Order**, from left to right:
  1. Tertiary actions in a left group (`mr-auto`, or the status slot).
  2. Cancel.
  3. The primary.
  - This is the macOS and TablePlus order.
  - With three buttons, the alternative choice ("Discard and Continue") goes between Cancel and the primary as `outline`.
  - LargeFileDialog's three full-sentence options may stay stacked vertically, with the primary first and Cancel last, as NSAlert does.
- **Primary.** One per footer.
  - `variant="default"`.
  - `variant="destructive"` when it drops, deletes, truncates, kills, discards or clears.
  - Running-state cancels ("Cancel Export") are `outline`.
- **Keycap.** When the dialog handles a shortcut for the primary (⌘↩, ⌘S), end the button with `<KbdHint combo="Mod+Enter" />`.
  - One recipe for default, destructive and outline buttons. Do not add `ml-1`; the button's gap spaces it.
  - Add it to Save Theme (ThemeEditorDialog) and Save (BootstrapCommandsDialog).
  - Primaries without a shortcut show no keycap.

### Widths (`DialogContent`)

These are the parity tiers (`tableplus-parity.md`).

| Kind                              | Class                                                                                                                                                                      |
| --------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Confirmation, single-field prompt | `sm:max-w-md` (448px)                                                                                                                                                      |
| Form                              | `sm:max-w-[34rem]` (544px); `lg` and `xl` dialogs snap to this                                                                                                             |
| List or tool panel                | `sm:max-w-[min(48rem,calc(100%-2rem))]`, fixed height `h-[min(34rem,calc(100dvh-4rem))]`                                                                                   |
| Viewer or editor                  | `sm:max-w-[min(56rem,calc(100%-2rem))]`, fixed height `h-[min(40rem,calc(100dvh-4rem))]`                                                                                   |
| Off-tier, documented exceptions   | Process list 1120px, User management and CSV import 980px, Backup 880px, Item overview 72rem, connection form 600px, SQL viewer and diagram 92vw, palettes 620px and 480px |

### Alert dialogs

- `AlertDialogContent className="data-[size=default]:sm:max-w-md"`, or `data-[size=default]:sm:max-w-[34rem]` when it holds a field or a code block.
- Never use `size="sm"`. It centres the header and makes the buttons equal-width grid cells, unlike the other 22 alerts.
- Never use a bare `sm:max-w-[…]`; it loses to the primitive's `data-[size=default]` width (DonePopup).
- Titles are sentence-case questions or statements: "Drop table “orders”?", "SSH host key has changed". Never end a title with "…".
- Title `text-sm`, description `text-xs`, footer as above. There is no X.

### Sheets

- `SheetHeader className="gap-1 border-b px-4 py-3 pr-12"`, with `SheetTitle className="text-sm"` and `SheetDescription className="text-xs"`.
- `SheetFooter className="flex-row justify-end border-t bg-muted/50"`. This is the primitive's `p-4`, so a sheet footer matches a dialog footer.
- Inner gutter `px-4`.

### Popovers

- `PopoverContent className="w-72 gap-2 p-3 text-xs"`. Widths come from `w-64`, `w-72`, `w-80` and `w-96`; the filter editor may use `w-[32rem]`.
- List popovers (combobox, options) use `p-1` with menu-like rows.
- Heading: `<SectionLabel>`. Captions: `text-xs text-muted-foreground`.
- Footer actions sit right-aligned in a row of `size="xs"` buttons.

### Tooltips

- Chrome controls that have a shortcut use the Radix `Tooltip` with the label followed by `<KbdHint combo=… />`. This replaces `className="border-border/40 bg-transparent text-inherit"` on `Kbd`.
- Elsewhere a native `title` is fine. Write it as "Label (⌘K)" with `formatCombo`, never a hard-coded "↩".
- Every icon-only button has an `aria-label`.

## 4. Forms

**Fields.**

| Context                                                                                                      | Control                                                                                                                                                                                                       |
| ------------------------------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Dialog and panel forms (default)                                                                             | Input `h-7 rounded-md text-xs md:text-xs`; Textarea `rounded-md text-xs md:text-xs`; SelectTrigger `size="sm" className="text-xs"` (do not restate `h-7`); SelectItem `text-xs`; `NativeSelect` (admin/parts) |
| Connection form, driver fields, single-field prompts (password, URL, group name, passcode, driver questions) | The primitive's `h-8`; always pass `text-sm`. The primitive's `text-base` applies below 768px, so the Welcome window otherwise gets 14px                                                                      |
| Bars (find, filter, structure header)                                                                        | `h-6 rounded-md text-xs md:text-xs`                                                                                                                                                                           |
| SQL, identifiers, values                                                                                     | Add `font-mono`                                                                                                                                                                                               |

- **Always pair `text-xs` with `md:text-xs`.** The primitive's `md:text-sm` survives `cn`, so `text-xs` alone renders 13px in any window 768px or wider. That is a latent bug at 26 Inputs and 14 Textareas.
- **Radius follows height.** h-7 and h-6 controls are `rounded-md`, like `Button size="sm"` and `SelectTrigger size="sm"`. h-8 controls keep the primitive's `rounded-lg`.
- **Hand-rolled inputs** use the standard focus ring: `focus-visible:ring-2 focus-visible:ring-ring/50`.

**Labels.**

- **Stacked** (the default for dialogs): `<FormField label hint error htmlFor>`. The label is muted `text-xs font-medium` with a 4px gap to the control.
  - It replaces the admin, structure, backup and metrics `Field` copies and the ad hoc stacked labels (Agent, SSH, ExportConnections, GroupDialog, Bootstrap).
- **Side column** (export options, saved query, CSV import): `<FormField orientation="inline">`. The column is 88px, the label right-aligned, muted `text-xs`, normal weight, with `pt-1.5` to centre it on an h-7 control.
- **The connection form keeps its own grid.** It uses `grid-cols-[104px_minmax(0,1fr)] gap-x-3 gap-y-2.5`, with labels `h-8 justify-end text-right text-xs font-normal text-muted-foreground`. This is TablePlus's form, at h-8 to match its fields.
- **Settings** keep `SettingRow`. Write its label as `text-sm leading-5` and its description as `text-xs leading-4` (rem tokens). This is the macOS settings look; hand-made copies (McpSettings, ShortcutsSection, sections.tsx) use the same classes.
- **Every multi-field form has visible labels.** Placeholder-only labels are allowed only on single-field prompts. NewKeyDialog's four fields need labels.

**Spacing.**

- Rows in a form: `gap-3`.
- Label to control: 4px (inside FormField).
- Groups in a form: `gap-5`, each opened by a `SectionLabel`.
- Two-column field grids: `grid grid-cols-2 gap-3`.
- Single-field prompts keep the dialog's own `gap-4`.

**Hint.**

- `text-xs leading-4 text-muted-foreground` under the control (FormField `hint`). No 10px hints.
- Mark optional fields, never required ones, with the placeholder "Optional", or "Optional, e.g. …" when it also gives an example. Do not add a "(optional)" suffix to the label.

**Errors.**

- **Field-level.** Use FormField `error`: `text-error text-xs leading-4` with `role="alert"`, which replaces the hint. Also set `aria-invalid` and `aria-describedby="${id}-error"` on the control.
- **Form-level.** `<FormError>` (a 14px CircleAlert, `gap-1.5`, `text-xs text-error`, selectable) goes at the end of the body, or in the full-bleed footer's status slot with `clamp`.
- **Server errors with detail lines** keep admin `ErrorLine` (`text-error text-xs`, detail lines `opacity-80`).
- **Validation is shown in `text-error`, not muted.** UserManagement shows it muted today.
- **Never write `text-destructive`.**

**Checkboxes and switches.**

- **Checkbox row:** `flex h-7 items-center gap-2 text-xs` with foreground text. Use `h-8` only inside the connection grid, which also makes ConnectionDialog's muted row foreground. Use the Checkbox's default `size-4` (no `size-3.5`). ExportConnections drops from 13px to `text-xs`.
- **Hint under a checkbox:** `pl-6 text-xs leading-4 text-muted-foreground`, aligned with the label text.
- **Inline toggle beside a field** ("Use socket", "Over SSH", "Clear"): `flex items-center gap-1.5 whitespace-nowrap text-xs text-muted-foreground`.
- **Switch:** only in setting rows, at the default size; `size="sm"` only in panel inspectors. New form booleans use Checkbox. Existing Switches keep their role (no aria change).
- **Choosing one of 2 to 4 short options:** the shared `Segmented` (settings/controls). A native radio (`accent-primary size-3.5`, `gap-1.5 text-xs`) is fine for sentence-long options.

## 5. Lists and regions

**Bars.**

| Region                                                                                               | Classes                                                                                                                                                                                                                                                                                                                 |
| ---------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Window toolbar                                                                                       | `h-10 bg-surface-2 border-b`. Controls are `size-7` or `h-7` with `size-3.5` icons; only the status pill may be `h-8`                                                                                                                                                                                                   |
| Tab strip                                                                                            | `h-8 bg-surface-2`. Tab title `text-[12px]`; controls `size-6` with `size-3.5` icons; the tab kind icon is `size-4`                                                                                                                                                                                                     |
| Editor bar                                                                                           | `min-h-10 bg-surface-2`, `h-7` controls                                                                                                                                                                                                                                                                                 |
| Grid bottom bar (`BottomBarFrame`)                                                                   | `h-8 bg-surface-2 border-t px-2 text-xs`. Controls are `Button size="xs"` / `"icon-xs"` (h-6); pagination is an `h-7` group                                                                                                                                                                                             |
| Panel header (inspector, console, results, cell view, SQL viewer, metrics, boards, roles, assistant) | `flex h-8 shrink-0 items-center gap-2 border-b px-3 text-xs`; use `px-2` when it starts with a control or tablist. The title is `<SectionLabel as="span">` when it names the panel ("Console", "Boards") and `text-xs font-medium` foreground when it names an object. The current `h-9` and `h-7` headers become `h-8` |
| Strip inside a pane (plan title, chart notes)                                                        | `flex h-7 items-center gap-2 border-b px-3 text-xs`                                                                                                                                                                                                                                                                     |
| Info strip or notice (result info, query notice)                                                     | `min-h-6 px-3 py-1 text-xs bg-surface-2`. The connection warning banner stays `h-7 bg-warning/10`                                                                                                                                                                                                                       |
| Sidebar footer                                                                                       | `h-8`, `size-6` controls                                                                                                                                                                                                                                                                                                |

**Rows.**

| List                                                              | Height                                                         | Icons                                                                                                                      |
| ----------------------------------------------------------------- | -------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------- |
| Sidebar trees and panes (Items, Queries, History, Boards)         | `h-6` (24px, SchemaTree `ROW_H`)                               | Object glyphs `size-4`; disclosure `size-3.5`; PK/FK markers `size-3`                                                      |
| Lists in dialogs and panels, palettes, settings nav, admin tables | `h-7`                                                          | `size-3.5`. WorkspaceSwitcher, backup ConnectionList and the Launcher group header go from h-8 to h-7                      |
| Two-line rows                                                     | Launcher connections `h-12`; history and plugin entries `py-2` |                                                                                                                            |
| Menu items                                                        | The primitive (`py-1 text-sm`, `size-4` icons)                 | Remove `text-xs`, `py-0.5` and `text-[12.5px]` overrides from items, and `text-xs` from Content, which has no effect there |

**Section headers in lists.**

- `SectionLabel` classes: muted `text-xs font-medium`, never semibold. Sticky ones add `sticky top-0 z-10` and the list's own background: `bg-sidebar` in the sidebar, `bg-surface-2/95` elsewhere.
- **Menu labels** use the primitive `DropdownMenuLabel` / `ContextMenuLabel` (`text-xs font-medium` muted) with no size or weight override.

**Counts.** `ml-auto text-xs font-normal tabular-nums text-muted-foreground`, or `text-[10px]` in the sidebar. No alpha.

**Chips and badges.**

- Neutral: `inline-flex h-4 items-center rounded-sm border border-border px-1 text-[10px] font-medium leading-none text-muted-foreground`.
- State: `bg-brand-soft text-brand` with no border.
- Warning: `border-warning/50 text-warning`.
- The text is the data or a sentence-case word, never CSS-uppercased.
- This replaces 13 variants.

**Icons.** Lucide at the global stroke of 1.5. Never set `strokeWidth`, except on the Windows caption glyphs (1.25) and in charts. Icons take the colour of their text role.

| Context                                                                                                                              | Size                                                                                                        |
| ------------------------------------------------------------------------------------------------------------------------------------ | ----------------------------------------------------------------------------------------------------------- |
| Menu items, sidebar object glyphs, tab kind, dialog title, status pill, EmptyState (`size-6`)                                        | `size-4`                                                                                                    |
| Toolbar, tab-strip controls, panel-header and editor-bar controls, sidebar pane buttons, disclosure chevrons, list rows, form errors | `size-3.5`                                                                                                  |
| Grid bottom bar, inspector field actions, sort arrows, key markers, inline meta                                                      | `size-3`                                                                                                    |
| Inside `<Button>`                                                                                                                    | Leave it unset; the size variant picks it (xs and icon-xs 12px, sm and icon-sm 14px, default and icon 16px) |

Fix the outliers: size-4 in size-7 buttons (ResultsPane, EditorBar Format and Explain), size-4 in size-6 buttons (TabStrip), and size-3 chevrons in QueriesPane.

**Buttons in chrome.**

- Use the size variants as they are. Do not restate heights (`xs h-6`, `sm h-7`, `icon-xs size-6`, `icon-sm size-7`).
- Resize only to fit a bar, and only to a scale step.

**Empty states.**

- **A pane or view** uses `<EmptyState title description? icon?>`. The title is sentence case with no period. The description is a full sentence with a period.
  - This applies to driver inspectors too (Dynamo, Elastic, Mongo, Redis currently use a plain `<p>`).
- **Inside a list:**
  - A list row: `px-3 py-2 text-xs text-muted-foreground`.
  - A table body: `<td colSpan… className="py-6 text-center text-xs text-muted-foreground">`.
  - No italic, no period.
- **Wording:**
  - "No {things}"; "No {things} yet" when the user can create one.
  - "No matches" under a visible search field; `No matches for “x”` when the query is not visible.
  - "No {things} match the filter" for filter bars.
  - "Could not load the {noun}" for failures.

**Loading.**

- `QuietLoading` for panes, `QuietStatus` for parts of dialogs and lists. No skeletons and no "Loading…" text: replace the ad hoc `sr-only` spans in UserManagement and DatabaseList with `QuietStatus`.
- `animate-spin` is allowed only for a wait the user started that has no known progress (the update check). Determinate progress is a bar.

**Toasts.** There is one `NotificationToaster`, with no type icons.

- The title says what happened, in sentence case with no period: "Copied 3 rows". Failures start with "Could not …".
- The description is full sentences with periods.
- Action labels are Title Case.

## 6. Copy

**Title Case** (Apple HIG) applies to buttons, menu items, window and dialog titles that are not questions, tab triggers, and command names wherever they appear as commands: menus, the palette, keymap action labels and the Shortcuts list.

- Capitalise every word except a, an, the, and, but, or, nor, and prepositions of four letters or fewer (as, at, by, for, from, in, into, of, on, to, with), unless the word is first or last.
- Examples: "Show in Finder", "Copy Rows As", "Import from URL…", "Export This Connection…".
- Outliers to fix:
  - "New table", "New connection", "Add foreign key", "Query history", "Preview changes", "New trigger", "New key in db0".
  - "Try again", "Clear history", "Add row", "Create table", "Cancel export", "Keep editing".
  - The sentence-case keymap labels, so the same command reads the same in the menu and the palette.

**Sentence case** applies to:

- Alert titles phrased as questions or statements.
- Labels, section headings, descriptions, hints, placeholders, empty states, tooltips, aria-labels, toasts, menu section labels, settings rows and chips.

**TablePlus wording that stays verbatim:**

- Window names: "Backup database", "Restore database", "Bootstrap commands" (and the "Bootstrap commands…" button).
- Settings labels: "Auto Save SQL Query Tab", "Enable Passcode", "Unlock Safe Mode with Touch ID", "Unlock Passcode with Touch ID", "Change Passcode…".
- User management: "Granted Privileges" and the privilege tab names.
- "Result View Mode".
- "Ok" in the FK popover ("Delete and Ok").
- The AWS term "Secret Access Key".

**Ellipsis.**

- Use "…" (never "...") on buttons and menu items that open more UI before acting.
- Never end a title or placeholder with it. The palette mode prompts ("Search for database…", "Switch connection…") are exempt.

**Periods.**

- None on titles, buttons, labels, captions, hints that are fragments, toast titles, chips or short empty states.
- Descriptions, EmptyState descriptions and toast descriptions that are sentences end with a period. Add the missing periods in ResultsPane and InspectorPanel. Drop them from short status lines ("Stopped", "Not connected").

**Words.**

- No "!".
- Write "and", not "&" (TablePlus verbatim strings excepted): "Discard and Run", "Trust and Connect".
- No contractions: "cannot", "does not".
- Failures read "Could not …", never "Failed to" or "Unable to".
- American spelling in UI copy: "color", "canceled". Identifiers and stored values stay as they are.
- "OK", not "Ok", outside the FK popover.

**Placeholders.**

- Sentence case.
- Search fields read "Search" or "Search {noun}", with no ellipsis.
- Example values are written as data, for example `/tmp/mysql.sock`.
- No shortcut text inside the placeholder. Show it as a `Kbd` beside the field.

**Shortcuts in UI.**

- Show shortcuts as `<Kbd>` on page surfaces and `<KbdHint>` on coloured ones.
- Use one keycap size: drop the `h-4 min-w-4 text-[10px]` overrides. Inline in prose, add only `mx-0.5 align-middle`.
- Never use a raw `<kbd>` or plain glyph text such as "↩ save · esc cancel"; use `formatCombo`.

## 7. Motion

- **Hover and state changes:** `transition-colors` at the default 150ms. Never write `transition-all` (primitives aside). No `hover:scale`. Press feedback is the primitive's `active:translate-y-px`.
- **Overlays** (dialog, popover, menu, tooltip, sheet) use only the primitives' fade plus `zoom-in-95` (100ms) or the sheet slide (200ms). Add no durations or delays.
- **Disclosure chevrons:** `transition-transform duration-150 motion-reduce:transition-none`.
- **Nothing loops,** except `animate-spin` for an indeterminate wait the user started and the update-download progress fill.
- **Never animate size or position.** Width, height, top and left (pane resizes, grid columns) move without a transition.

## 8. Shared components (`src/components/`)

| Component                                           | Role                                                                                                                              |
| --------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------- |
| `FormField`                                         | Label (stacked, or the `inline` 88px column), control, then the hint, which the error replaces (`${htmlFor}-hint` / `-error` ids) |
| `FormError`                                         | Form-level or footer error: CircleAlert `size-3.5`, `text-xs text-error`, `role="alert"`, optional `clamp` and `title`            |
| `SectionLabel`                                      | Group, panel and popover heading: muted `text-xs font-medium`; `as` picks the element (default `h3`)                              |
| `KbdHint`                                           | Keycap inside a primary, destructive or outline button or a tooltip: `border-current/30 bg-transparent text-inherit`              |
| `Kbd`, `EmptyState`, `QuietLoading` / `QuietStatus` | These already exist; use them as described above                                                                                  |

## 9. Constraints for editors

- Never edit `src/components/ui/**` or `src/lib/ipc/bindings/**`.
- Do not change behaviour, shortcuts, stores, IPC, `data-testid` or aria roles. Adding a missing `aria-label` or focus ring is fine.
- Tests that assert classes or labels move with the change. `DataGrid.test.tsx` locks `text-[12px]` on GridCell and the id header.
