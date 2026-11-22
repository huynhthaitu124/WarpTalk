# WarpTalk web: E2E test plan

Real stack: Next.js dev server (`:3199`, `NEXT_PUBLIC_E2E_HARNESS=1`, `/api` rewritten to the backend) -> real `warptalk-api` (`:18080`, Postgres + Redis, LiveKit **not** configured).
Run: `cd apps/web && npx playwright test` (starts `scripts/e2e-backend.sh`, `next dev` and, for the `export` project, `next build` + `scripts/serve-out.mjs` on `:3198` itself).
Update screenshots: `npx playwright test --update-snapshots`.

Conventions
- Every test creates fresh random users/workspaces (`e2e-<label>-<hex>@example.com`), so the suite is re-runnable against a dirty DB.
- Email verification and invitations: the test backend runs with `DEV_MAIL_CAPTURE=1`, which stores the raw token in Redis (`dev:mail:verify:{email}`, `dev:mail:workspace-invite:{email}`). Fixtures read it with `redis-cli GET` and call `POST /auth/verify-email` / `POST /invitations/accept` (`fixtures/api.ts`: `verifyEmail`, `inviteAndAccept`). No psql for verification or membership (`addWorkspaceMember` is deleted). psql remains only for state with no public API: `expireBridgeLease`, `seedMeetingRow` (the meeting row a LiveKit join creates), `seedChatTranslation` (what the AI translation worker stores) and `makeSystemAdmin`.
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
| MTG-01 | P0 | started room | `/meetings/:id`: pre-join, then Join | No control bar/Unmute/Leave before a session exists (FN-06); "LiveKit is not configured" state keeps room info: People roster with host, Transcript placeholder, Chat disabled | auto |
| MTG-02 | P1 | pre-join | Open "I speak" / "I listen in"; pick English / Japanese; Join | Options are the room languages (vi, en, ja); `POST /translation-rooms/:id/join` carries `speakLanguage`/`listenLanguage` | auto |
| MTG-03 | P1 | started + waiting room | `/meetings` | Only live/open rooms listed; Open link goes to meeting | auto |
| MTG-04 | P1 | meeting not configured | "Try again" | Back to pre-join | auto |
| MTG-05 | P2 | LiveKit server | Connected: grid, mic, captions on data topic, `dub-{lang}` subscription, connection-quality badge | todo (needs a LiveKit server in CI) |
| MTG-06 | P0 | started room, host | Header "End meeting" -> dialog -> Cancel (room unchanged) -> End meeting | `POST /meetings/{id}/end` answers 404/503 without LiveKit, the app falls back to ending the room; toast, lands on `/rooms/:id`, pill "Ended"; `/meetings/:id` then shows "This meeting has ended" with no Join/End | auto |
| MTG-07 | P1 | started room, workspace MEMBER | Open `/meetings/:id` | No "End meeting" button, Join available | auto |
| MTG-08 | P1 | harness (`e2e/harness/meeting.tsx`), unstarted room, then start/pause via API | In-meeting header status strip (NFR-US-02) | Connection "Connected", Network "Good"/"Poor", Translation "Not started" -> "Active/Stopped" -> "Paused" (room polled) | auto |
| MTG-09 | P0 | harness (`e2e/harness/captions.tsx`, real `CaptionsPanel` + `parseCaption`/`mergeCaption` fed pipeline payloads) | stt then translation message of one segment; translation only in another language; interim + 10 min old `emittedAtMs`; plain text | Original + translation per line, latency "450 ms" from `emittedAtMs`, same `segment_id` merges, "Translating to English…" only for final lines, skewed latency hidden | auto |
| MTG-10 | P1 | harness | Original-audio listener, reconnecting + poor + paused | First available translation shown; labels "Reconnecting", "Poor", "Paused" | auto |
| MTG-11 | P0 | harness, host | Mic, camera, share screen, captions toggles | Buttons flip name + `aria-pressed`; local tile shows mic state and video; share switches grid to spotlight and back; caption overlay hides/shows | auto |
| MTG-12 | P0 | harness, 3 participants | `speak()` a remote; join/leave a participant | Tile `data-speaking` + "speaking" text, others unaffected; grid count follows; host crown | auto |
| MTG-13 | P1 | harness | Raise hand, remote hand, lower, garbage payload | Publishes `{t:"hand"}` on topic `reactions`; hand badge on the right tiles; invalid payload ignored | auto |
| MTG-14 | P1 | harness | Send a reaction; remote reaction | Publishes `{t:"reaction",emoji}`; floating chip with sender name; expires after ~4 s | auto |
| MTG-15 | P1 | harness | Chat while on Transcript tab; send; Enter; duplicate id | Unread badge on tab + panel button, cleared on open; local echo `data-mine`; whitespace not sendable; wire duplicates dropped; "not saved" note (no backend chat persistence) | auto |
| MTG-16 | P0 | harness | stt + translation of one segment, second speaker | One line per segment with original + translation + latency badge + speaker name/accent (different per speaker); "Translating to English…"; overlay shows latest | auto |
| MTG-17 | P0 | harness + real API, approval room, two lobby guests | People tab | Lobby badge on tab and panel button; "Waiting to be admitted (2)"; roster shows host role/languages; Admit moves to "In the meeting", Deny removes (participant KICKED) | auto |
| MTG-18 | P0 | harness + real API, unstarted room | Host controls: Start, Pause, Resume, Stop translation | Room status/`translationActive` change on the backend; status strip follows | auto |
| MTG-19 | P0 | harness + real API | Host: Leave dialog (host note, Cancel), End for all dialog (Cancel keeps room, confirm ends); member: no host menu, plain Leave | Room ENDED after confirm; `onLeave`/`onEnded` callbacks fire | auto |
| MTG-20 | P1 | harness | `setConnection(reconnecting)`; listen-language menu | Warning banner + strip "Reconnecting", gone on reconnect; menu: Original + Dubbed vi/en/ja, current marked | auto |
| MTG-21 | P2 | harness | Close/reopen side panel | Tabs Transcript, People, Chat | auto |
| MTG-22 | P0 | harness | axe over the meeting in dark + light, all three tabs | No serious/critical violations | auto |
| MTG-23 | P2 | harness | Ended view | "This meeting has ended" + way back | auto |
| MTG-24 | P1 | harness at 390x844 | Layout | Two-column compact tiles (<140px), stage never overlaps the control bar (LY-04), no horizontal scroll, panel is a bottom sheet with Close | auto |
| MTG-25 | P1 | harness, 1280x800 and 390x844, dark + light | `toHaveScreenshot` of the live meeting (speaking, hand, captions) | Matches `meeting-live-{1280,390}-{theme}.png` | auto |
| MTG-30 | P0 | project `meeting-media` (Chromium fake devices) | Open pre-join | Preview video plays, mic meter rises above 0, camera and microphone pickers list the fake devices | auto |
| MTG-31 | P1 | meeting-media | Camera toggle; "Join with my microphone muted" | Preview goes off/on (track stopped/reopened); muted by default, can be unchecked | auto |
| MTG-32 | P2 | meeting-media | Pick the second microphone | Preview re-opens on it (skipped when only one fake mic) | auto |
| MTG-33 | P1 | meeting-media | Fill name, Join | Join request has name + languages; not-configured state; preview element and tracks released | auto |
| MTG-34 | P1 | meeting-media, 1440/1280/390 x dark/light | Pre-join layout + screenshots (video/meter masked) | One centred card (two columns >= 1024), no overflow, Join above the device pickers on phones | auto |

