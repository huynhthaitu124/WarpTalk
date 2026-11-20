# 99 - Open Issues, Contradictions and Gaps

Items are grouped by type. "Source" names the origin; resolve by confirming against code or migrations.

## 1. Contradictions inside the report

| ID | Issue | Detail |
|---|---|---|
| OI-01 | Password hashing | SRS and security NFR state PBKDF2-SHA512 with 100,000 iterations; the Auth detailed design may name a different hasher (possible mismatch; confirm in code and keep one statement) |
| OI-02 | PostgreSQL version | Architecture states PostgreSQL 18; the test environment table lists 16+ |
| OI-03 | Billing unit | Plan and subscription DTOs expose minute quotas (TranslationMinutesQuota, MonthlyMinutesQuota, BalanceMinutes, AmountMinutes) while billing rules and the worker use credits with per-second metering and a one-credit minimum |
| OI-04 | Payment providers | Payments table description names VNPay in addition to Stripe, but the integration, NFR and test scope cover Stripe only |
| OI-05 | Load and availability targets | NFR-PF-08/09 and NFR-RL-06 define load targets (1,200 users, 800 req/s, k6 thresholds) and a 90% availability target, yet the test plan excludes load, stress, scalability and availability measurement; targets are unverified |
| OI-06 | Unit coverage target | Unit acceptance requires 100% business-logic coverage, while the quality-objective table allows up to one defect per ten cases; no coverage result is reported in text |
| OI-07 | Orchestration terms | Documents refer to Kubernetes, K3s and "Kurbenetes" interchangeably; the test environment lists Kubernetes 1.37.x and deployment uses Docker Compose today with K3s as target |
| OI-08 | Meeting finish status | Meeting room end status is FINISHED in the Meeting service while the Translation Room uses ENDED; mapping between them is not stated |
| OI-09 | Artifact access default | Artifact access defaults to HOST_ONLY, but the user manual shows participants reviewing artifacts; the ALL_PARTICIPANTS toggle path is only partly documented |
| OI-10 | Test-plan dates | Unit/integration periods run to 15/08/2026 while milestones list regression and acceptance work to 22/09/2026, after the stated acceptance period (01-05/09/2026) |

## 2. Gaps in the source

| ID | Gap | Impact |
|---|---|---|
| GP-01 | Column-level database design exists only as ER images in Report 7 | Resolved: 05a-data-dictionary.md generated from the ERD DBML (migrations to 2026-08-13). Report 7 table counts and some table names may lag that source |
| GP-02 | Sequence diagrams are images | Behaviour in 06-service-design.md is derived from class tables, FR flows and BRs |
| GP-03 | Test results (unit, integration, system, UAT) are figures only | No pass/fail counts, defect numbers or coverage values are available |
| GP-04 | Full per-test-case lists are in external spreadsheets | Not summarized beyond scope and strategy |
| GP-05 | No numbered NFR identifiers in the report | NFR-xx IDs in 02 are assigned by this spec |
| GP-06 | BR-83 (artifact retention) marked as future | Retention enforcement behaviour undefined |
| GP-07 | Rate card values, plan prices, credit prices and overage cap amounts are not given as text | Billing amounts cannot be specified here |
| GP-08 | Supported languages limited to Vietnamese, English, Japanese in Release 1.0 | Review feedback discusses Italian and other languages; scope for additional languages undecided |
| GP-09 | Streaming audio protocol and STT provider decisions were open in the review meeting | Report 7 states OpenAI Realtime over LiveKit; confirm this closed the review question |
| GP-10 | Multi-tenant structure discussion was unreadable in the review recording | Workspace-as-tenant model is the implemented answer; confirm with developers |

## 3. Review feedback (review meeting analysis)

Source: review-meeting analysis notes (transcript quality poor; terms marked uncertain there). Items and their status against Report 7:

| ID | Feedback | Status |
|---|---|---|
| RV-01 | Bill per segment duration, not per word and not per session; session cost is the sum of segment costs | Report 7 states per-second billing with idempotency keys; per-segment traceability is not shown. `usage_records` was reported to lack `segment_id` and the review proposed adding it. Confirm in migrations |
| RV-02 | Add a rate table per usage type and language | Report 7 lists `usage_rate_card` (per feature and model); per-language rates are not confirmed |
| RV-03 | Unify units: rename `translation_chunks` to a duration unit | Confirm `usage_records` enum in migrations |
| RV-04 | Correction semantics: overwrite versus keep parallel versions | Report 7 chose separate correction records with current/non-current translation links (NFR-RL-05) |
| RV-05 | Multiple target languages per segment | Implemented through translation contents and segment links (unique per segment and target language in the reviewed schema) |
| RV-06 | Workspace owner sees remaining usage and per-member usage | Billing exposes workspace usage; per-member usage view is not specified |
| RV-07 | Voice-clone flags on participant and audio route | Present (participant voice-clone use, route voice-clone flag) |

## 4. Recommended follow-ups

1. Reconcile OI-01, OI-02, OI-03, OI-04 and OI-08 against code before baselining this spec.
2. ~~Generate column-level tables~~ done in 05a-data-dictionary.md; regenerate it when the DBML changes.
3. Decide whether load targets stay normative or become informative until staging load tests run.
4. Record test results in text (counts, coverage) so acceptance can be traced.
