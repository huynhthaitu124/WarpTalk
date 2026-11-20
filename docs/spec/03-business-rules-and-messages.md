# 03 - Business Rules and Application Messages

Source: Report 7, SRS 5.1 (BR-01..BR-100) and 5.3 (MSG01..MSG126). Rule text is condensed; IDs are preserved verbatim. BR-83 and BR-84 are marked Future/Proposed in the report.

## 1. Business rules

### Authentication and account (BR-01..BR-07, BR-87, BR-90)
| ID | Rule |
|---|---|
| BR-01 | Disabled, locked, pending-verification or soft-deleted accounts MUST NOT obtain or refresh a normal session; issued access tokens stay valid until expiry. |
| BR-02 | A new local account MUST verify email before first normal login. |
| BR-03 | 5 consecutive failed logins (configurable) lock the account 15 min (configurable); blocked until expiry or admin unlock. |
| BR-04 | Google login/linking only for tokens issued to WarpTalk's OAuth client; auto-link to a local password account only if that account is email-verified. |
| BR-05 | Google linking fails if Google email differs (case-insensitive); unlinking forbidden if no local password. |
| BR-06 | Logout revokes the presented token's refresh family; password change/reset revokes all of the user's refresh tokens. |
| BR-07 | Users read/update only own settings/profile; languages are BCP-47; timezone UTC or valid IANA ID. |
| BR-87 | Verification-email resend: 60 s cooldown per account, 5 per 15 min; same policy when Google login auto-triggers verification. |
| BR-90 | Reset and verification tokens are single-use, only latest valid, expire (verification 60 min, reset 30 min). |

### Workspace and membership (BR-08..BR-44, BR-88, BR-89)
| ID | Rule |
|---|---|
| BR-08 | Single Enterprise Workspace model; behavior governed by workspace policy and subscription entitlements (no personal-workspace branch). |
| BR-09 | Creating a workspace bootstraps creator as first active Owner. |
| BR-10 | Exactly one current Owner per workspace; Owner cannot leave or be removed, only transferred. |
| BR-11 | Only the Owner transfers ownership; new owner is an active non-external member (email on a verified domain if domains exist); previous Owner becomes Admin. |
| BR-12 | Only Owner/Admin update workspace settings. |
| BR-13 | Owner-only: external-collaboration toggle, verified-domain add/revoke, role elevation/demotion, ownership transfer, workspace deletion. |
| BR-14 | Role elevation and demotion are Owner-only. |
| BR-15 | Admins perform only explicitly allowed operational member actions; cannot manage Owner or peer Admins or self-grant restricted meeting permissions. |
| BR-16 | Removing a member removes normal authorization without deleting audit, billing, meeting or document history. |
| BR-17 | Only Owner/Admin create invitations; only Owner invites Admin; Owner role never granted by invitation. |
| BR-18..BR-23 | External Members: restricted collaborators; MUST stay MembershipType=External with role Member; no Owner/Admin authority; cannot create invitations or manage settings/billing/ownership; no unrestricted internal member data; only explicitly allowed resources. |
| BR-24 | Owner may add a company domain only if non-public and unclaimed; a domain different from Owner's email domain needs explicit self-asserted consent. |
| BR-25 | Owner-added domains count as verified (legal verification out of scope). |
| BR-26 | When enforcement active (at least one active verified domain), verified domains decide Internal classification. |
| BR-27 | Public email domains cannot be verified enterprise domains. |
| BR-28 | One active verified domain belongs to at most one workspace. |
| BR-29 | Without enforcement, Internal invitations need no domain validation. |
| BR-30 | A user may belong to multiple workspaces. |
| BR-31 | At most one active Internal membership in an Enterprise Workspace at a time. |
| BR-32 | Additional memberships MUST be External; Internal attempt is refused, not reclassified. |
| BR-33 | Invitations expire after the workspace window (default 7 days). |
| BR-34 | Resend after failed delivery marks previous token REPLACED; re-invite of a non-acceptable pending invitation revokes it. |
| BR-35 | Only newest pending, unexpired token is acceptable. |
| BR-36 | Acceptance is identity-bound to invited email. |
| BR-37..BR-39 | Acceptance preserves inviter's intended membership class; admit only as invited Internal/External or reject; no silent reclassification even under current domain-policy evaluation. |
| BR-40 | External collaboration on by default, Owner can disable; while off, outside-domain users cannot be invited/approved as External. |
| BR-41 | Join requests create Member-level access only; only Owner/Admin approve/reject. |
| BR-42 | Repeated join request for same workspace/email returns the open request. |
| BR-43 | Workspace-scoped core features need an active workspace context; preflight treats inactive/soft-deleted as unavailable. |
| BR-44 | Approval atomically converts request to ACCEPTED and creates exactly one membership; rejection only changes review tracking and status. |
| BR-88 | High-risk role elevation needs fresh explicit confirmation (signed preview valid 15 min); stale/superseded approvals rejected. |
| BR-89 | Ownership transfer and workspace deletion need explicit confirmation; stale/duplicate submissions rejected once state changed. |

