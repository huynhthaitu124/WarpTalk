# WarpTalk web: E2E test plan

Real stack: Vite dev server (`:5199`) -> real `warptalk-api` (`:18080`, Postgres + Redis, LiveKit **not** configured).
Run: `cd apps/web && npx playwright test` (starts `scripts/e2e-backend.sh` and `vite` itself).
Update screenshots: `npx playwright test --update-snapshots`.

Conventions
- Every test creates fresh random users/workspaces (`e2e-<label>-<hex>@example.com`), so the suite is re-runnable against a dirty DB.
- Email verification: the API only logs the token length. Fixtures mark users verified with `psql` (`fixtures/db.ts`, **test-only**). The register -> verify screen is still exercised through the UI.
- Signed-in state is arranged by seeding the refresh token into localStorage; the app boots through its real silent-refresh path. Auth specs use the real login form.
- Priority: P0 = must pass for any release, P1 = important, P2 = nice to have / not automated yet (status `todo`).
- Projects: `chromium` (all specs), `macos-webview` (1280x800, WKWebView-style UA; specs `webview`, `visual`, `a11y`).
- Status: `auto` = implemented, `todo` = planned only (reason in Notes).

## 1. Auth (AUTH)

| ID | Pri | Preconditions | Steps | Expected | Status |
|---|---|---|---|---|---|
| AUTH-01 | P0 | none | Register via form (name, email, password) | Redirect to `/verify-email` with "Check your inbox" instructions; after fixture verifies the user, login works and lands on `/rooms`, account name visible in sidebar | auto |
| AUTH-02 | P0 | verified user | Login with wrong password | Error alert "invalid email or password", still on `/login` | auto |
| AUTH-03 | P0 | registered, unverified user | Login | Alert "email address is not verified" with "Verify your email" link to `/verify-email` | auto |
| AUTH-04 | P0 | signed out | Open `/settings` | Redirect to `/login`; after login returns to `/settings` | auto |
| AUTH-05 | P0 | signed in | Account menu -> Sign out; reload | Lands on `/login`; refresh token removed from storage; old refresh token rejected by API (401) | auto |
| AUTH-06 | P0 | signed in | Reload page twice | Stays signed in (silent refresh), stored refresh token rotates each time; old token no longer accepted | auto |
| AUTH-07 | P1 | signed in | Server answers 401 to all calls bearing the current access token while several queries are in flight | Exactly one `POST /auth/refresh`; all requests retried and page renders | auto |
| AUTH-08 | P1 | none | Open `/verify-email`, submit a bogus token | Error alert shown, no crash | auto |
| AUTH-09 | P1 | none | Register with password < 8 chars; register with an existing email | Inline validation error; API error "already" shown | auto |
| AUTH-10 | P2 | token capture | `/verify-email?token=...` auto-verifies | Success state | todo (backend exposes no token in dev; needs a dev-only capture endpoint) |
| AUTH-11 | P2 | expired refresh token | Reload | Redirect to `/login` | todo (needs short TTL env) |

## 2. Workspaces (WS)

| ID | Pri | Preconditions | Steps | Expected | Status |
|---|---|---|---|---|---|
| WS-01 | P0 | new user, no workspace | Open `/rooms` -> "Create workspace" dialog -> submit | Toast, workspace name in sidebar switcher, rooms empty state | auto |
| WS-02 | P0 | user with 2 workspaces | Switcher -> pick the other; reload | Switcher shows the picked one, persists after reload | auto |
| WS-03 | P1 | owner | Settings -> Members | Table lists owner with OWNER pill and email | auto |
| WS-04 | P1 | owner | Settings -> Workspace: change max participants, Save; reload | Toast; value persists | auto |
| WS-05 | P1 | owner | Set max participants to 0, Save | API validation error alert | auto |
| WS-06 | P2 | owner | `/workspaces` list | Roles shown, current marked | todo |

## 3. Rooms (ROOM)