MTG-11..25 run the real in-meeting UI against a scripted media session (`e2e/harness/meeting.tsx`, mounted by `e2e/harness/helpers.ts` on a signed-in page; `window.__meeting` scripts speaking, data-channel messages, joins and connection state). Roster, admit/deny, translation start/stop/pause and end-meeting hit the real backend. MTG-30..34 live in the `meeting-media` Playwright project (Chromium `--use-fake-device-for-media-stream --use-fake-ui-for-media-stream`); the other projects skip that file. Real LiveKit track rendering stays under MTG-05 (todo).

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
| A11Y-09 | P0 | `/external` in a plain browser (fallback card, valid and invalid link), dark + light | auto |
| A11Y-10 | P0 | `/external` with the native mock: claimed room, capture error row, permissions panel, dark + light | auto |
| A11Y-11 | P1 | `/meetings/:id` status strip + "End meeting" dialog open | auto |

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
| VR-07 | P1 | `/external` with the native mock, before claiming (dark + light) | auto |
| VR-08 | P1 | `/external` in a plain browser, fallback card (dark + light) | auto |

## 9. macOS WebView shell (WV)

| ID | Pri | Steps | Expected | Status |
|---|---|---|---|---|
| WV-01 | P1 | 1280x800 signed-in `/rooms` | Sidebar + top bar visible, no horizontal scrolling, main region scrolls independently | auto |
| WV-02 | P1 | Reduced motion media | No running CSS animations on the page (`getAnimations()` empty after settle) | auto |
| WV-03 | P2 | Narrow (390px) | Sidebar becomes a drawer opened by "Open navigation" | auto |

## 10. External meeting (EXT) - `e2e/tests/external.spec.ts` (chromium project)

The macOS host is replaced by a scriptable `window.warptalkNative` mock injected with `page.addInitScript` (`fixtures/native-mock.ts`, contract `apps/macos/BRIDGE.md`): scripted capabilities, audio apps, permissions, a `startExternalCapture` rejection, `grantOnRequest`; every call is recorded (`nativeCalls`) and host events are fired with `emitNative` (`warptalk-native` CustomEvents). The backend is the real API; the capturer lease is aged with `expireBridgeLease` (psql, test-only) because waiting out the 45 s lease would slow the suite. LiveKit is not configured, so caption delivery itself is covered by the MTG-09/10 harness.