### Translation room, meeting, workspace-tenant rules (BR-45..BR-55, BR-80..BR-86, BR-91..BR-93, BR-97, BR-99, BR-100)
| ID | Rule |
|---|---|
| BR-45 | Join only existing, non-soft-deleted, non-terminal (ENDED/CANCELLED/EXPIRED) rooms. |
| BR-46 | Host bypasses approval (CONNECTED/HOST); non-host first join follows requires_approval (default true); admitted participants may rejoin without re-approval. |
| BR-47 | Kicked participant is permanently blocked unless a future re-invite flow changes that. |
| BR-48 | Room settings editable only while SCHEDULED/WAITING, except artifact-sharing access and live host controls (lock, mute-on-entry); later joiners obey latest settings. |
| BR-49 | Lifecycle transitions across SCHEDULED, WAITING, IN_PROGRESS, PAUSED, ENDED, CANCELLED, EXPIRED stay legal and consistent; discarded drafts not persisted. |
| BR-50 | Only authorized host-side controls kick or transfer host (current host only); only current host or workspace Owner/Admin admit/reject waiting users or end a live room. |
| BR-51 | Participant lifecycle state is separate from per-participant translation-audio state. |
| BR-52 | Workspace language policy bounds source/target languages at create and edit; unsupported languages rejected before live processing. |
| BR-53 | Reminders at T-30, T-10, T-1 min, once per window, no duplicates on retry. |
| BR-54 | Only users who joined the live session view/send chat; moderation hides without erasing original. |
| BR-55 | Transcript access limited to host and actual participants; after meeting only if host shared (artifactAccess=ALL_PARTICIPANTS, default HOST_ONLY); invited-never-joined emails cannot read. |
| BR-80 | Meetings are workspace-owned tenant assets; creation/listing governed by workspace membership and policy. |
| BR-81 | Meeting creation only for members granted authority; still subject to active-room quota and language policy. |
| BR-82 | Non-members denied by room ID/link; joining by room code controlled by approval setting. |
| BR-83 [Future/Proposed] | Transcript/summary/artifact retention follows workspace governance; audit metadata preserved after cleanup. |
| BR-84 [Future/Proposed] | Removing a member evicts them from active meetings. |
| BR-85 | Workspace deletion cancels in-progress meetings; suspension blocks new room creation, admission, start. |
| BR-86 | Active-room and target-language quotas follow subscription terms at creation; participant capacity follows room maximum. |
| BR-91 | Audio routes are generated for live translation but do not block it when missing; COMPLETED routes are terminal and not updatable. |
| BR-92 | Scheduled links expire 2 h after scheduled start; sessions with no connected participants auto-end after idle window (5 min). |
| BR-93 | Recurring occurrences materialized without duplicates per series date. |
| BR-97 | Suspended/inactive/soft-deleted workspaces block meeting creation, admission, start, document ops, join/leave requests. |
| BR-99 | Host transfer only by current host to a participant in the live session. |
| BR-100 | Assistant requests must come from active participant/host, use only eligible context, and store a fallback result on failure. |

