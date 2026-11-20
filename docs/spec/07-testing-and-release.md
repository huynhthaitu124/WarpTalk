# 07 - Testing and Release

Source: Report 7, Software Testing Documentation (V) and Release Package and User Guides (VI). Detailed test cases live in spreadsheets outside the report (unit: `SU26SE112_GSU16_Report5_UnitTest.xls`; integration, system, UAT: `SU26SE112_GSU16_Report5_Test Report.xlsx`). Result charts are images and are not reproduced; no pass/fail counts are available as text.

## 1. Test scope

In scope (feature groups): authentication and session security; workspace, members, invitations, document access; room lifecycle, join flow, waiting approval, recurring meetings, history, artifact access; real-time translation pipeline (routing, voice output, consent, quota exhaustion blocking room creation and join, reconnect recovery); transcript, corrections, artifact share/revoke, minutes link, AI summary, chat, WarpBot, knowledge retrieval, glossary; billing, subscription, credits, invoices, checkout, failed payments, Stripe webhooks; notifications, admin broadcast (SpecificUsers), audit logs, document history, PII/DLP scanning.

In-scope NFRs: tenant and workspace isolation; external members restricted (no full internal directory); document RBAC, deny-by-default, sensitive documents, external grace-period expiry; live transcription and translation latency recorded against a 5 s target in the prepared environment; usability on Chrome and Electron across desktop resolutions, including mic toggle, leave, join-request workflow.

Out of scope: production Kubernetes and cloud networking, installer packages, container orchestration, Terraform pipelines, pgBouncer, Coturn internals; Stripe internals and AI provider algorithms (only Stripe Sandbox integration verified); database failover, self-healing, crash recovery; offline manuals, developer and API reference docs; microphone/camera driver troubleshooting; 90% availability and MTBF measurement; large-scale load, stress, scalability, bandwidth degradation; visual design and subjective UX.

Note: NFR-RL-06 and NFR-PF-08/09 describe load targets while the test plan excludes large-scale load testing; see 99-open-issues.md.

## 2. Levels (V-Model)

| Level | Period | Method | Acceptance criteria |
|---|---|---|---|
| Unit | 19/05/2026 - 15/08/2026 | White-box automated: xUnit, NSubstitute, Moq, FluentAssertions; frontend TypeScript tests; pytest for AI workers | All unit cases pass; business-logic coverage at least 100% |
| Integration | 01/06/2026 - 15/08/2026 | Functional: services to database, gRPC, Redis Streams, Stripe Sandbox, sandbox or mocked external services | Migrations succeed; correct status codes and data; no integrity issues; integrations work or fail gracefully |
| System | 16/08/2026 - 30/08/2026 | Black-box end-to-end by all members: auth, workspace, meetings, live translation, transcript, subscription and payment | E2E flows pass; security controls block unauthorized access; no Blocker or Critical defects open |
| Acceptance | 01/09/2026 - 05/09/2026 | UAT with supervisors, stakeholders, end users; OAT for deployment readiness, config, backup, security (no DR, failover or auto-recovery) | Stakeholders approve; operational checks pass; ready for handover |

Test types by level: functional (all four levels), non-functional (unit, integration, system), white-box (unit, integration), change-related/regression (all levels).

## 3. Tools and environment

| Purpose | Tools |
|---|---|
| Backend | xUnit, Moq, FluentAssertions, WebApplicationFactory, Testcontainers.PostgreSQL |
| AI workers | pytest, pytest-asyncio, pytest-cov |
| API | OpenAPI, Swagger UI, Postman |
| Web | Manual browser testing, frontend TypeScript tests |
| Load (referenced in NFRs) | k6 |
| Dependencies | Docker Desktop with PostgreSQL, Redis, MinIO, Seq |

Environment: .NET SDK 10.0, CPython 3.11+, PostgreSQL 16+ (the platform spec says 18; see open issues), Redis 7.x, RabbitMQ 4.x, LiveKit Cloud or Server 1.13.x, Kubernetes bundled with Docker Desktop, Windows developer OS.

Constraints: no dedicated QA team (developers test, TDD); payment tests only in Stripe Sandbox; AI calls mocked in automated tests to control cost. Assumptions: stable internet, supported Chrome and Electron, working WebRTC and audio hardware.