| ID | Pri | Preconditions | Steps | Expected | Status |
|---|---|---|---|---|---|
| EXT-01 | P0 | plain browser (no mock) | Open `/external`; claim a Meet link | Fallback card ("Capture needs the WarpTalk desktop app", get-the-app steps, viewing-only note), no source-app field; claim answers 201, role Capturer plus "This browser cannot capture", no Start capture | auto |
| EXT-02 | P0 | signed in | Type Meet URL/code, Zoom URL/number, Teams URL, foreign host, bad Meet code, short Zoom id, garbage | Provider badge + canonical code for valid input (query stripped, Meet lower-cased, Zoom digits only); alert text and disabled Claim for invalid | auto |
| EXT-03 | P1 | signed in | Choose the same far-side and listen language | Inline error, Claim disabled | auto |
| EXT-04 | P1 | signed in | Claim a Zoom link | Request carries `provider: zoom`, `meetingUrl`, source `en`, targets `[vi]` | auto |
| EXT-05 | P0 | native mock | Claim a new meeting | 201, "New room created", role Capturer, pill Waiting, lease heartbeat shown; API lists an `EXTERNAL_BRIDGE` room titled `External meeting <code>` | auto |
| EXT-06 | P0 | meeting already claimed via API by the same user | Claim again in the UI | 200, "Joined the existing room", still Capturer; "Open meeting page" links to the room; "Use a different meeting" resets the form | auto |
| EXT-07 | P0 | two users (second is a workspace member), real backend | B claims A's meeting; Take over (fresh lease); age A's lease; Take over again | B is Viewer (200); first takeover 409 `CAPTURER_ACTIVE` shown; then "Stale" badge + "capturer looks inactive"; takeover succeeds, B is Capturer, capture card appears; API shows B as capturer | auto |
| EXT-08 | P0 | claimed, Chrome selected | Start capture | Room started (Live), `PUT audio-mode` text, native `startExternalCapture` gets `appBundleId`, `roomId`, `mode: text`, `heartbeatIntervalSeconds: 15`, a JWT the API accepts; state Running; source/mode controls locked | auto |
| EXT-09 | P1 | capture running | Emit `level`, `audioFlowing`, `heartbeat` (ok and failed), `log`, `status` | Meter value/text (-20 dBFS, Silent), "Audio flowing"/"No audio buffers", last sound, heartbeat OK/Failed + message, native log, backend label | auto |
| EXT-10 | P0 | `startExternalCapture` rejects `PERMISSION_DENIED`; permissions denied | Start capture; Request permission; Open System Settings; Dismiss | Error row with code; `requestPermission('screenRecording')`; `openSystemSettings('audioCapture')`; state stays Idle | auto |
| EXT-11 | P1 | rejects `NO_AUDIO_PROCESS` | Start; clear the error in the host; "Try again" | Row appears, retry starts capture (2 native calls), row gone | auto |
| EXT-12 | P1 | running | Fatal `error` event `LEASE_LOST`, then `status: idle` | "Capture stopped" row with Refresh room; back to Idle with Start capture | auto |
| EXT-13 | P0 | running | Wait 1.1 s; emit `auth-required` | One `POST /auth/refresh`; `updateAuth` called once with a different, valid JWT; toast; no error row | auto |
| EXT-14 | P1 | running, refresh answers 500 | Emit `auth-required`; remove the fault; "Refresh session" | Row `AUTH_REFRESH_FAILED`, no `updateAuth`; recovery button then calls `updateAuth` | auto |
| EXT-15 | P0 | running | Stop capture | `stopExternalCapture` once, Idle, meter Silent, controls unlocked | auto |
| EXT-16 | P0 | BlackHole device reported | Start in text, stop, switch to Voice, start | 409 `BRIDGE_AUDIO_MODE_LOCKED` alert ("Voice mode is locked"), mode reset to Text only, native not called again | auto |
| EXT-17 | P1 | no output device | Open Mode select | Voice option disabled, explanation visible | auto |
| EXT-18 | P1 | BlackHole + speakers reported | Choose Voice, start | Dub output defaults to the virtual device; native gets `mode: voice`, `listenLanguage: vi`, `dubOutputDeviceUid`; server audio mode Voice | auto |
| EXT-19 | P1 | Chrome playing, zoom.us silent | Open Source app; pick zoom; host reports it producing; Refresh apps | Options show "playing audio"/"silent"; indicator badge follows | auto |
| EXT-20 | P1 | claimed | Start with no app chosen | Start disabled, hint shown, native not called | auto |
| EXT-21 | P1 | mixed permission states | Request microphone; Open settings; change state in host; Refresh | States (Granted/Denied/Not requested); audioCapture has no Request; calls recorded; Granted after request/refresh | auto |
| EXT-22 | P1 | native mock | Toggle overlay, click-through, hide from screen share, off | `showOverlay(true)`, `setOverlayOptions` payloads, `clearOverlay` + `showOverlay(false)` | auto |
| EXT-23 | P1 | LiveKit unset | Claim, start capture | Captions placeholder explains missing LiveKit; translation badge; Retry captions | auto |
| EXT-24 | P1 | browser at first | Host announces itself later (`warptalk-native-ready`) | Fallback disappears, "Desktop app" badge with OS version | auto |
| EXT-25 | P1 | running, refresh answers 401 | Emit `auth-required` | Session ends (redirect to `/login`), `updateAuth` never called | auto |
| EXT-26 | P2 | LiveKit server | Captions arrive while overlay is on | `pushOverlayCaption` called per segment (interim then final, same `segmentId`) with the listener's translation | todo (needs a LiveKit server in CI) |
| EXT-27 | P2 | host already capturing | Reload `/external` | Page resumes the running capture's room from `captureStatus()` | todo (mock cannot pre-seed a running status yet) |

