# 04 - Architecture

Source: Report 7, SDD IV.1 (Architecture), Release section (deployment options), plus the package-diagram and Google Meet feature notes. Hostnames, keys and connection strings are intentionally omitted; use placeholders such as `<STRIPE_SECRET_KEY>`.

## 1. Style and principles

- Microservices on .NET 10 (ASP.NET Core) behind a YARP API Gateway; each service follows Clean Architecture / DDD (Controller, Service, Repository, UnitOfWork, DbContext; Domain, Application, Infrastructure, API layers).
- Database-per-service on PostgreSQL 18 (see 05-data-model.md). No cross-service joins.
- Real-time AI pipeline in separate Python 3.11+ workers, decoupled from services by Redis Streams.
- Media plane is LiveKit (WebRTC SFU); business plane is the Translation Room service; Meeting service bridges the two.

## 2. Repositories and packages

| Repository | Contents |
|---|---|
| warptalk-web | Next.js 16.3, TypeScript 5, Tailwind 4 web client; also the UI shell loaded by desktop |
| warptalk-desktop | Electron 31.7 wrapper (Windows and macOS; Linux remote build exists), desktop-login route, External Bridge (virtual audio routing, transcript popup window) |
| warptalk-backend | Modules: gateway, auth, workspace, meeting, translation-room, transcript, assistant, billing, notification, payment; shared contract packages `shared` and `contracts` (abstract) |
| warptalk-ai | Workers: livekit_ingress_worker, stt_worker, translation_worker, tts_worker, ai_assistant_worker, embedding_worker, security_worker, billing_worker, metrics_exporter; shared package |
| warptalk-infrastructure | Docker Compose (base and dev), production Docker (three roles: app, data, infra), Kubernetes/K3s manifests, Terraform provisioning, observability, pgbouncer, coturn, scripts (init-db) |

Dependencies: clients depend on backend; backend orchestrates AI runtime; infrastructure provisions and operates both.

## 3. Components

| Component | Responsibility |
|---|---|
| Web / Desktop client | UI, WebRTC via LiveKit client, SignalR for captions/transcripts/room state, REST via gateway |
| API Gateway (YARP) | Routing, JWT validation, rate limiting, aggregation of REST endpoints; SignalR hubs reached through it |
| Auth | Accounts, sessions, Google OAuth, voice profiles and consent |
| Workspace | Tenancy, members, invitations, documents, access policies, entitlements, admin actions |
| Translation Room | Room lifecycle, participants, lobby, audio routes, artifacts, external bridge |
| Meeting | LiveKit room/token management, webhooks, chat, history |
| Transcript | Segments, corrections, glossaries, exports, finalization |
| Assistant (WarpBot) | LLM conversations, tool calls, workspace RAG |
| Billing | Plans, subscriptions, Stripe, credits, usage, invoices |
| Notification | In-app, email, push, templates, admin broadcast |
| Migrator | Applies forward-only SQL migrations; completes before services are ready |
| AI workers | See section 5 |
| Observability | OpenTelemetry, Prometheus, Grafana, Seq, Alertmanager |

## 4. Communication

| Channel | Use |
|---|---|
| REST (HTTPS) | Client to gateway to services |
| SignalR (secure WebSocket) | Live transcript, captions, room state, chat, notifications, AI suggestions |
| WebRTC (LiveKit, DTLS-SRTP) | Audio/video, translated audio tracks |
| gRPC (internal, shared secret `<GRPC_INTERNAL_SECRET>`) | Synchronous service-to-service calls, e.g. Transcript to Translation Room (host check), Assistant to Workspace (workspace context), Translation Room to Transcript (segment save), Notification server (SendNotification, SendEmailInvitation, SendMeetingInvitation) |
| MassTransit / RabbitMQ | Domain events between services (entitlement updates, membership changes, room ended) |
| Redis Streams / Pub-Sub | AI pipeline queues and results; `meeting.end_room`, `meeting.billing.stop`; `translate:results`, `tts:results`; `embedding:index_requests`, `embedding:index_results`; idempotency keys; active-workspace cache |
| LiveKit webhooks | Room, participant, track and egress events into Meeting service |

## 5. Real-time translation pipeline