Milestones: plan 01-06/07/2026; unit, validation, integration of core modules 07/07-05/08; functional, authorization, realtime pipeline 06/08-24/08; regression and full-suite consolidation 01/08-22/09; demo verification and acceptance 15/08-22/09.

## 4. Quality targets

See 02-non-functional-requirements.md section 8 (coverage and defect-rate targets per stage).

## 5. Deliverable package

| Item | Version |
|---|---|
| Schedule/task tracking | V2.1 |
| Project backlog | V4.5 |
| Source code (WarpTalk-CapstoneProject) | v1.5 |
| Database scripts | V8.1 |
| Final report | v7.2 |
| Test case documents (unit; integration/system/UAT) | v1.3 |
| Slides | v1.5 |

Delivery rules: secrets are never in the package and MUST come from local `.env` files; the desktop app is not started by Docker; DB migration MUST complete before gateway, web, AI or desktop testing is considered ready.

## 6. Installation and run

Client requirements: Chrome or Edge on Windows 10+/macOS 13+; 8 GB RAM minimum (16 GB recommended); 4 Mbps minimum; microphone, speaker, camera; desktop app needs 500 MB disk. A virtual audio driver is needed only for external-meeting integration.

Server (local): 4 cores, 8 GB RAM, 20 GB disk minimum (8 cores, 16 GB, 40 GB recommended); Docker Desktop with Compose v2; local ports 3000, 3001, 5200, 5432, 6432, 6379, 5672, 15672, 9000, 9001, 9090, 5341 (and 7880-7882 for self-hosted LiveKit).

Toolchain: Docker Desktop, .NET SDK (net10.0), Node.js 22.x recommended, Python 3.11+, Git and Git Bash on Windows, optional kubectl/K3s, Terraform (Vietnix provisioning only).

Happy path:
1. Copy `.env.example` to `.env` in infrastructure and AI repos; web uses `.env.development` copied to `.env.local`. Fill boot-critical values (see 04-architecture.md section 7).
2. In the infrastructure repo run `./scripts/init-db.sh` (new volume only), then Docker Compose with base and dev files and `up --build`. Check the migrator log and `ps`. `down -v` resets volumes.
3. Optional: run backend services directly with `dotnet run`; web with `npm run dev` (configure the public API, SignalR, LiveKit URL and Google client ID variables).
4. AI workers: create venv, `pip install -e ".[dev,tts,embeddings]"`, start stt, translation, tts, assistant, embedding workers (or the Compose file in warptalk-ai).
5. Desktop: `npm install`, `npm run dev` (or `dev:desktop` if the web app already runs). Installers: Windows `build:win` (bundled web) or `build:win:remote` (production web); macOS and Linux remote builds exist.
6. Verify: containers up, migrator succeeded, gateway reachable, web at port 3000, desktop window loads; provider-backed features tested only after keys are present.

Kubernetes path (Docker Desktop cluster): create namespace, create a generic secret from the `.env` file, apply manifests, port-forward the web service, delete the namespace afterward.

## 7. User manual highlights

| Workflow | Actor | Key steps |
|---|---|---|
| Setup and governance | Owner / Admin | Register or log in, create workspace (Enterprise plan confirmed first), invite members by email with role, approve join requests (subscription member limits enforced), manage roles and meeting-creation access, configure timezone, active-room limit, artifact retention, external collaboration, AI guardrails; glossary import via .xlsx or .csv (Term and Translation required; optional Field, Definition, Note, Part of speech, Priority); documents up to 10 MB with AI-allowed flag, member uploads need Owner/Admin approval to become knowledge |
| Real-time meeting | Host / Participant | Create or schedule, choose languages, invite or share code, admit from waiting room, device check, choose speaking and listening language, host starts and optionally records; Transcript Only mode; Voice Clone Normal or Flash mode; chat, @WarpBot, People panel; host mute, remove, transfer host, End Meeting for All |
| Voice profiles and consent | Member | Preview and select listening voice, accept consent, record or upload sample, preview, save or delete |
| Post-meeting | Host / authorized member | History, transcript and translation, summary, decisions, action items, corrections where permitted, download artifacts, WarpBot Q&A, feedback rating |
| Billing | Owner (or authorized Admin) | Review credits, usage, invoices, top up via Stripe Checkout, manage plan |
| Platform admin | Platform Administrator (System Admin role, separate from workspace roles) | Monitor platform and workspaces, audit logs, global glossary publish/archive/import, billing analytics, suspend or reactivate workspace with required reason |
