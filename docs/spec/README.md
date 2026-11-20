# WarpTalk Software Specification

WarpTalk (project code SU26SE112) is an AI real-time speech translation platform with opt-in voice cloning, delivered as a web client and an Electron desktop client over .NET microservices and Python AI workers.

Source note: this specification is derived from the Report 7 capstone report, version Sep 2026, with gaps filled from the project's supporting notes (review analysis, Google Meet feature note, package and state diagrams). It distills the report; it does not copy it. Original IDs (FE, LI, UC, BR, MSG) are kept for traceability. NFR IDs are assigned by this spec. Normative words MUST, SHOULD and MAY follow RFC 2119. No secrets or production hostnames appear; placeholders such as `<STRIPE_SECRET_KEY>` are used.

## Index

| File | Content |
|---|---|
| [00-overview.md](00-overview.md) | Vision, features FE-01..FE-07, actors, scope, limitations LI-1..LI-14, assumptions, risks |
| [01-functional-requirements.md](01-functional-requirements.md) | Use cases UC-01..UC-77 with flows and acceptance criteria |
| [02-non-functional-requirements.md](02-non-functional-requirements.md) | Interfaces, usability, reliability, performance, security, scalability, maintainability, quality targets |
| [03-business-rules-and-messages.md](03-business-rules-and-messages.md) | BR-01..BR-100 and MSG01..MSG126 |
| [04-architecture.md](04-architecture.md) | Components, communication, pipeline, integrations, deployment, tech stack |
| [05-data-model.md](05-data-model.md) | 63 tables by service, relationships, Qdrant, storage |
| [05a-data-dictionary.md](05a-data-dictionary.md) | Column-level schema of all 63 tables (types, constraints, indexes, relationships), generated from the ERD DBML |
| [06-service-design.md](06-service-design.md) | Per-service responsibilities, APIs, flows, events, state machines |
| [07-testing-and-release.md](07-testing-and-release.md) | Test strategy, scope, release package, install and user-guide highlights |
| [99-open-issues.md](99-open-issues.md) | Contradictions, gaps, review feedback |

## Scope

In scope for Release 1.0: workspaces and members, translation rooms with real-time STT, translation and TTS, voice cloning with consent, transcripts and exports, WarpBot and workspace knowledge, billing with Stripe, notifications, platform administration, Google Meet external bridge. Languages in Release 1.0: Vietnamese, English, Japanese. UI is English only and not optimized for mobile. See 00-overview.md for limitations.

## Tech stack (summary)

Next.js 16.3, TypeScript 5, Tailwind 4, Electron 31.7; .NET 10 with YARP, MassTransit/RabbitMQ, SignalR, gRPC; PostgreSQL 18, Redis, Qdrant, MinIO or Cloudflare R2; Python 3.11+ workers; LiveKit; OpenAI; Cartesia; Stripe; Resend; Docker Compose, K3s, GitHub Actions.

## Glossary

| Term | Meaning |
|---|---|
| Workspace | Tenant: members, documents, glossary, plan, credits |
| Owner / Admin / Member | Workspace roles; System Admin is a separate platform-level role |
| Internal / External member | Membership type; external members have restricted access and a grace period |
| Translation Room | Business entity for a translated meeting (schedule, participants, languages, artifacts) |
| Meeting Room | LiveKit RTC runtime bound to a Translation Room |
| External Bridge | Room that carries audio from an external platform (for example Google Meet) via the desktop client |
| Audio route | Speaker-to-listener translation path with language pair and voice-clone flag |
| Segment | Speaker-attributed unit of transcript |
| Voice cloning | Opt-in synthesis of a speaker's voice in the target language (Cartesia); needs consent (BR-94) |
| Transcript Only mode | Captions and translated text without synthesized audio |
| WarpBot | In-product AI assistant (chat, RAG, tools) |
| Credit | Billing unit deducted per usage, per second, minimum one credit |
| Glossary | Workspace or global term list applied during translation |
| VAD / STT / MT / TTS | Voice activity detection, speech-to-text, machine translation, text-to-speech |
| RAG | Retrieval-augmented generation over workspace knowledge in Qdrant |
| DLP | Data loss prevention scanning and PII masking before indexing |
| SFU | Selective forwarding unit (LiveKit media server) |
