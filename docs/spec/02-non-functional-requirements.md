# 02 - Non-Functional Requirements

Source: Report 7, SRS 4 (External Interfaces, Quality Attributes). IDs below (NFR-xx) are assigned by this spec for traceability; the report does not number them. Targets marked "target" are not yet measured in production.

## 1. External interfaces

| ID | Requirement |
|---|---|
| NFR-IF-01 | Web (Next.js) and Desktop (Electron) MUST share one UI; desktop loads the same web UI starting from the desktop login flow. |
| NFR-IF-02 | Clients access media devices (mic, camera, speaker) through browser/device media APIs and WebRTC (LiveKit). |
| NFR-IF-03 | Backend MUST expose REST via API Gateway and SignalR (secure WebSocket) hubs for transcripts, captions and room state. Internal communication uses gRPC and Redis Streams/Pub/Sub. OpenAPI/Swagger SHOULD be available per service where enabled. |
| NFR-IF-04 | LiveKit Cloud: backend issues signed room-scoped join tokens and receives LiveKit lifecycle webhooks (room, participant, track, egress). |
| NFR-IF-05 | Google Meet bridge: host client opens a publish-only LiveKit "bridge" connection carrying Google Meet audio into the room. |
| NFR-IF-06 | STT via OpenAI persistent per-speaker Realtime transcription sessions; translation/summaries/action items/knowledge extraction/assistant via OpenAI LLMs; TTS and opt-in voice cloning via Cartesia, results published back through LiveKit audio tracks. |
| NFR-IF-07 | Payment state MUST be confirmed via Stripe webhook or checkout-session verification before credits/subscription change. |
| NFR-IF-08 | Google sign-in: frontend obtains Google token, backend verifies before create/link/authenticate. |

## 2. Usability

- NFR-US-01: A new user SHOULD be able to register, create a workspace and start a translated room with minimal guidance.
- NFR-US-02: System MUST show descriptive errors and real-time status indicators (translation state, connection state, network quality).
- NFR-US-03: Web and Desktop use consistent layouts/terminology and synchronized dual-language transcript views.
- NFR-US-04: Common settings (audio, video, language) SHOULD need few actions before joining/starting.
- UI is English only (LI-5); not optimized for mobile (LI-8).

## 3. Reliability

| ID | Requirement |
|---|---|
| NFR-RL-01 | Availability target 90% during business hours (web, desktop, APIs); AI features depend on upstream providers. |
| NFR-RL-02 | MTBF not measured for initial release; critical failures detected via logs/monitoring; MTTR target under 30 minutes. |
| NFR-RL-03 | Degraded mode: voice cloning failure falls back to standard built-in TTS voice; total TTS failure falls back to translated text subtitles/transcripts; LiveKit adapts media via simulcast/adaptive bitrate. |
| NFR-RL-04 | Every billing charge MUST carry an idempotency key (no duplicate charge on Redis redelivery/worker restart). |
| NFR-RL-05 | Transcript corrections MUST be stored as separate records keeping original and corrected text, author and review status (auditable). |
| NFR-RL-06 | Load test (k6: login, workspace list, room join, transcript retrieval, SignalR handshake): HTTP error rate below 1%, checks above 99%, API P95 below 500 ms, SignalR handshake success above 99%, no service restarts, DB pool exhaustion or queue backlog growth. |

## 4. Performance

| ID | Metric | Target |
|---|---|---|
| NFR-PF-01 | STT partial transcript latency | about 0.8-1.0 s |
| NFR-PF-02 | STT final segment commit (4 s utterance) | about 1.5-2.0 s |
| NFR-PF-03 | REST API | P95 below 1.0 s, median below 300 ms |
| NFR-PF-04 | Transcript and text translation delivery | below 2.0 s from end of utterance |
| NFR-PF-05 | Speech-to-speech, standard TTS | below 2.0-2.5 s |
| NFR-PF-06 | Speech-to-speech, voice cloning | below 3.0-3.5 s |
| NFR-PF-07 | Post-meeting AI jobs (summary, action items, artifacts) | complete within 30-45 s after meeting end |
| NFR-PF-08 | Concurrency | scale horizontally to at least 1,200 concurrent users (to be validated by staging load tests; not yet achieved) |
| NFR-PF-09 | API Gateway throughput | target up to 800 req/s (not measured). Local baseline: about 30-35 req/s, 14 VUs, 0% errors, P95 57-79 ms |
| NFR-PF-10 | AI workload resilience | work queued in Redis Streams with bounded worker concurrency; benchmark 120 messages x 8 target languages, zero pending; scale AI worker replicas when consumer lag exceeds 2 minutes; workers report end-to-end latency telemetry |

Current deployment: two-VM Docker Compose (App VM 8 vCPU / 17 GiB; Data VM 4 vCPU / 13 GiB); multi-node K3s target prepared for HA.

## 5. Security

- NFR-SC-01: JWT validation; refresh tokens rotated per use with family-based reuse detection (a replayed old token invalidates the family).
- NFR-SC-02: Passwords hashed with PBKDF2-SHA512, 100,000 iterations, random salt; never stored/logged in plain text. (See 99-open-issues.md for a possible mismatch with the Auth detailed design.)
- NFR-SC-03: RBAC everywhere; HTTPS/TLS 1.2+ and secure WebSockets in transit; media encrypted with DTLS-SRTP.
- NFR-SC-04: Voice cloning is strictly opt-in (VoiceCloneEnabled=true plus valid consent), never default (BR-94).
- NFR-SC-05: CI MUST run secret scanning (gitleaks), dependency checks and access-control tests before release; secrets MUST NOT be committed.
- NFR-SC-06: AI guardrails: PII masking/DLP before indexing, access-scoped retrieval, plugin write actions require explicit confirmation (LI-7, LI-14).

## 6. Scalability

- NFR-SL-01: Cloud-native, horizontally scalable (stateless services behind gateway; Redis Streams consumer groups for workers).
- NFR-SL-02: Production-scale capacity MUST be confirmed through staging load tests before each release (release gates).

## 7. Maintainability

- NFR-MT-01: DDD and Clean Architecture; separate Domain, Application, Infrastructure, API layers.
- NFR-MT-02: Services containerized with Docker; independent deployment.
- NFR-MT-03: Tests: xUnit, Moq, NSubstitute, FluentAssertions, Testcontainers (PostgreSQL) for .NET; pytest for AI workers; Postman for API integration; k6 for load.
- NFR-MT-04: Schema changes are forward-only ordered SQL migrations tracked in `public.schema_migrations`; static analysis (Ruff, Mypy, ESLint + TS strict, .NET analyzers).

## 8. Quality objectives (project QA targets)

| Stage | Coverage | Defect target |
|---|---|---|
| Unit | 100% of business logic | at most 1 defect per 10 test cases, below 10% |
| Integration | major cross-service API contracts | at most 2 defects, below 5% |
| System | all critical user flows | at most 3 functional defects, below 5% |
| Acceptance | 100% acceptance criteria | 0 blocking defects |

## 9. Observability

OpenTelemetry, Prometheus, Grafana, Seq, Alertmanager; AI workers expose metrics via a metrics exporter (see 04-architecture.md).