## Audit follow-up: new pages and fixed findings (spec files `pages`, `rooms`, `workspace`, `settings`, `keyboard`, `webview`, `auth`, `a11y`, `visual`)

| ID | Finding | Pri | Expected | Spec |
|---|---|---|---|---|
| ROOM-01 | IA-04 | P1 | Breadcrumb `Meetings / <title>` with `aria-current` on the title | rooms |
| ROOM-02/10/11 | FN-04, IA-02, LY-08 | P0 | Meetings has Active/History/All tabs, search and status filter in the URL; filtered-empty says "No cancelled meetings" with Clear filter; `/rooms` and `/meetings` redirect | rooms |
| ROOM-03/04 | LY-07, DS-06 | P1 | One primary per state; Stop translation/Cancel/End in "More" with confirm dialogs | rooms |
| ROOM-05/07 | FN-08 | P1 | Waiting guest sees no room code or roster; a person with no workspace gets a minimal shell | rooms |
| ROOM-06/14 | FN-07 | P1 | Deny toast says "declined"; Admit all | rooms |
| ROOM-09 | FN-10 | P2 | Validation messages and required markers in the create dialog | rooms |
| ROOM-12/13 | FN-09 | P1 | "copied" toast only after the clipboard promise resolves; failure toast otherwise | rooms |
| ROOM-15 | FN-12 | P2 | Missing room: no `/participants` request | rooms |
| WS-01/06/07 | FN-02, IA-03 | P0 | Default workspace = the one with meetings, persisted; URL slug wins and becomes the stored choice | workspace |
| WS-08 | IA-01 | P0 | Home: quick actions, live now, recent history | workspace |
| MEM-01/02/03 | IA-01 | P0 | Invite, resend, revoke; invitee accepts from pending banner; plain member cannot invite (UI+API 403) | pages |
| KNOW-01/02/03 | IA-01 | P1 | Knowledge search: empty state, real-API outcome, mocked results with page and meeting+timestamp citations, no-results | pages |
| AI-01..05 | IA-01, IA-05 | P0 | Provider keys write-only (last4), BYO-off explained, RBAC (member: no section, API 403), per-task config validation and payload | settings |
| SET-06/07 | IA-05 | P1 | Settings sections, `?tab=` redirects, no slug as description | settings |
| DS-01/02/03/07/09 | DS-* | P1 | Real logo/favicon, Inter loaded, 30px/13px grouped sidebar, no unadvertised features, bare table in card, truncate+tooltip | pages |
| A11Y-01..03 | A11Y-01..03 | P1 | Computed contrast of control border, primary hover/pressed, tertiary ink in both themes | pages |
| A11Y-06 | A11Y-06 | P2 | Focused invalid input draws a danger ring | pages |
| LY-A..F | LY-01/02/04/05 | P0 | `scrollWidth <= innerWidth` at 390px on every page (signed-in and public, with dialogs, drawer, lobby with long names, 150-char title); rows stack; members list; settings nav scrolls | pages |
| KBD-07/08/09 | FN-01 | P1 | G-then-M/S/H, C, J work; ignored while typing; menu hints are real | keyboard |
| WV-03/04/05 | A11Y-04 | P1 | Mobile drawer is a Radix dialog: focus trap, Escape, focus return, close button, overlay | webview |
| AUTH-10/11/12 | FN-03, IA-06 | P1 | Field-level validation with focus on the first invalid field; show/hide password; INVALID_REQUEST JSON | auth |
| A11Y-12/13 | | P1 | axe on home, knowledge, join dialog, auth errors, settings AI/members | a11y |
| VR-04/09 | | P1 | Visual: meetings and home (dark+light), re-baselined login/design/room/settings/external | visual |