### Transcript, documents, glossary (BR-56..BR-64, BR-95, BR-98)
| ID | Rule |
|---|---|
| BR-56 | Correction allowed only while transcript is not FINALIZED/ARCHIVED and segment exists; preserve source linkage; re-translation keeps prior translations as non-current history. |
| BR-57 | Global glossary is platform-level, System Admin managed; only published terms used; archived not used. |
| BR-58 | Workspace documents are enterprise-owned assets governed by membership and policy. |
| BR-59 | Access policy is deny-overrides. |
| BR-60 | Pending-ingestion/awaiting-approval docs fail-closed (Owner/Admin and uploader only); restricted docs need Owner/Admin, uploader or explicit allow. |
| BR-61 | External members: only explicit grant or meeting-scoped exception within grace period (default 24 h) after a meeting they joined. |
| BR-62 | Allowed types: pdf, docx, xlsx, md, png, jpg, jpeg, webp, bmp, gif; reject mismatched content, empty files, over 10 MB. |
| BR-63 | AI retrieval only for active, retention-active, ingestion-completed, indexed, AI-eligible documents. |
| BR-64 | Upload, view, download, delete produce an audit trail. |
| BR-95 | Pending-approval restricted to Owner/Admin; access-policy mutation to Owner/Admin or document owner; auditable. |
| BR-98 | Glossary terms scoped by workspace or platform and validated; changes apply to meetings started after the change; cached prompts in running meetings may keep old terms up to 24 h. |

### Billing and credits (BR-60..BR-62 apply to payments in SRS text; BR-65..BR-74)
| ID | Rule |
|---|---|
| BR-65 | One active subscription per workspace. |
| BR-66 | Paid AI ops are metered and settled against subscription state, AI service state, credits and overage policy; ineligible workspace is suspended from AI usage. |
| BR-67 | When credits run out, usage continues only as overage if allowed, up to cap (balance may go negative within cap); otherwise charge refused and AI service suspended. |
| BR-68 | Metering by usage event and processing type using rate-card unit; live translation and TTS billed per second of processed audio, minimum 1 credit per event; charge only when service actually used. |
| BR-69 | Every AI charge, top-up, purchase, renewal and admin adjustment creates a credit-ledger transaction with traceable references. |
| BR-70 | Settlement idempotent and concurrency-safe. |
| BR-71 | Cancellation: immediate for trials, end-of-period for ongoing paid subscriptions. |
| BR-72 | Credit and payment histories paginated, newest first. |
| BR-73 | Billing/payment management (plans, checkout, top-ups, cancellation, overage setting) restricted to Owner/Admin; only Owner may pay invoices. |
| BR-74 | Plan management is System Admin only; deactivated plans not selectable for new purchases. |

Note: the SRS function sections cite BR-60/61/62/69 for Subscribe & Pay although BR-60..62 are document rules (see 99-open-issues.md).

### Notification and admin (BR-75..BR-79, BR-96)
| ID | Rule |
|---|---|
| BR-75 | Notification preferences default to all channels enabled; email honors preference. |
| BR-76 | Users read/mark only own notifications; pagination page size clamped 1-100 (default 50). |
| BR-77 | Admin announcements: payload validated by type, HTML tags rejected, targets deduplicated and capped at 10,000, fan-out in events of at most 1,000 recipients. |
| BR-78 | Publishing to the delivery stream is the synchronous success boundary. |
| BR-79 | System-admin workspace actions (suspend/reactivate) gated by System Admin policy and written to admin audit log. |
| BR-94 | Voice cloning opt-in, default disabled, requires active consent record, only speaker's own outgoing routes, falls back to default voice. |
| BR-96 | Admin announcements store audience mode; only specific-users supported (broadcast/segment reserved). |

## 2. Application messages

Types: In line (inline form/status text) or Toast. Placeholders in braces. Verbatim text is condensed to meaning for non-parameterized items; parameterized texts are quoted.

