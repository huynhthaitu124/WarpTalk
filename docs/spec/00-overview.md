# 00 - Product Overview

## 1. Vision

WarpTalk is an AI real-time speech-translation platform for multilingual online meetings. It combines speech-to-text (STT), context-aware LLM translation, text-to-speech (TTS) and voice cloning in one meeting system, so participants hear translated speech in a voice that preserves the speaker's identity instead of reading subtitles.

Target customers: small and medium-sized organizations working with overseas vendors, remote staff and international customers. Reference competitors: Palabra AI (speech translation only, no full meeting suite) and KUDO AI (conference/interpreter focused, higher cost, mostly one-way AI translation). WarpTalk's differentiator is real-time speech translation + voice-preserving output + meeting management + workspace knowledge (glossary, documents, RAG assistant) at SME price points.

Project identity: code SU26SE112, "WarpTalk - An AI Speech Translation Platform for Real-Time Multilingual Communication with Voice Cloning". Client types: Web and Desktop (Electron).

## 2. Major features (FE)

| ID | Feature | Summary |
|---|---|---|
| FE-01 | Auth & Workspace Management | Register, login (incl. Google SSO), profiles/avatars, workspaces, invitations, workspace documents, RBAC (Owner/Admin/Member), workspace policies |
| FE-02 | Live Meeting Management | Instant/scheduled/recurring meetings, meeting types, join by room code or invitation with lobby admission, LiveKit audio/video, screen share, in-meeting chat, recording, host controls |
| FE-03 | Real-time Translation & Voice Cloning | Live STT + translation with workspace/global glossaries, TTS and voice cloning (voice profiles, consent), per-language audio routing |
| FE-04 | Transcript & AI Meeting Intelligence | Speaker-attributed live transcripts, persisted artifacts, Ask WarpBot (LLM + RAG), AI summaries and meeting minutes (multilingual templates, export/share) |
| FE-05 | External Platform Integration | Google Meet via External Bridge rooms (scheduled via WarpBot or detected live by Desktop app); virtual audio drivers for third-party meetings |
| FE-06 | Billing & Subscription | B2B plans, Stripe payments, per-usage metering, workspace credit wallets (top-up, deduction, rollover, overage), billing analytics |
| FE-07 | Administration & System Mgmt | In-app + email notifications, billing/pricing config, infra health monitoring, audit logs, admin management of users, workspaces and global glossary |

## 3. Actors and roles

| # | Actor | Description |
|---|---|---|
| 1 | Unauthenticated User | Visitor; can view public entry points and start account-access flows |
| 2 | Authenticated User | Signed in; manages personal settings; can join/create a workspace |
| 3 | Workspace Member | Active membership; accesses collaboration features and shared resources |
| 4 | Participant | Member in a live meeting; chat, translation, personal history |
| 5 | Host | Participant with meeting-control privileges (schedule, configure, start, close) |
| 6 | Workspace Owner | Privileged member: setup, member governance, documents, integrations, billing, dashboard (the report also groups "Workspace Owner/Admin" as "workspace managers") |
| 7 | System Handler | Background system actor: ingestion, translation, notification, billing, audit workflows |
| 8 | System Admin | Platform administrator: global accounts, workspaces, policies, subscriptions, announcements |

Workspace roles: Owner, Admin, Member. Membership types: Internal, External. Room roles: Host, Participant (plus External Bridge participant for external rooms).

## 4. Scope

### In scope (Release 1.0)
- Everything in FE-01..FE-07 above.
- Languages for meeting translation: Vietnamese, English, Japanese (UI is English only).
- Clients: web app (Next.js) and desktop app (Electron) - desktop is required for external-platform (Google Meet) integration.
- Use cases UC-01..UC-77 (see 01-functional-requirements.md).

### Limitations and exclusions (LI-x)
| ID | Limitation |
|---|---|
| LI-1 | Requires stable internet; depends on third parties (OpenAI, Cartesia, LiveKit Cloud, Resend, Stripe, Google OAuth, Cloudflare R2, optional plugins). No fallback provider; outages/quota exhaustion disable affected features |
| LI-2 | About one second processing latency per stage; room participant limit by meeting type; subscription plan limits concurrent active rooms |
| LI-3 | Quality degrades with noise, overlapping speech, accents, mic quality, domain terms |
| LI-4 | Only WarpTalk users can be voice-cloned; external-platform participants get a stock voice; external cloned audio goes through a virtual mic and is re-encoded by the platform |
| LI-5 | English-only UI, no i18n layer; translation supports Vietnamese, English, Japanese |
| LI-6 | Capacity/features governed by plans (credits, concurrent rooms, languages, feature flags); workspaces configure artifact retention |
| LI-7 | AI output may be inaccurate; PII masking, DLP and access-scoped retrieval reduce but do not eliminate prompt-injection/data-exposure risk; plugin write actions need explicit user confirmation |
| LI-8 | External integration needs Desktop app; full two-way audio on Windows (VB-CABLE) and macOS (BlackHole); Linux or no driver = captions only; no mobile app, web not mobile-optimized |
| LI-9 | No offline operation; no upload of pre-recorded audio/video for translation; live meetings only |
| LI-10 | External integration supports only Google Meet |
| LI-11 | No guarantee of regulatory, data-residency or industry certification compliance |
| LI-12 | Core AI (STT, translation, TTS, LLM, embeddings) uses third-party cloud APIs; only VAD runs locally; long meetings subject to fixed limits (e.g. 2,000-segment cap for summary regeneration) |
| LI-13 | Screen sharing carries video only (no system/tab audio) |
| LI-14 | AI actions that create/modify/share/send/delete external data require explicit user confirmation; instructions inside retrieved content are never user authorization |

## 5. System context

External entities for Release 1.0: end users (browser/Electron), LiveKit Cloud (WebRTC media), OpenAI (STT, translation, assistant LLM, embeddings), Cartesia (TTS/voice clone), Stripe (payments), Google OAuth (SSO) and Google Meet (external bridge), Resend (email), Cloudflare R2 / MinIO (object storage). Details in 04-architecture.md.

## 6. Assumptions and constraints

- Internet connectivity and availability of third-party providers (LI-1).
- Multi-tenant model: one Workspace = one organization = tenant boundary.
- Desktop app and a virtual audio driver are prerequisites for external-meeting translation (LI-8).
- Consent is a hard precondition for voice cloning (BR-94).
- Schema changes are forward-only SQL migrations applied by a migrator service (see 04-architecture.md).
- Project constraints: 249 man-days estimated effort, 4-member team, Scrum with 2-week sprints (planning artifacts; not product requirements).

## 7. Project-level risks (selected)

| Risk | Impact | Response |
|---|---|---|
| End-to-end translation latency too high | High | Monitor latency, optimize chunking, degrade gracefully |
| TTS latency/rate limits (Cartesia) | High | Quotas, graceful handling, fall back to text-only transcripts |
| Streaming interruptions | Medium | Client reconnection, session recovery, retries |
| Inaccurate AI output | Medium | Label as assistive; allow review/edit |
| Security/privacy/access-control flaws | High | RBAC, workspace authorization, token validation, secret scanning, access-control tests |
| Third-party dependency (OpenAI, Cartesia, LiveKit, Stripe) | High | Health monitoring, retries, quotas, contingency plans |
| External-meeting audio capture issues | Medium | Limit supported platforms, document setup, troubleshooting guide |