LiveKit ingress worker subscribes to speaker tracks, applies VAD, streams audio to STT (OpenAI Realtime, one persistent session per speaker), publishes segments; translation worker translates per target language using glossary context; TTS worker (Cartesia) synthesizes standard or cloned voice and publishes audio back to LiveKit tracks; Transcript service stores segments; billing worker consumes `translate:results` and `tts:results` and deducts credits with idempotency keys. Degradation order: voice clone fails then standard TTS; TTS fails then text-only subtitles. Workers use consumer groups with bounded concurrency and scale by consumer lag (see NFR-PF-10).

Other workers: embedding_worker (document and transcript chunks into Qdrant), ai_assistant_worker (LLM prompts, suggestions), security_worker (PII/DLP scan before indexing), metrics_exporter (latency telemetry).

## 6. External integrations

| System | Purpose |
|---|---|
| LiveKit (Cloud or self-host, ports 7880-7882 for self-host) | WebRTC SFU, egress recording |
| OpenAI | STT, translation, assistant, summaries, embeddings (`text-embedding-3-small`) |
| Cartesia | TTS and voice cloning |
| Stripe Checkout and webhooks | Subscriptions, credit top-ups (sandbox in testing) |
| Google OAuth | Sign-in |
| Google Workspace / Calendar / Meet | WarpBot tool `google_calendar_create_meet_event` creates an event with a Meet link; room stored as `EXTERNAL_BRIDGE` with provider metadata (see section 8) |
| Resend or SMTP | Email |
| FCM / Web Push | Push notifications |
| MinIO / Cloudflare R2 (S3-compatible) | Documents, exports, audio, avatars, egress output |
| Qdrant | Vector search |
| Coturn | STUN/TURN |

## 7. Deployment

| Option | Description |
|---|---|
| Local Docker Compose | `docker-compose.yml` plus `docker-compose.dev.yml`; PostgreSQL starts first, migrator must succeed before backend and gateway are ready; web on port 3000 |
| Production Docker | Three-host roles (app, data, infra). Current reference deployment: two VMs, App VM 8 vCPU / 17 GiB, Data VM 4 vCPU / 13 GiB |
| K3s / Kubernetes | Alternative cluster deployment with storage class, load balancer, external secrets, DNS, release scripts; multi-node target prepared for HA; images in GHCR |
| Terraform (Vietnix provider) | Provisions app, data and infra hosts |

CI/CD: GitHub Actions builds images to GHCR; gates include Gitleaks secret scan, dependency checks and access-control tests (NFR-SC-05). The desktop app is not containerized.

Boot-critical configuration (all supplied via untracked `.env`, never committed): database passwords, `<REDIS_PASSWORD>`, RabbitMQ credentials, `<JWT_SECRET>`, `<GRPC_INTERNAL_SECRET>`, observability admin passwords, LiveKit URL/key/secret, provider keys (`<OPENAI_API_KEY>`, Cartesia, `<STRIPE_SECRET_KEY>`, Stripe webhook secret and price IDs, Google client ID, `<RESEND_API_KEY>`), object storage credentials, Qdrant URL/key.

## 8. External meeting bridge (Google Meet)

- Host desktop client opens a publish-only LiveKit bridge connection (bridge token from Meeting service) carrying external audio into the room; a virtual audio driver is required on the host machine.
- WarpBot can create a Google Meet through Google Calendar (requires connected Google Workspace and Calendar API enabled on the OAuth project). The tool result carries provider, eventId, calendarEventLink, meetLink and meetLinkStatus; `hangoutLink` is preferred, otherwise the video entry point URI. The tool is write-effect, so confirmation and audit gates apply. WarpBot MUST return the exact meetLink in its final answer.
- Translation Room stores external provider metadata on an `EXTERNAL_BRIDGE` room and exposes it in list and detail DTOs; the web schedule MUST distinguish external rooms from native ones and show the external join URL; the desktop bridge exposes `openTranscriptWindow` for the transcript popup.
- Out of scope: arbitrary user-defined tools, remote Google MCP tool sync, and the private WarpBot composer inside the popup (tracked as WT-615).

## 9. Security architecture

JWT with rotating refresh tokens; PBKDF2-SHA512 passwords; RBAC at gateway and service; tenant isolation by workspace ID on every query; TLS 1.2+; DTLS-SRTP media; document ACL with deny-overrides; DLP/PII masking before indexing; consent-gated voice cloning; secrets only through environment/secret stores.

## 10. Observability

OpenTelemetry traces and metrics, Prometheus scraping, Grafana dashboards, Seq structured logs, Alertmanager alerts; metrics_exporter publishes AI latency and queue lag; billing diagnostics use budget and rate variables.