| Range | Area | Notes |
|---|---|---|
| MSG01-MSG04 | Login errors | MSG01 invalid email or password; MSG02 "Account locked until {locked_until}."; MSG03 account deactivated; MSG04 email not verified |
| MSG05-MSG08 | Validation | MSG05 required field; MSG06 invalid email; MSG07 "Exceeds max length of {max_length}."; MSG08 password policy |
| MSG09-MSG17 | Register/verify/login/reset | MSG09 email already registered; MSG10 registration success; MSG11 email verified; MSG12 verification token invalid/expired; MSG13 login success; MSG14 logout; MSG15 reset email sent; MSG16 reset token invalid/expired; MSG17 password updated |
| MSG18-MSG22 | Google, profile | MSG18 invalid Google token; MSG19 Google email mismatch; MSG20 cannot unlink Google without local password; MSG21 profile updated; MSG22 preferences updated |
| MSG23-MSG44 | Workspace | MSG23 no workspaces found; MSG24 already internal member elsewhere; MSG25 public domain cannot be verified; MSG26 domain already claimed; MSG27 workspace created; MSG28 joined; MSG29 access request submitted; MSG30 invitation invalid/expired; MSG31 "Invitation sent to {email_address}."; MSG32 resent; MSG33 revoked; MSG34 invitation replaced; MSG35 role updated; MSG36 only Owner can change roles; MSG37 ownership transferred; MSG38 new owner must be active internal member; MSG39 member removed; MSG40 settings saved; MSG41 only Owner/authorized Admin update settings; MSG42 owner-only governance; MSG43 workspace deactivated; MSG44 "Domain {domain} verified & added successfully." |
| MSG45-MSG54 | Documents, glossary | MSG45 uploaded; MSG46 unsupported type; MSG47 "File exceeds the {max_size} limit."; MSG48 access denied by ACL; MSG49 policy updated; MSG50 rule removed; MSG51-53 glossary term added/updated/deleted; MSG54 no search results |
| MSG55-MSG78 | Rooms, session, transcript, chat | MSG55 room created; MSG56 updated; MSG57 cancelled; MSG58 invite link copied; MSG59 joined; MSG60 invalid invite/access denied; MSG61 start requires at least one audio route; MSG62 cannot update COMPLETED route; MSG63 unauthorized room control; MSG64 meeting started; MSG65 ended; MSG66 participant removed; MSG67 host transferred; MSG68 participant admitted; MSG69 rejected from lobby; MSG70 audio route updated; MSG71 realtime translation started; MSG72 correction submitted; MSG73 correction not allowed for archived transcript / segment must exist; MSG74 transcript exported; MSG75 AI summary ready; MSG76 summary not yet available; MSG77 chat sent; MSG78 chat only during active session |
| MSG79-MSG86 | Voice, plan, credits | MSG79 voice profile created; MSG80 deleted; MSG81 voice cloning needs consent first; MSG82 feature blocked by plan; MSG83 insufficient credits; MSG84 "Successfully switched to {plan_name} plan."; MSG85 subscription cancelled; MSG86 direct top-up disabled, use Stripe Checkout + verified webhook |
| MSG87-MSG92 | Payment, plans | MSG87 payment completed; MSG88 payment failed; MSG89 billing access denied; MSG90-92 plan created/updated/deactivated |
| MSG93-MSG100 | Admin | MSG93 admin notification sent; MSG94 system notification broadcast; MSG95-MSG98 global glossary term draft/updated/published/archived ("{term}"); MSG99 workspace suspended; MSG100 reactivated |
| MSG101-MSG106 | Platform errors | MSG101 System Admin permission required; MSG102 authentication required; MSG103 access denied; MSG104 network/server error; MSG105 unexpected error; MSG106 workspace suspended/inactive |
| MSG107-MSG115 | Room/artifact errors | MSG107 room not found; MSG108 participant not found; MSG109 not in waiting room; MSG110 kicked cannot rejoin; MSG111 scheduled link expired; MSG112 artifacts only for finished rooms; MSG113 artifact not found; MSG114 retention expired; MSG115 consent required before artifact download |
| MSG116-MSG120 | Assistant | MSG116 message content required; MSG117 conversation not found; MSG118 message processing failed; MSG119 conversations failed to load; MSG120 conversation failed to load |
| MSG121-MSG126 | Misc | MSG121 global term deleted; MSG122 global term min 3 characters (short common words risk hijacking STT); MSG123 preferred translation cannot be empty; MSG124 notification title required; MSG125 content required; MSG126 voice cloning enabled |
