# Legacy WarpTalk web vs new web: visual audit (LIGHT theme focus)

Legacy = `warptalk-web` (Next 16 + Tailwind v4, run from a scratch copy on :3100, API mocked in Playwright).
New = `apps/web` (Vite, :5199, real Rust backend + `demo-seed.sh`).
All numbers are `getComputedStyle` values at 1440x900 (DPR 2) from the running legacy app unless marked "(src)" (read from source).
Colors written as `#hex/NN` are the hex at NN% alpha. Legacy chip colors are Tailwind v4 palette (computed `lab()/oklab()` converted to hex).

Screenshots
- Legacy: `apps/web/audit/legacy/*-light.png` (home, meetings, room-detail, room-detail-live, members, knowledge, documents, settings, prejoin = live meeting room, in-meeting = dev test page), `*-dark.png` (home, meetings, members), close-ups `closeup-*.png`.
- New: `apps/web/audit/compare/new/*.png` (same names).
- Side by side (left legacy, right new): `apps/web/audit/compare/<page>.png`: `home, meetings, room-detail, members, knowledge, documents, settings, prejoin, dark-home, dark-meetings, dark-members, closeup-sidebar, closeup-shell-topleft, closeup-cards-home, closeup-card-settings, closeup-chips-meetings, closeup-list-rows, closeup-table-members, closeup-roomdetail-header`.

## 0. The headline: the shell is a three-layer ground, not a flat page

Legacy (`src/app/(app)/layout.tsx:596-611`):