| ID | Pri | Preconditions | Steps | Expected | Status |
|---|---|---|---|---|---|
| ROOM-01 | P0 | workspace | Create room dialog: title, source `vi`, targets `en`+`ja`, requires approval | Navigates to detail; title, "Waiting" pill, 12-char code, "Lobby on" | auto |
| ROOM-02 | P0 | 2 rooms (one started via API) | `/rooms` list; filter by status | Both listed with pills; filter "Live" shows only the started one | auto |
| ROOM-03 | P0 | waiting room, host | Start -> Pause -> Resume -> Stop translation -> End | Pills: Live, Paused, Live, Live, Ended; controls disappear after End | auto |
| ROOM-04 | P1 | waiting room | Cancel | Pill "Cancelled" | auto |
| ROOM-05 | P0 | approval room, host ctx + guest ctx | Guest opens `/rooms/:id`, joins; host sees lobby, admits | Guest sees "Waiting for the host"; host Lobby lists guest; after Admit guest banner clears and "Open meeting" appears; participants table shows guest CONNECTED | auto |
| ROOM-06 | P1 | guest in lobby | Host denies | Guest sees "You were removed from this room" | auto |
| ROOM-07 | P1 | approval room | Guest uses `/join` with code | Result "Waiting for the host to admit you"; code with wrong length shows validation; unknown code shows API error | auto |
| ROOM-08 | P1 | started room with admitted guest | Host: Generate routes | No error; routes table or empty-state visible | auto |
| ROOM-09 | P1 | dialog | Empty title, or all targets unchecked | Create button disabled | auto |
| ROOM-10 | P2 | room | Copy code toast | Toast "Room code copied" | todo (clipboard permission) |

## 4. Runtime settings (SET)

| ID | Pri | Preconditions | Steps | Expected | Status |
|---|---|---|---|---|---|
| SET-01 | P0 | owner | Settings -> Runtime -> suggestion/workspace: toggle enabled, min words 6, Save; reload | Toast; badge "Overridden"; values persisted | auto |
| SET-02 | P0 | owner | Raw JSON tab: broken JSON -> Save; then `min_confidence: 5` -> Save | Local "Invalid JSON" error; then API error alert with code | auto |
| SET-03 | P0 | workspace MEMBER (seeded) | Runtime tab; direct `PUT /settings/suggestion` as member | Read-only note, inputs disabled, no Save button; API answers 403 | auto |
| SET-04 | P1 | owner, room | pipeline namespace, room scope: save then reset | Overridden -> Inherited | auto |
| SET-05 | P1 | member | Workspace tab | Read-only note, inputs disabled | auto |
| SET-06 | P2 | member who hosts a room | Room scope editable | Save works | todo |

## 5. Meeting shell (MTG)

| ID | Pri | Preconditions | Steps | Expected | Status |
|---|---|---|---|---|---|
| MTG-01 | P0 | started room | `/meetings/:id` -> Join meeting | "LiveKit is not configured" state; roster shows host; mic toggle disabled; captions placeholder | auto |
| MTG-02 | P1 | MTG-01 state | Open listen-language select | Options: original + dubbed vi/en/ja; selection changes | auto |
| MTG-03 | P1 | started + waiting room | `/meetings` | Only live/open rooms listed; Open link goes to meeting | auto |
| MTG-04 | P1 | meeting not configured | "Try again" | Back to pre-join | auto |
| MTG-05 | P2 | LiveKit server | Connected: grid, mic, captions on data topic, `dub-{lang}` subscription | todo (needs a LiveKit server in CI) |

## 6. Accessibility (A11Y) - axe, no serious/critical violations

| ID | Pri | Page | Status |
|---|---|---|---|
| A11Y-01 | P0 | `/login` (dark + light) | auto |
| A11Y-02 | P0 | `/register`, `/verify-email` | auto |
| A11Y-03 | P0 | `/design` (dark + light) | auto |
| A11Y-04 | P0 | `/rooms` with data | auto |
| A11Y-05 | P1 | `/rooms/:id` host view (lobby + participants + routes) | auto |
| A11Y-06 | P1 | `/settings` tabs: workspace, members, runtime (form + JSON), account | auto |
| A11Y-07 | P1 | `/meetings/:id` not-configured state, `/join`, `/meetings` | auto |
| A11Y-08 | P1 | Create-room dialog open, command menu open | auto |