Not automated: production-build check that the "Design system" link is absent (DS-11; tests run against the dev server where it is shown), real knowledge hits (the e2e backend has no embedding keys), saving a real AI key (no `AI_KEYS_KEK`; covered with mocked routes).

## 11. Meeting chat (CHAT) - `e2e/tests/chat.spec.ts`

REST is the source of truth (`POST/GET /meetings/{id}/chat/messages`, `POST .../{id}/hide`); the LiveKit `chat` topic only speeds delivery up. The e2e backend has no LiveKit, so a meeting row is seeded and the panel runs in the meeting harness against the real API.

| ID | Pri | Steps | Expected | Status |
|---|---|---|---|---|
| CHAT-01 | P0 | API: post twice with one `clientMessageId`; empty, 1001 chars, bad id; outsider; no meeting row; ended room | 201 then 200 with the same id, one stored message; 400s; 403; 404 + empty history; 409 | auto |
| CHAT-02 | P0 | API: 7 messages, `limit` + `before` paging | Oldest-first pages 5-7, 2-4, 1, then empty | auto |
| CHAT-03 | P0 | API: member and host hide | Member 403; host 200 idempotent; host history flags `hidden`, others do not see it | auto |
| CHAT-04 | P0 | UI: 52 stored messages, open Chat | Last 50 shown, "Load older messages" adds the rest, button disappears | auto |
| CHAT-05 | P0 | UI: slow POST | Bubble at once as "Sending…" (`data-status=sending`), then `sent`, one stored | auto |
| CHAT-06 | P0 | UI: response lost after the server stored it | Automatic retry with the same `clientMessageId`; stored exactly once | auto |
| CHAT-07 | P1 | UI: server 503 | "Not sent: ..." with Retry/Discard; Retry delivers once; Discard drops it | auto |
| CHAT-08 | P0 | UI: translation rows appear for a message (no push) | Shown under the message in the listen language within the 5 s poll; follows a listen switch | auto |
| CHAT-09 | P1 | UI: `chat.translated` on the data channel | Immediate refetch, translation shown | auto |
| CHAT-10 | P1 | UI: REST-only message; data-channel copy of a stored message; copy with a spoofed sender | Unread badge via poll; no duplicate bubble; spoofed copy ignored | auto |
| CHAT-11 | P1 | UI: host hides a member message; member view | Host sees "Hidden from participants"; member sees neither text nor Hide control | auto |
| CHAT-12 | P1 | axe on the chat tab (history, translation, hidden), dark + light | No serious/critical | auto |
| CHAT-13 | P2 | Real LiveKit data delivery and a realtime hub pushing `chat.translated` | | todo (needs LiveKit server and hub) |

MTG-15 (section 5) covers the harness path: REST post, data-channel mirror, dedupe, unread badge, no "not saved" notice.

## 12. Mid-meeting language switch (LANG) - `e2e/tests/language.spec.ts`

| ID | Pri | Steps | Expected | Status |
|---|---|---|---|---|
| LANG-01 | P0 | API: `PUT .../participants/me/languages` listen, speak `none`, listen `none`, empty, bad code | 200 with updated participant; 4xx for the rest | auto |
| LANG-02 | P0 | Listen menu: Dubbed Vietnamese, then Original audio | PUT first, then the subscription switches (`onListen`); "Original" sends nothing | auto |
| LANG-03 | P1 | PUT answers 500 | Toast "Could not change the listen language"; subscription unchanged | auto |
| LANG-04 | P0 | Meeting settings menu: I speak English, then Listen only | Server `speakLanguage` `en`, then `none`; current marked | auto |
| LANG-05 | P2 | Real LiveKit: `dub-{lang}` track subscription follows the switch | | todo (needs LiveKit) |

## 13. Glossary (GLOS) - `e2e/tests/glossary.spec.ts`

| ID | Pri | Steps | Expected | Status |
|---|---|---|---|---|
| GLOS-01 | P0 | Glossary nav item, New glossary: empty name, same languages, valid; duplicate name | Inline validation; lands on detail; API list matches; 409 shown in the dialog | auto |
| GLOS-02 | P0 | Inline add (validation, duplicate), inline edit, delete with confirm | Terms table and API agree; Cancel keeps the term | auto |
| GLOS-03 | P0 | CSV import (header, quoted comma, duplicate of existing), malformed file, Export CSV | "Imported 2 terms, skipped 1."; failure alert; download contains the terms | auto |
| GLOS-04 | P0 | Workspace member | Read-only note, no create/import/edit/delete/add form, export available; API writes 403, reads 200 | auto |
| GLOS-05 | P1 | Edit and delete a glossary; outsider read | Heading renames; confirm names the term count; list empties; outsider refused; 404 after delete | auto |
| GLOS-06 | P0 | Plain owner vs system admin on Settings > Global glossary: draft, publish (definition required), edit, audit drawer, archive, delete | No entry/403 for owners; published term is in `/glossaries/global` and read-only on a member's Glossary page; audit entries listed; drawer passes axe | auto |
| GLOS-07 | P1 | Glossary list and detail, dark + light, 1280 and 390 | No overflow, axe clean, `glossary-{list,detail}-{1280,390}-{theme}.png` | auto |

