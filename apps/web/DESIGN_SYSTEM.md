# WarpTalk design system

Source of truth for the product web UI (`apps/web`, also loaded by the macOS app's WebView).
Derived from the Linear-style spec in the legacy `warptalk-web/DESIGN.md` and its `globals.css`
tokens, adapted from "marketing canvas" to a dense product UI. The living gallery is the
`/design` route (public, used for visual regression).

Implementation map

| Concern | File |
|---|---|
| Tokens (CSS variables + Tailwind 4 `@theme`) | `src/styles/index.css` |
| Primitives | `src/ui/*.tsx` (barrel: `src/ui/index.ts`) |
| Class merging (knows our type scale) | `src/lib/cn.ts` |
| Theme switching (`data-theme` on `<html>`, persisted in `localStorage["wt.theme"]`) | `src/lib/theme.tsx` |
| Gallery | `src/features/Design.tsx` -> `/design` |
| Brand assets, favicon | `public/brand/*`, `public/favicon*.png` |

## 1. Principles

1. **Depth comes from the surface ladder and hairlines, not shadows.** canvas -> panel -> surface-1..4. Cards are one step above the panel with a 1px hairline.
2. **One chromatic accent: lavender `#5e6ad2`.** Used for the primary action, focus ring, active tab/switch/checkbox and link emphasis only. Semantic colours (success/warning/danger/info) appear only in pills, alerts and toasts, always with a text label.
3. **Dense and keyboard-first.** 32px controls, 8px radius, list rows 44px, Cmd/Ctrl+K reaches every destination, every interactive element has a visible `:focus-visible` ring.
4. **Dark is the default, light is an equal.** Both themes are tokenised; no component hard-codes a colour. The theme is applied before first paint (inline script in `index.html`).
5. **Colour never carries meaning alone.** Status pills show a dot plus a text label; the mic state has text; errors have text and `role="alert"`.
6. **Motion is optional.** All transitions/animations are <= 180ms and are disabled under `prefers-reduced-motion: reduce`.

## 2. Tokens

All raw values are CSS variables on `:root` (dark) and `:root[data-theme="light"]`. Tailwind utilities
(`bg-surface-1`, `text-ink-muted`, `border-hairline`, ...) read them through `@theme inline`, so
switching `data-theme` re-colours everything without a rebuild.

### 2.1 Surface ladder and lines

| Token | Dark | Light | Use |
|---|---|---|---|
| `--canvas` | `#050506` | `#f1f2f4` | App shell: sidebar, top bar, page background behind everything |
| `--panel` | `#0a0b0c` | `#f7f8f9` | The page content area |
| `--surface-1` | `#131415` | `#ffffff` | Cards, inputs, tables, list containers |
| `--surface-2` | `#1c1d20` | `#f0f1f4` | Hover rows, raised cards, dialogs, nested fills |
| `--surface-3` | `#25272a` | `#e4e6ea` | Menus/popovers, active nav item, toasts |
| `--surface-4` | `#2d2f33` | `#d0d3da` | Highlighted menu item, switch track, deepest lift |
| `--hairline` | `#232428` | `#e3e5e9` | 1px borders on cards, dividers |
| `--hairline-strong` | `#313338` | `#d0d3da` | Popover/dialog outline, kbd, dashed empty-state frame (decorative only) |
| `--hairline-tertiary` | `#3e3e44` | `#9ca3af` | Decorative stronger divider (not a control boundary) |
| `--control-border` (`border-control`) | `#6b7078` | `#7c838f` | **Visual boundary of inputs, selects, checkboxes, switch track, secondary buttons.** >= 3:1 on surface-1..3 (WCAG 1.4.11): dark 3.7 / 3.4 / 3.0, light 3.8 / 3.4 / 3.1 |

### 2.2 Ink (text)

| Token | Dark | Light | Use |
|---|---|---|---|
| `--ink` | `#f7f8f8` | `#111214` | Headlines, body, emphasised text |
| `--ink-muted` | `#d0d6e0` | `#3b3f47` | Secondary text, labels |
| `--ink-subtle` | `#9a9fa8` | `#5a606b` | Descriptions, meta, icons |
| `--ink-tertiary` | `#8d929b` | `#5f6672` | Placeholders and hints only. >= 4.5:1 on surface-1, -2 and -3 (dark 5.9 / 5.4 / 4.8, light 5.8 / 5.1 / 4.6). **Never on surface-4** (4.3 / 3.9): use `ink-subtle` there |

### 2.3 Brand and semantic

| Token | Dark | Light | Use |
|---|---|---|---|
| `--primary` | `#5e6ad2` | `#5e6ad2` | Primary button, active controls (white text: 4.7:1) |
| `--primary-hover` | `#5a66cc` | `#4f5bc4` | Hover. White text 5.0:1 dark / 5.8:1 light (was 3.8:1 in dark) |
| `--primary-pressed` | `#4f5bbf` | `#4651b3` | Active/pressed (5.9:1 / 6.6:1) |
| `--link` | `#8b96ff` | `#4651b3` | Links and tinted primary text |
| `--focus` | `#7c86ea` | `#5e6ad2` | Focus ring colour (drawn at 75% alpha) |
| `--success` / `-soft` / `-text` | `#3fbf5f` / `#11291a` / `#5fd47e` | `#1c8a3a` / `#e6f6ea` / `#167030` | Live, connected |
| `--warning` / `-soft` / `-text` | `#f59e0b` / `#33250a` / `#f5b03d` | `#b45309` / `#fdf2dc` / `#8f4207` | Waiting, lobby, paused |
| `--danger` / `-soft` / `-text` | `#d6363b` / `#3a1618` / `#ff8b8f` | `#d6363b` / `#fdecec` / `#b4252a` | Errors, destructive buttons (white text 4.7:1) |
| `--info` / `-soft` / `-text` | `#6ba4f0` / `#14233a` / `#8dbbf5` | `#2f6fd0` / `#e5eefb` / `#22569f` | Scheduled, open, informational |
| `--overlay` | `rgba(0,0,0,.6)` | `rgba(17,18,20,.45)` | Dialog scrim |

Utilities: `bg-danger`, `text-danger-text`, `bg-warning-soft`, `text-on-primary`, ... Use `-soft` backgrounds with `-text` ink for pills and alerts.

### 2.4 Radius

| Token | px | Use |
|---|---|---|
| `rounded-xs` | 4 | Kbd, checkbox |
| `rounded-sm` | 6 | Inline tags |
| `rounded-md` | 8 | **All buttons and form controls**, menu items |
| `rounded-lg` | 12 | Cards, alerts, tables, toasts |
| `rounded-xl` | 16 | Dialogs, command menu, meeting control bar |
| `rounded-xxl` | 24 | Rare oversized panels |
| `rounded-pill` | 9999 | Status pills, switch, avatars |

CTAs are never pill-shaped.

### 2.5 Spacing

4px base: `1`=4, `2`=8, `3`=12, `4`=16, `6`=24, `8`=32, `12`=48 (Tailwind steps). Card padding 20px (`p-5`), dialog padding 24px, page padding 24px (16px under 640px), list row min-height 44px, controls 32px (`md`), 28px (`sm`), 40px (`lg`), inputs 36px.

### 2.6 Type scale

Family: Inter, self-hosted (`@fontsource-variable/inter`, imported in `main.tsx`; family name `Inter Variable`) with the SF Pro / system fallback stack. It is loaded, not assumed to be installed. Mono for codes and JSON: `ui-monospace, SF Mono`.
Weights: 400 body, 500 labels/buttons, 600 headings (never 700+). Display sizes track aggressively negative; body tracks -0.05px; the eyebrow is the only positive tracking.

| Utility | Size / line-height | Tracking | Use |
|---|---|---|---|
| `text-display-xl` | 80 / 1.05 | -3px | Marketing only |
| `text-display-lg` | 56 / 1.10 | -1.8px | Marketing only |
| `text-display-md` | 40 / 1.15 | -1px | Gallery title |
| `text-headline` | 28 / 1.20 | -0.6px | Page titles (`PageHeader`) |
| `text-card-title` | 22 / 1.25 | -0.4px | Auth card titles |
| `text-title` | 18 / 1.35 | -0.2px | Card and dialog titles |
| `text-body-lg` | 16 / 1.5 | -0.1px | Lead text |
| `text-body` | 14 / 1.5 | -0.05px | Default UI text |
| `text-body-sm` | 13 / 1.45 | 0 | Hints, dense table text, small buttons |
| `text-caption` | 12 / 1.4 | 0 | Meta, pills, kbd |
| `text-eyebrow` | 11 / 1.3 | +0.4px | Uppercase table headers, group labels |

`cn()` registers these names with `tailwind-merge`; always combine classes through `cn()` or a `cva` so `text-body` (size) never overrides `text-on-primary` (colour).

### 2.7 Elevation and focus

| Level | Treatment |
|---|---|
| 0 | Flat |
| 1 | `surface-1` + 1px `hairline` + top edge highlight (`.edge-top`) |
| 2 | `surface-2` + `hairline-strong` (raised card, dialog) |
| 3 | `surface-3` + `shadow-pop` (menus, popovers, toasts) |
| Focus | `outline: 2px solid color-mix(--focus 75%)`, offset 2px, **only on `:focus-visible`** (global rule; `.focus-ring` utility for components) |

### 2.8 Motion

`animate-fade-in` 120ms, `animate-pop-in` 140ms, `animate-slide-up` 180ms, `animate-shimmer` (skeleton). A global `@media (prefers-reduced-motion: reduce)` rule collapses every animation and transition to ~0ms.

## 3. Components (`src/ui`)

| Component | API | Notes |
|---|---|---|
| `Button` | `variant` primary \| secondary \| ghost \| danger \| danger-soft, `size` sm (28) \| md (32) \| lg (36, level with inputs) \| icon, `loading` | `loading` sets `aria-busy` and disables. Icon-only buttons **must** pass `aria-label`. |
| `Field` + `Input` / `Textarea` / `Select` | `<Field label hint error>` wraps one control | Generates the `id`, links label, hint and `aria-describedby`, sets `aria-invalid`. Errors render with `role="alert"`. |
| `Select` | `value`, `onValueChange`, `options` | Radix Select (role `combobox`, options `option`). Pass `aria-label` when not inside a `Field`. |
| `Checkbox`, `Switch` | `checked`, `onCheckedChange`, `label`, `description` | Label is bound with `htmlFor`; the whole row is clickable. |
| `Badge`, `StatusPill` | `tone` neutral \| primary \| success \| warning \| danger \| info; `StatusPill status="IN_PROGRESS"` | `StatusPill` maps room/participant/role statuses to a tone, a dot and a label ("Live" for `IN_PROGRESS`). Emits `data-status` for tests. |
| `Card`, `CardHeader`, `CardBody` | `raised` | |
| `Dialog` / `DialogContent` | `title` (required), `description`, `footer` | Radix: focus trap, Escape, `aria-labelledby`. Focus returns to the opener even when opened by URL/command (see `Dialog.tsx`). |
| `Dropdown*` | `DropdownItem icon shortcut danger` | Radix menu: arrow keys, typeahead. |
| `CommandMenuProvider` / `useCommandMenu` | `items: {id, group, label, icon?, shortcut?, run}[]` | Cmd/Ctrl+K toggles; cmdk filtering; dialog named "Command menu", input named "Search commands". |
| `Drawer` | `open`, `onOpenChange`, `title` (required), `description` | Right-hand sheet on `surface-2` (Radix dialog: focus trap, Escape, focus return). Body is a focusable scroll region. Used for the global-glossary audit history. |
| `Tabs*` | Radix Tabs | Underline style; active = ink + primary underline. |
| `Tooltip` | `content`, `side` | Never the only carrier of essential info. |
| `ToastProvider` / `useToast()` | `toast({tone, title, description})` | `role="region"` + polite live region; 4.5s (8s for errors). |
| `Skeleton`, `EmptyState`, `Alert` | | Empty states explain what belongs there and offer the next step. |
| `Avatar` | `name`, `size` | Deterministic colour from the name; palette is >= 4.5:1 against white initials. |
| `Kbd` | | Shortcut hints. |
| `SidebarNav`, `SidebarLink` | `heading`, `to`, `icon`, `badge`, `actions` | Legacy rows: 30px, 13px/500, icon `ink-muted/80`, active = surface-3 + ink, hover = surface-3/60; hover actions (join by code, new meeting) stay in the tab order. Icons are Phosphor `duotone` 16px. |
| `Table`, `THead`, `TBody`, `TR`, `TH`, `TD`, `ListRow` | `bare` | Tables always get an `aria-label`; `TH` is `scope="col"`. **Inside a `Card` use `bare`** (no second frame); the wrapper scrolls itself. Below 640px render a stacked list instead of a table. `ListRow` is the dense issue-style row. |
| `PageHeader`, `Brand` | | `Brand` renders the real legacy PNG (`public/brand/`), recoloured per theme by `.brand-img`. `PageHeader` clamps long titles and exposes the full title as a tooltip. |

## 4. Usage rules

- Compose class names with `cn()`; never concatenate strings that include Tailwind colour/size tokens.
- Icons: `@phosphor-icons/react`, `weight="duotone"` for navigation and empty states, `bold`/`regular` for inline glyphs; give them `size`/`size-N`. `lucide-react` has been removed from the project.
- Tertiary ink usage: placeholders and hints on surface-1..3 only. Control borders use `border-control`; `hairline*` never draws an input boundary.
- Destructive: solid `danger` only for the confirm button of a dialog; in-page End/Leave/Revoke use `danger-soft`. One primary per region; destructive page actions go in a "More" menu with a confirm dialog.
- Long text: `truncate` plus a `title` tooltip; rows stack below 640px; no page may scroll horizontally at 390px (asserted in e2e).
- Use tokens, not hex: `bg-surface-1`, `text-ink-subtle`. The only literals allowed are the avatar palette and the brand SVG.
- One primary button per view region. Destructive actions use `danger` and live next to a plain cancel.
- Every control needs an accessible name: visible label (`Field`), visible text (buttons), or `aria-label` (icon buttons). **The accessible name must contain the visible text** (WCAG 2.5.3, enforced by axe `label-content-name-mismatch`).
- Add `data-testid` only when role + name is ambiguous (account menu, workspace switcher, room code, lobby card, participant grid).
- Lists of records: `ListRow` for scannable rows with a status pill on the right; `Table` for multi-column data.
- Loading: `Skeleton` blocks matching the final layout. Errors: `Alert tone="danger"` with the API `message`. Never show raw JSON errors.
- Status colours are reserved for state. Do not use `success` for decoration.

## 5. Do and don't

| Do | Don't |
|---|---|
| Lift with the ladder (`canvas -> panel -> surface-1`) | Add drop shadows to cards |
| Use lavender for the primary action, focus and active state | Use lavender as a section background or card fill |
| Keep display weights at 600 and track negatively | Use weight 700+ or positive tracking on headings |
| Round controls to 8px, cards to 12px | Pill-round CTAs |
| Ship both themes for every new component and check `/design` | Hard-code `#fff` / `#000` or dark-only colours |
| Pair colour with text/icon for status | Rely on colour alone (e.g. red/green dot) |
| Use `#050506` (blue-tinted) as the dark canvas | Use pure `#000000` as the canvas |
| Respect `prefers-reduced-motion` (already global) | Add infinite animations without a reduced-motion fallback |
| Use `type="button"` for non-submit buttons (Button does) | Rely on hover for essential information |

## 6. Themes

- Dark (`:root`, default) and light (`:root[data-theme="light"]`).
- First paint: the inline script in `index.html` reads `localStorage["wt.theme"]` and sets `data-theme` before React mounts.
- Dev-only: the sidebar "Design system" link and command are shown only when `import.meta.env.DEV`; `/design` stays reachable by URL.
- Toggles: top bar button, account menu, Settings -> Account, command menu ("Toggle theme").
- Unsupported on purpose: system-follow. The macOS shell can set `data-theme` itself via `localStorage` if it wants to follow the OS.

## 7. Accessibility baseline

- axe (WCAG 2.0/2.1 A + AA) reports no serious/critical violations on the main pages in both themes (`e2e/tests/a11y.spec.ts`).
- Contrast: body text >= 4.5:1 on every surface; primary (rest, hover, pressed) and danger buttons carry white text at >= 4.7:1; control borders >= 3:1.
- Landmarks: `Skip to content` link, `<aside aria-label="Sidebar">`, `<header>` with a `nav aria-label="Breadcrumb"`, `<main id="main">`, `<nav aria-label>`. The mobile drawer is a Radix dialog named "Navigation" (focus trap, Escape, focus return).
- Forms: errors are per field (`Field error` -> `aria-invalid`, `role=alert`), the first invalid field is focused; a focused invalid control draws a danger ring.
- Keyboard: Cmd/Ctrl+K, `G` then `H`/`M`/`P`/`K`/`S`/`E` (go to Home, Meetings, Members, Knowledge, Settings, External), `C` create meeting, `J` join by code (ignored while typing or when a dialog/menu is open; the hints in the command menu are the only source of truth), dialogs trap focus and restore it, menus use arrow keys, focus ring is visible on every interactive element.