## 7. Keyboard (KBD)

| ID | Pri | Steps | Expected | Status |
|---|---|---|---|---|
| KBD-01 | P0 | Ctrl/Cmd+K, type "settings", Enter; reopen, Escape | Menu opens with focused search, navigates to `/settings`; Escape closes | auto |
| KBD-02 | P0 | Tab through login form | Each stop has a visible focus ring (outline width > 0) | auto |
| KBD-03 | P1 | Open create-room dialog by keyboard | Focus inside dialog, Tab stays trapped, Escape closes and returns focus to trigger | auto |
| KBD-04 | P1 | First Tab on a signed-in page | "Skip to content" link focused; Enter moves focus to main | auto |
| KBD-05 | P1 | Workspace switcher via keyboard | Enter opens, ArrowDown+Enter selects the other workspace | auto |
| KBD-06 | P1 | Command menu: "Create room" | Opens create-room dialog | auto |

## 8. Visual regression (VR) - `toHaveScreenshot`, dark + light, both projects

| ID | Pri | Target | Status |
|---|---|---|---|
| VR-01 | P0 | `/design` full page, dark | auto |
| VR-02 | P0 | `/design` full page, light | auto |
| VR-03 | P1 | `/login` dark + light | auto |
| VR-04 | P1 | `/rooms` populated (dynamic text masked) dark + light | auto |
| VR-05 | P1 | `/rooms/:id` host view dark | auto |
| VR-06 | P1 | `/settings?tab=runtime` dark | auto |

## 9. macOS WebView shell (WV)

| ID | Pri | Steps | Expected | Status |
|---|---|---|---|---|
| WV-01 | P1 | 1280x800 signed-in `/rooms` | Sidebar + top bar visible, no horizontal scrolling, main region scrolls independently | auto |
| WV-02 | P1 | Reduced motion media | No running CSS animations on the page (`getAnimations()` empty after settle) | auto |
| WV-03 | P2 | Narrow (390px) | Sidebar becomes a drawer opened by "Open navigation" | auto |

## Counts

| Area | Cases | P0 | P1 | P2 | Automated |
|---|---|---|---|---|---|
| Auth | 11 | 6 | 3 | 2 | 9 |
| Workspaces | 6 | 2 | 3 | 1 | 5 |
| Rooms | 10 | 4 | 5 | 1 | 9 |
| Runtime settings | 6 | 3 | 2 | 1 | 5 |
| Meeting shell | 5 | 1 | 3 | 1 | 4 |
| Accessibility | 8 | 4 | 4 | 0 | 8 |
| Keyboard | 6 | 2 | 4 | 0 | 6 |
| Visual regression | 6 | 2 | 4 | 0 | 6 |
| macOS WebView | 3 | 0 | 2 | 1 | 3 |
| **Total** | **61** | **24** | **30** | **7** | **55** |

All P0 and P1 cases are automated; the six `todo` cases are P2. Spec files: `e2e/tests/{auth,workspace,rooms,settings,meeting,a11y,keyboard,visual,webview}.spec.ts` (64 Playwright tests in the chromium project because a11y/visual cases run in dark and light; the macos-webview project re-runs `a11y`, `visual` and `webview`).

## Known backend gaps surfaced by this suite

1. `POST /translation-rooms/join` (by code) returns a participant without `roomId`, so an outsider cannot navigate to the room after joining; the UI reads an optional `roomId` and the E2E uses the shared room link.
2. `POST /translation-rooms/{id}/join` and `GET /translation-rooms/{id}` answer 404 for people outside the workspace who have not joined; join-by-id only works for workspace members.
3. No endpoint to invite/add workspace members (E2E seeds MEMBER rows with `psql`).
4. The email verification token is never returned or sent in dev (E2E marks users verified with `psql`).