## 14. Members management (MEM) - `e2e/tests/members.spec.ts`

MEM-01..03 (invite, resend, revoke) are in `pages.spec.ts`.

| ID | Pri | Steps | Expected | Status |
|---|---|---|---|---|
| MEM-04 | P0 | Owner: Actions -> Make admin, Cancel then confirm | No change on Cancel; role ADMIN in UI and API; menu now offers Make member | auto |
| MEM-05 | P0 | Owner removes a member | Dialog explains the loss of access; row gone; API role undefined; member refused | auto |
| MEM-06 | P0 | Admin, member and owner views; direct API calls | Admin only gets Remove on members; member and owner row/self row have no controls; server answers 403 | auto |
| MEM-07 | P0 | Owner transfers ownership to an admin | New owner OWNER, previous owner ADMIN and without controls | auto |
| MEM-08 | P1 | Remove answers 409 | Message shown inline in the dialog; row stays | auto |

## 15. Documents (DOC) - `e2e/tests/documents.spec.ts` (chromium project)

Real backend with a throwaway local object store (`STORAGE_LOCAL_DIR` is a `mktemp -d` set by `scripts/e2e-backend.sh`). Upload, list, patch, archive and download are real; ingestion happens asynchronously on the server, so assertions accept `pending|completed|skipped` and only the AI-off case (`skipped`, `ai_retrieval_not_allowed`) is deterministic. DOC-12/13 and the visual cases mock only the list `GET`.

| ID | Pri | Steps | Expected | Status |
|---|---|---|---|---|
| DOC-01 | P0 | Owner: Upload dialog, picker, title + category Legal | Row with file name, category and an ingestion pill; API holds title, `LEGAL`, `MEMBERS_ONLY`, AI allowed | auto |
| DOC-02 | P1 | Drag and drop a CSV, access Admins only, AI switch off | Selected file shown; row appears; API `ADMIN_ONLY`, AI off, status `skipped` / `ai_retrieval_not_allowed` | auto |
| DOC-03 | P0 | Pick `.exe`, an empty file and a 20 MB + 1 file | Client alerts (type list, "The file is empty.", "exceeds the 20 MB limit."); no POST is sent; submit without a file asks for one | auto |
| DOC-04 | P0 | API: `.exe`, 21 MB txt, fake `.pdf`; UI: fake `.pdf` | 415 `UNSUPPORTED_FILE_TYPE`, 413 `FILE_TOO_LARGE`, 415 `FILE_CONTENT_MISMATCH`; nothing stored; UI shows the server message and keeps the dialog open | auto |
| DOC-05 | P0 | Member context | List readable, no Upload button, read-only note, drawer has Download but no form/Archive; API upload, patch, archive answer 403 | auto |
| DOC-06 | P0 | Owner uploads ADMIN_ONLY + PUBLIC docs; member and owner views | Member never sees the admin-only title (list, search); direct GET and download answer 404; owner sees both | auto |
| DOC-07 | P0 | Drawer: title, category, access, AI toggle, Save | Toast, drawer heading updated, ingestion note says AI retrieval is off; API patched; row shows new category and `skipped` | auto |
| DOC-08 | P0 | Archive: Cancel, then confirm | Dialog explains consequence; Cancel keeps it (API 200); confirm removes the row (API 404) | auto |
| DOC-09 | P0 | Download from the drawer | Browser download with the original file name; bytes equal the upload | auto |
| DOC-10 | P1 | Search by title, category filter, no match, Clear filters | List narrows and restores | auto |
| DOC-11 | P1 | Open the page; upload a PDF | PDF/DOCX extraction notice always visible ("no indexable content"); PDF accepted | auto |
| DOC-12 | P1 | Mocked list with all four statuses | Pills Completed/Skipped/Pending/Failed; focus shows a reason tooltip; drawer explains PDF extraction is unavailable | auto |
| DOC-13 | P1 | Mocked list: pending twice, then completed | List polls, pill flips to Completed without reload, polling stops | auto |
| DOC-14 | P0 | axe on list, drawer and upload dialog, dark + light, 1280 + 390 | No serious/critical violations, no horizontal scroll | auto (4 tests) |
| DOC-15/16 | P1 | Screenshots of the list and of the drawer, dark + light, 1280 + 390 | `documents-{theme}-{vp}.png`, `documents-drawer-{theme}-{vp}.png` | auto (8 tests) |
| DOC-17 | P2 | More than 20 documents: pagination | Previous/Next | todo (needs 21 uploads; covered by manual check) |