```
<div class="relative h-dvh flex overflow-hidden bg-canvas">                 // #f1f2f4 fills the whole window
  <AnimatedWidthPanel width=224> <LinearSidebar/> </AnimatedWidthPanel>       // sidebar = recessed layer, NO border, NO own radius
  <div class="relative flex flex-col flex-1 overflow-hidden min-w-0">        // main column
    <div class="flex flex-col flex-1 overflow-hidden mt-1.5 mr-1.5 mb-0
                rounded-xl border border-border bg-panel shadow-sm">          // RAISED PANEL, 4 round corners
       <header class="h-[44px] ... border-b border-border"> ... </header>
       <main class="flex-1 overflow-y-auto"> page </main>
    </div>
    <div class="shrink-0 mr-1.5"><GlobalChatbot/></div>                       // 36px "Ask WarpBot" strip under the panel
  </div>
</div>
```
Measured: panel rect x=224 y=6 w=1210 h=858 (window 1440x900). Inset = top 6, right 6, bottom 36 (the WarpBot strip), left 0 (butts the sidebar column, whose items are padded 12px so the panel edge reads as the sidebar's edge). Radius is 16px on ALL four corners (`rounded-xl` = 16px because the theme redefines `--radius-xl: 16px`). The 1px border `#e3e5e9` and `shadow-sm` outline it.

New (`src/features/Shell.tsx:277-290`): `flex h-dvh bg-canvas` > `aside.w-60 border-r border-hairline bg-canvas` + column with `TopBar (h-12 border-b bg-canvas)` + `main.bg-panel` flat, edge to edge, no radius, no inset, no shadow, topbar is on the canvas layer outside any panel.

## 1. P0 Shell / sidebar / main panel

| Aspect | Legacy | New | Fix |
|---|---|---|---|
| Window ground | `bg-canvas` `#f1f2f4` on root | root `bg-canvas` (transparent, body canvas) | keep, ok |
| Sidebar width | 224px (rail 64px collapsed) | 240px (`w-60`) | `w-56` (224px) |
| Sidebar bg | `#f1f2f4` (canvas), no border (expanded) | `#f1f2f4` + `border-r 1px #e3e5e9` | remove `border-r` on `aside` (panel border does the separation) |
| Sidebar header | 48px, `px-3`, no divider; workspace switcher = 20px gradient (pink-500 to rose-500) rounded square + 14px/600 name + caret, `-ml-1.5 px-1.5 py-1 rounded-md`; search icon button right | brand logo row, then separate 40px switcher row | one 48px row: switcher left, search icon right; brand mark not shown here |
| Sidebar nav | `px-3 py-3`, items `gap-px` | `p-3 gap-4`, items 30px | keep 30px rows, `gap-px` (1px) |
| Nav item | height 30px, `px-2` (8px), `rounded-[8px]`, 13px/500, tracking -0.325px, icon 16px duotone `gap-2.5` (10px) | height 30px, `px-2`, `rounded-8`, 13px/500 (same) | match icon weight "duotone" and gap 10px; rest ok |
| Nav active | row bg `#e4e6ea` (surface-3), text `#111214`, icon `#111214` | `#e4e6ea` (same) | ok |
| Nav idle | transparent, label `ink/90` (`#111214` at 90%), icon `ink-muted/80`; hover `surface-3/60` | label `#111214/90` | ok; set icon color `#5e6470` at 80% |
| Section label | 12px/500, color `#8a8f98`, NOT uppercase in workspace nav (uppercase `tracking-wider` only in Settings nav), row 24px tall `px-2`, `mb-1`, group `mb-3` | 11px/500 uppercase tracking .4px `#5a606b`, p 4px 8px | 12px/500, `#8a8f98`, no uppercase in main nav |
| User card (bottom) | white `#fff` card, `1px` border `#e3e5e9/50`, radius 16px, `shadow-sm`, padding 8px, margin 12px (`m-3`), avatar 28px + name 12.5px/500 + email 11px + role chip "Owner · Internal" (primary text) | borderless row, 44px, no card | wrap in raised card: `m-3 p-2 rounded-2xl border border-hairline/50 bg-surface-1 shadow-sm` |
| Main panel | `bg #f7f8f9`, 1px `#e3e5e9`, radius **16px x4**, `shadow 0 1px 3px rgba(0,0,0,.1), 0 1px 2px -1px rgba(0,0,0,.1)`, margins top 6 / right 6 / bottom 0 + 36px strip, left 0 | `main` flat `#f7f8f9`, no radius/border/shadow/margin | wrap `TopBar + main` in one `div.m-1.5 ml-0 mb-0 flex flex-col rounded-2xl border border-hairline bg-panel shadow-sm overflow-hidden`; bottom strip (36px) or `mb-1.5` |
| Top bar | INSIDE the panel, `h-44px`, grid `1fr / 420px / 1fr`, `px-4`, `border-b 1px #e3e5e9`, transparent bg | OUTSIDE panel, `h-12` (48px), `bg-canvas`, `border-b` | move into panel, `h-11`, transparent, 3-col grid with centered search |
| Top bar contents | collapse-sidebar icon (24px, r6), breadcrumb 13px/`#5e6470` current `#111214`/500, `+` tab add; centered search; right: 24px round icon buttons (bell, sound, theme, language, help) `border #e3e5e9`, `bg-white`, shadow `0 1px 2px rgba(0,0,0,.04)`, divider 1x14px, right-panel toggle | breadcrumb 14px/500, search right (256x32) + theme button | centered search; icon buttons per legacy |
| Search | 388x28, `#fff`, 1px `#e3e5e9`, radius 8, 12px text `#5e6470`, `⌘K` kbd chip | 256x32, 1px **`#7c838f`** (control border), radius 8, 13px | border `#e3e5e9`, h 28, w up to 420 centered |
| Page padding | none from shell: each page owns its layout; toolbars are edge to edge (`px-4`) | `max-w-6xl px-6 py-8` centered column | drop the centered max-width wrapper; pages are full-bleed lists/toolbars (see grid lines) |

## 2. P0 Cards

| Aspect | Legacy | New | Fix |
|---|---|---|---|
| Card bg | `#ffffff` (surface-1) on `#f7f8f9` panel | `#ffffff` | ok |
| Border | 1px `#e3e5e9` (settings card, home quick card). Sub-lists inside use `border-border/40..60` | 1px `#e3e5e9` | ok |
| Radius | quick-jump cards 12px; settings card 12px; room-detail "People" card 10px; sidebar user card 16px; dashed placeholder card 8px; main panel 16px | 12px everywhere (`rounded-lg`) | keep 12 for cards, add 16 for panel/user card, 8 for nested/dashed |
| Shadow | `0 1px 1px rgba(17,18,20,.035)` (`--shadow-card`); settings card none; people card `0 1px 2px rgba(0,0,0,.04)`; featured CTA card `0 12px 26px rgba(94,106,210,.24)` | `inset 0 1px 0 rgba(255,255,255,.6), 0 1px 1px rgba(17,18,20,.035)` | drop the inset `edge-top` highlight on light theme |
| Padding | quick card `p-3` (12px) h min 56; people card `p-3`; settings rows `px-5 py-4`-ish (row 88px tall); knowledge cell `px-4 py-3` | `CardBody p-5` (20px), header `px-5 py-4` | reduce to `p-3` (compact) / `px-4 py-3`; header `px-4 py-3` |
| Card header | no header band on most cards; titles are 15px/600 `#111214` section headings OUTSIDE the card (`h2` 22.5px line) with uppercase 11px/600 `tracking .55px` `#8a8f98` group labels above settings cards | header band with `border-b` + 18px/600 title + subtitle 14px | move to section label above card (11px/600 uppercase `#8a8f98`) and drop header band `border-b` where possible |
| Featured card | `bg #5e6ad2`, border `#5e6ad2`, text white, icon tile `white/12` + `border white/20` | n/a | add "featured" variant to quick actions |
| Quick-action card | icon tile 36px r9 `bg #f0f1f4` border `#e3e5e9` icon `#5e6ad2`; label 13px/500; `hover -translate-y-0.5 border #d0d3da bg #f0f1f4` | 2-line card (title + description) | title only, 56px high, tile 36px |
| Dashed empty/placeholder | 1px **dashed** `#e3e5e9`, radius 8, white, p 12x14 | solid EmptyState | use dashed variant for "still processing / nothing yet" |

## 3. P0 Chips, badges, status pills

Legacy rule: every chip is `bg-{color}/10` (or /5), `border 1px {color}/20..30`, text = a darker shade of the same hue, padding `2px 8px`, weight 500-600, size 10-11px. New rule: no border, soft solid fill, 12px/500, always full pill, leading dot.

| Chip | Legacy (light) | New | Fix |
|---|---|---|---|
| Status pill (meetings list) `In progress` | h 22.5, pad 2/8, radius pill, 11px/500 capitalize, text `#00bc7d`, bg `#00bc7d/10`, border `#00bc7d/30`, leading 12px ring dot | h 20.8, 12px/500, bg `#e5eefb`, text `#22569f`, no border, dot filled | switch to tinted-border style below |
| `Scheduled` | text `#10b981`, bg `#10b981/10`, border `#10b981/20`, ring icon | blue `#e5eefb`/`#22569f` (info tone) | emerald |
| `Waiting` | text `#f59e0b`, bg `#f59e0b/10`, border `#f59e0b/20` | amber soft `#fdf2dc`/`#8f4207` | amber-500 |
| `Paused` / neutral | text `#5e6470`, bg `#f0f1f4`, border `#e3e5e9/50`, empty ring icon | `#f0f1f4` / `#3b3f47` | as legacy |
| Ended | icon CheckCircle fill `#6b7280` + text `#6b7280`, no bg | n/a | gray, no fill |
| Cancelled/failed | ring `#ef4444` (border 1.5px, bg `/20`) | danger soft | red-500 |
| Status dot (row lead icon) | 12px circle, 1.5px border `status-*` + `bg status-*/20`; in-progress adds `0 0 8px` glow (`#3b82f6`, `#f59e0b`, `#10b981`, `#6b7280`, `#ef4444`) | dot inside pill | add 12px ring icon at row start (list) |
| Role chip `Member/Owner/Admin` | h 20, pad 2/8, radius **4px**, 10px/600 capitalize, bg `#f0f1f4`, border `#e3e5e9`, text `#111214` | pill, 12px/500, owner = `primary/15` + `#4651b3` | square-ish 4px chip, neutral |
| Membership `Internal` | radius 4, 10px/600, text `#0069a8`, bg `#00a6f4/10`, border `#00a6f4/25` | n/a | sky tint |
| Membership `External` | radius 4, 10px/600, text `#bb4d00`, bg `#fe9a00/10`, border `#fe9a00/25` | n/a | amber tint |
| Member status `Joined` | h 20, radius **8px**, 10px/500, text `#00d492`, bg `#00bc7d/5`, border `#00bc7d/20` | n/a | emerald |
| `Invited` | radius 8, 10px/500, text `#fe9a00`, bg `#fe9a00/5`, border `#fe9a00/20` | n/a | amber |
| `You` marker | radius 8, 10px/400, text `#5e6ad2`, bg `#5e6ad2/10`, border `#5e6ad2/20`, pad 0/4 | n/a | primary tint |
| Knowledge `AI Ready` | h 22.5, radius pill, 11px/500, text `#009966`, bg `#00bc7d/10`, border `#00bc7d/20`, leading dot | n/a | emerald |
| Knowledge `Disabled` | text `#ec003f`, bg `#ff2056/10`, border `#ff2056/20` | n/a | rose |
| Fact category `Decision` | radius 8, 10px/400 capitalize, `#5e6470` on `#f0f1f4`, pad 2/6, mt 6, no border | n/a | neutral tag |
| Document status | 10px/600, h 19, pill, tinted (AI Context = `#5e6ad2` tint, Pending = amber, Rejected text `#e5484d`, bg `#e5484d/10`, border `#e5484d/20`) with 12px leading icon | n/a | same tint recipe |
| Meeting-record badge `Not shared yet` | h 22.5, pill, 11px/500, `#bb4d00` on `#fe9a00/10`, border `#fe9a00/30` | n/a | amber tint |
| Info pills in room header/list (host, languages, occupancy, date, code) | **26px (row) / 28px (detail)** high capsules: `bg #fff`, 1px `#e3e5e9/60`, `px-2.5`, radius pill, 11px, `shadow 0 1px 2px rgba(0,0,0,.02)`; code chip in mono 11px; host chip has 20px avatar | plain muted caption text + gray mono code chip (`#f0f1f4`, r8, 12px) | render host/langs/occupancy/date as white bordered capsules (this is the single biggest "chip" difference) |
| Language chip `VI EN JA` | 11px/400 `#5e6470`, tiny `#f0f1f4` pill, separated by `·` | text "Vietnamese -> English, Japanese" | 2-letter caps chips inside one capsule |
| Count badge (tabs `Invited 1`) | 18px circle-ish `bg ink/8 (#1112140.08)`, 11px/500, `#5e6470` | none | add to tabs |
| Recurring badge | r6, 10px/500 `#5e6ad2` on `primary/10` border `primary/20` | n/a | n/a |

Hex recipe for the new `Badge` (light): `bg:{c}/10 border:1px {c}/20 text:{c-dark}`; sizes `sm` 10px/500 `py-0.5 px-2 h-5`, `md` 11px/500 `h-[22.5px]`; radius 4 for categorical chips (role, membership), 8 for member status/tags, `full` for status pills.
Palette: emerald `#00bc7d` (text-dark `#009966`), amber `#fe9a00` (text-dark `#bb4d00`; list status uses `#f59e0b`), sky `#00a6f4` (text-dark `#0069a8`), rose `#ff2056` (text-dark `#ec003f`), red `#e5484d`, primary `#5e6ad2`, neutral `#f0f1f4`/`#5e6470`.

## 4. P0 Grid-line layout (hairlines instead of boxes)

Legacy pages are NOT stacks of floating cards in a centered column. They are full-bleed bands separated by 1px hairlines that run from panel edge to panel edge:

| Where | Legacy | New | Fix |
|---|---|---|---|
| Top bar | `border-b 1px #e3e5e9` (full panel width) | `border-b` on canvas bar | keep full-width line, inside panel |
| Toolbar/tab strip under header | `flex items-center gap-3 border-b border-border/40 px-4 pb-3`; line = `#e3e5e9` at 40% = `#eff0f3`; full width | tabs in centered column w/ 2px primary underline; filter row separate | pill tabs (below) + `border-b border-hairline/40` strip, `px-4` |
| List group header | h 30px, `px-4`, 12px `#5e6470`, `border-b /40`, sticky `bg #fff/90 + backdrop-blur` (renders as a white band, x 225..1433) | none | add group header band |
| List row | `min-h-[44px] py-1 px-4 border-b border-border/40`, full-bleed (x 225..1433), hover `bg accent/50` | 64.8px rows inside a bordered/rounded card with inset 288px margins; `border-b #e3e5e9` | remove card wrapper; rows full-bleed 44px, `border-b #e3e5e9/40` |
| Table (members) | thead row `border-b /40` (11px/600 uppercase `#5e6470`, p 8px); body rows h 57px `py-3 px-2`, `border-b /40`, `rounded-[8px]` hover | n/a (card list) | same classes |
| Table (documents) | `th` 11px/500 uppercase `.275px` `#5e6470` p 10/16; `tr border-b` `#e3e5e9/40` | n/a | same |
| Table (knowledge) | card wrapper 1174px wide, **`border 1px` rounded 12 white**, th 10px/500 uppercase `.25px` `#8a8f98` p 10/16, `tr border-b #e3e5e9/60`, cell p 12/16 | n/a | same |
| Room detail | sections separated by full-width `1px` rules (`#e3e5e9`) with 24-32px air; right "People" card 300px wide, radius 10, sticky height | stacked Cards | rules between sections, side rail card |
| Settings | 640px column; group label (11px/600 uppercase `.55px` `#8a8f98`) then ONE white card (r12, 1px `#e3e5e9`) whose rows are separated by `border-b #e3e5e9`; row = label 13px/600 + 12px muted description left, control right (`#f0f1f4` input, r8, 40px) | many separate Cards with header bands | one card per group, rows divided by hairlines |
| Settings left nav | replaces the app sidebar (same canvas layer): "Back to app", uppercase 12px/500 `.6px` `#8a8f98` group labels, 30px items, active `#e4e6ea` | tab-ish section nav inside page | swap sidebar content on `/settings` |
| Divider tokens | `--border/--hairline #e3e5e9`, `/60` = `#ebedef` on panel, `/40` = `#eff0f3`, `--hairline-strong #d0d3da`, `--hairline-tertiary #9ca3af` | `--hairline #e3e5e9`, `--control-border #7c838f` used for everything incl. search/inputs | use `hairline/40..60` for row grids; reserve `control-border` for focusable inputs only |

Tabs/filters: legacy uses **pill tabs** (not underline): h 33.5, `px-4 py-1.5`, radius full, 13px/500 capitalize; active bg `#f0f1f4` text `#111214` border transparent; idle transparent text `#5e6470` border `#e3e5e9/40`; secondary filter pills identical. New: underline tabs 36px, 14px, active underline 2px `#5e6ad2`.
Buttons: legacy primary = ink (`#111214`) pill, h 28, `pl-2.5 pr-3`, 13px/500, text `#f1f2f4`, shadow-sm (radius full). Secondary/icon = 28px round, `bg #f0f1f4`, border `#e3e5e9/60`, shadow-sm. New: primary = `#5e6ad2` r8 h32 14px; secondary = outlined with `#7c838f`. Primary indigo is used only on featured cards / links / active day marker in legacy.

## 5. P1 Typography and tokens

| Item | Legacy | New | Fix |
|---|---|---|---|
| Base size | **13px** body (rows, nav, buttons, cells) tracking -0.24px (letter-spacing on body) | 14px, -0.05px | base 13px; set body tracking `-0.24px` |
| Secondary | 11px (chips, meta, th), 12px (search, labels), 10px (small chips, th in knowledge) | 12px captions | add 10/11/12 scale |
| Page title | list pages: title lives in breadcrumb (13px/500); detail h1 **30px/600** lh 37.5 tracking -0.75; settings h1 **20px/700** lh 28 -0.5; section h2 15px/600 | h1 28px/600 -0.6 on every page + 14-16px description | no big h1 on list pages (breadcrumb only); detail 30px, settings 20px |
| Font family | system UI stack (`ui-sans-serif, system-ui, -apple-system, "Segoe UI"...`); mono for codes (`ui-monospace, SFMono...`) 11px | Inter-ish | use legacy stack (Inter @import is loaded but `--font-sans` is the system stack) |
| Ink | `#111214`; muted `#5e6470`; subtle `#8a8f98`; tertiary `#62666d` | ink `#111214`; muted `#3b3f47`; subtle `#5a606b`; tertiary `#5f6672` | muted `#5e6470`, subtle `#8a8f98` (decorative only; keep AA in new if needed) |
| Surfaces | canvas `#f1f2f4`, panel `#f7f8f9`, surface-1 `#ffffff`, surface-2 `#f0f1f4`, surface-3 `#e4e6ea`, surface-4 `#d0d3da` | identical | ok |
| Borders | `--border #e3e5e9`, `--input #e3e5e9`, hairline-strong `#d0d3da`, tertiary `#9ca3af` | + `--control-border #7c838f` for inputs/buttons | inputs: `#e3e5e9` bg `#fff` (settings inputs use bg `#f0f1f4` + border); keep control-border only where a11y requires |
| Accent | primary `#5e6ad2`, hover `#828fff`, ring `#5e6ad2`, destructive `#e5484d`, success `#27a644`, warning `#d97706`, info `#2f6fd0` | similar | ok |
| Radius scale | xs4 sm6 md8 lg12 xl16 xxl24 pill | rounded-8/12 mostly | add 16 (panel), 4 (chips), 10 (side cards) |
| Shadows | card `0 1px 1px rgba(17,18,20,.035)`; panel/primary btn `0 1px 3px rgba(0,0,0,.1),0 1px 2px -1px rgba(0,0,0,.1)`; header icon btn `0 1px 2px rgba(0,0,0,.04)`; chip capsule `0 1px 2px rgba(0,0,0,.02)` | `edge-top` highlight + card | see above |
| Dark (secondary) | canvas `#050506`, panel `#0a0b0c`, surface-1 `#131415`, surface-2 `#1c1d20`, surface-3 `#25272a`, border `#232428`, ink `#f7f8f8`, muted `#8a8f98`; same structure (panel raised r16 over canvas) | tokens already match | structural fixes carry over |
| Rail (in meeting) | 64px sidebar rail, 36px round icon items, active `surface-3`; panel full height; captions panel + floating pill control bar (white capsule, shadow-sm) bottom | n/a | P2: meeting page chrome |

## 6. Ordered fix list

P0 shell / sidebar / main panel
1. Wrap `TopBar + main` in a raised panel: `m-1.5 ml-0 mb-0` (+36px bottom strip or `mb-1.5`), `rounded-2xl` (16px) on all corners, `border border-hairline bg-panel shadow-sm overflow-hidden`; root stays `bg-canvas`.
2. Sidebar: 224px, remove `border-r`, 48px header row (switcher + search), 12px section labels `#8a8f98`, bottom user card (white, r16, border/50, shadow-sm).
3. Top bar into the panel: 44px, transparent, `border-b`, 3-col grid with 420px centered search (28px, `#e3e5e9` border), round 24px icon buttons on the right.
4. Kill the `max-w-6xl px-6 py-8` page wrapper; pages own full-bleed toolbars and lists.

P0 cards
5. Card: keep `#fff`/`#e3e5e9`/12px, remove inset top highlight, shadow `0 1px 1px rgba(17,18,20,.035)`, padding 12-16px; headings become 11px uppercase group labels or 15px/600 outside the card; add dashed (r8) and featured (primary) variants; one settings card per group with hairline-divided rows.

P0 chips
6. Rebuild `Badge` as tinted-border chips (`bg c/10`, `border c/20`, dark text), 10/11px, add sizes and radius 4/8/full; remap status tones to emerald/amber/sky/rose; add 12px ring/dot status icons.
7. Info capsules (host, languages, occupancy, date, code): white, bordered `#e3e5e9/60`, h26/28, radius full, 11px, `shadow 0 1px 2px rgba(0,0,0,.02)`.

P0 grid lines
8. Meetings list: full-bleed 44px rows, `border-b #e3e5e9/40`, group headers (30px sticky), no card wrapper; add trailing fixed-width capsule columns.
9. Tables (members/documents/knowledge): header 11px/600 (or 10px/500) uppercase `#5e6470/#8a8f98`, row `border-b` at /40-/60, knowledge inside r12 white card.
10. Room detail: hairline-separated sections + 300px "People" side card; settings: grouped hairline cards (item 5).

P1
11. Pill tabs/filters (h33.5, r-full, `#f0f1f4` active) with count badges; ink pill primary button (h28) and round 28px icon buttons.
12. Type scale: 13px base, -0.24px tracking, h1 30 (detail) / 20 (settings), 11px th/meta; muted `#5e6470`, subtle `#8a8f98`.
13. Settings nav swaps the sidebar content (Back to app + uppercase group labels).
14. Dark theme: same structure; tokens already match.
15. Meeting/pre-join chrome (64px rail, captions panel, floating control capsule); legacy pre-join could not be reached without LiveKit (see below).

## 7. How the legacy app was rendered (for repeatability)
- Copied to `/tmp/claude-0/legacy-web`, `npm ci`, `next dev -p 3100` (Next 16/turbopack). Fonts from Google blocked (stubbed); system font stack is what the legacy CSS uses anyway.
- Auth: cookie `access_token` = unsigned JWT with future `exp`, cookie `warptalk_session=1`, localStorage `warptalk-auth` (`{state:{user,accessToken,isAuthenticated:true},version:0}`), `warptalk-onboarding` marks tour seen, `theme` = light|dark (next-themes).
- API (`http://localhost:5200/api/v1/**`) fully intercepted with `page.route`: `/auth/me`, `/workspaces` (paged), `/workspaces/:id/{select,settings,entitlements,members,invitations,documents,knowledge}`, `/subscriptions/workspace/:id`, `/credits/workspace/:id`, `/translation-rooms{,/history,/my-meetings,/:id,/:id/participants}`, `/platform/status`, `/announcements/*` (array), others `{}`/`[]`.
- Scripts live in `/tmp/claude-0/audit-scripts/` (scratch, not in repo).
- Not rendered: glossary and billing pages crash on the shallow mocks; `/test-meeting` is a dev LiveKit test harness (dark, unrepresentative), so in-meeting is only the live-room page (`legacy/prejoin-light.png`, `legacy/room-detail-live-light.png`). New-app in-meeting needs LiveKit (unconfigured), so only its pre-join was captured.
- New app data: real backend + `demo-seed.sh`, plus 5 scheduled rooms and 2 text documents added via API; knowledge is empty (no AI keys), so `knowledge-light.png` shows the empty state, not rows.