## 16. Voice cloning (VOICE) - `e2e/tests/voice.spec.ts`, `voice-media.spec.ts`

Fixtures: `makeWav` in `fixtures/files.ts` generates silent and speech-like mono PCM16 WAVs. The test backend has no voice provider unless one is configured in the environment, so a good sample is allowed to end in either a quality score or `503 VOICE_CLONING_UNAVAILABLE`.

| ID | Pri | Steps | Expected | Status |
|---|---|---|---|---|
| VOICE-01 | P0 | Settings > Account: consent card, checkbox, "I consent" | Button disabled until checked; POST body has `consentVersion` `2026-11` and `acknowledgementHash` = SHA-256 of the displayed text; status shows date and version; persists on reload | auto |
| VOICE-02 | P0 | Revoke consent: Cancel, then confirm | Dialog explains deletion; Cancel keeps consent; confirm returns to the consent card; API `granted: false` | auto |
| VOICE-03 | P0 | Upload a silent 12 s WAV, submit | Specific message "too quiet ..." (`VOICE_SAMPLE_TOO_QUIET`); no profile stored | auto |
| VOICE-04 | P0 | Upload a speech-like 12 s WAV, submit | Either "Quality score N / 100" and the status card updates, or `VOICE_CLONING_UNAVAILABLE` explained as "no voice provider is configured" | auto |
| VOICE-05 | P1 | 2 s WAV; an MP3 renamed; a 7 MB file | `VOICE_SAMPLE_TOO_SHORT` text; client "not a WAV recording"; client "under 6 MB" | auto |
| VOICE-06 | P0 | API enroll without consent | 4xx `VOICE_CONSENT_REQUIRED` | auto |
| VOICE-07 | P1 | Two users | Consent is per user | auto |
| VOICE-08 | P0 | axe on consent card, enrollment card and error state, dark + light, 1280 + 390 | No serious/critical violations, no horizontal scroll | auto (4 tests) |
| VOICE-09 | P1 | Screenshots of the voice section (consent, enrollment), dark + light, 1280 + 390 | `voice-{consent,enroll}-{theme}-{vp}.png` | auto (4 tests) |
| VOICE-10 | P0 | `meeting-media` project (fake mic): Record sample | Live meter moves, timer counts, Stop locked until 10 s, auto-stop at 20 s; sample is mono PCM16 WAV about 10 s (24 kHz) in the enroll request; result or friendly error shown | auto |
| VOICE-11 | P1 | Record, then Cancel | Microphone released, nothing kept, Record available again | auto |
| VOICE-12 | P2 | Provider configured: success path end to end, `cloneEnabled` true | Needs a provider key in the test environment | todo |

## 17. Static export (EXP) - `e2e/tests/export.spec.ts` (`export` project)

The `export` project builds the app (`next build`, `output: 'export'` -> `out/`) and serves it with `scripts/serve-out.mjs` on `:3198` (same fallback rules as the macOS bundle server; `/api` proxied to the e2e backend). Harness modules are NOT available there (`NEXT_PUBLIC_E2E_HARNESS` is only set for `next dev`).

| ID | Pri | Preconditions | Steps | Expected | Status |
|---|---|---|---|---|---|
| EXP-01 | P0 | signed in, room | Open `/rooms/<id>` directly, then reload | 200, SPA boots, room heading shown | auto |
| EXP-02 | P1 | signed out | Open `/meetings/<id>`; request a missing `/_next/static/*.js` | Redirect to `/login`; asset is a real 404 (no HTML fallback for paths with an extension) | auto |

Test harness note: the Vite-era specs imported `/e2e/harness/*.tsx` by URL. Under Next they use `window.__wtHarness.<name>()` (`src/HarnessBridge.tsx`, only when `NEXT_PUBLIC_E2E_HARNESS=1`).

## Fixes in this round

| ID | Case | Spec |
|---|---|---|
| WS-09 | Home "Live now" lists only `IN_PROGRESS`; WAITING/OPEN/PAUSED are under "Waiting to start" | workspace |
| - | `meeting`, `meeting-room`, `external` specs use `inviteAndAccept`; `addWorkspaceMember` deleted; `lucide-react` removed from the project | all |

## Contract fixes (CONTRACT_AUDIT B-02, B-03, B-04, B-09, suggestions, AI data controls)

Spec: `e2e/tests/contracts.spec.ts` (+ `EXT-25 (B-09)` in `external.spec.ts`). Harnesses: `e2e/harness/audio.ts` (real subscription rules against fake LiveKit participants), `captions.tsx`, `meeting.tsx`; the SSE stream and follow-ups are mocked with `page.route`.

| ID | P | Case | Expected | Status |
|---|---|---|---|---|
| CAP-01 | P0 | `speakerName` present | Label is the name, never `pipeline-agent-*` or a UUID | auto |
| CAP-02 | P0 | 3 sentences, cumulative `translations` | One line, full original and full translation | auto |
| CAP-03 | P1 | Legacy `chunk_idx` shape, out-of-order | Appended in sentence order | auto |
| CAP-04 | P0 | Old snake_case shape, no indexes | Overwrite behaviour unchanged | auto |
| CAP-05 | P0 | No name; `speakerId` = participant id / `userId:tag`, sender = pipeline agent | Roster display name in transcript and overlay | auto (real backend roster) |
| CAP-06 | P1 | Current pipeline shape (cumulative `text`, `sentenceText`, `speakerUserId`) | Replaced per segment, no duplication | auto |
| AUD-01..05 | P0 | vi/vi/en room: vi listener hears vi originals + `dub-vi`; en listener `dub-en` + en original; third language dubs only; `original`; tag stripping, secondary identities and unknown speakers muted | Subscription set as listed | auto (harness) |
| SUG-01 | P1 | Mocked SSE: 2 suggestions, follow-ups, copy | Tab, count badge, dedupe on reconnect, clipboard | auto |
| SUG-02 | P1 | Dismiss / snooze | Hidden, stays hidden after reconnect re-delivery | auto |
| SUG-03 | P0 | Stream answers 404 | No Suggestions tab at all | auto |
| SUG-04 | P1 | SSE parser (split chunks, comments, CRLF, multi-line data) | Correct messages | auto |
| EXT-25 | P0 | `/external` captions join | Body has `clientTag: "listener"` | auto |
| SET-AI-01 | P1 | Owner toggles aiRetrievalAllowed / externalLlmAllowed / aiSuggestEnabled | Full-settings PUT per change | auto (settings mocked) |
| SET-AI-02 | P0 | Member | AI settings "Not available"; API PUT 403 | auto |
| ROUTE-01 | P2 | audio-route status CAPTION_ONLY / STANDARD_VOICE | Degraded badge in Room detail | todo |

## Counts

| Area | Cases | P0 | P1 | P2 | Automated |
|---|---|---|---|---|---|
| Auth | 11 | 6 | 3 | 2 | 9 |
| Workspaces | 6 | 2 | 3 | 1 | 5 |
| Rooms | 10 | 4 | 5 | 1 | 9 |
| Runtime settings | 6 | 3 | 2 | 1 | 5 |
| Meeting shell | 33 | 11 | 18 | 4 | 32 |
| Accessibility | 11 | 6 | 5 | 0 | 11 |
| Keyboard | 6 | 2 | 4 | 0 | 6 |
| Visual regression | 8 | 2 | 6 | 0 | 8 |
| macOS WebView | 3 | 0 | 2 | 1 | 3 |
| External meeting | 27 | 10 | 15 | 2 | 25 |
| Documents | 17 | 9 | 7 | 1 | 16 |
| Voice cloning | 12 | 7 | 4 | 1 | 11 |
| **Total (areas listed here; chat, language, glossary and members have their own sections)** | **150** | **62** | **74** | **14** | **140** |

All P0 and P1 cases are automated; the nine `todo` cases are P2. Spec files: `e2e/tests/{auth,workspace,rooms,settings,meeting,meeting-room,meeting-media,external,a11y,keyboard,visual,webview}.spec.ts` (64 Playwright tests in the chromium project because a11y/visual cases run in dark and light; the macos-webview project re-runs `a11y`, `visual` and `webview`).

## Known backend gaps surfaced by this suite

1. `POST /translation-rooms/join` (by code) returns a participant without `roomId`, so an outsider cannot navigate to the room after joining; the UI reads an optional `roomId` and the E2E uses the shared room link.
2. `POST /translation-rooms/{id}/join` and `GET /translation-rooms/{id}` answer 404 for people outside the workspace who have not joined; join-by-id only works for workspace members.
3. No endpoint to invite/add workspace members (E2E seeds MEMBER rows with `psql`).
4. The email verification token is never returned or sent in dev (E2E marks users verified with `psql`).
5. `POST /meetings/{id}/end` answers 404 "meeting has not started" until someone joined through LiveKit (and 503 when LiveKit is unset), so a host cannot end a bridge room through it; the web app falls back to `POST /translation-rooms/{id}/end`.
6. A bridge-room viewer is not a room participant, so `POST /meetings/{id}/join` (captions) is refused for them (`not a participant of this meeting`); a viewer cannot read captions until the join gate admits workspace members of a bridge room.
7. Bridge claim/takeover/heartbeat responses omit the lease expiry; the UI derives staleness from `capturerLeaseAt + leaseSeconds` and the server clock may differ from the browser's.
8. No endpoint to end/release a bridge room's capturer lease on stop (`stopExternalCapture` only stops native; the lease ages out after 45 s).
