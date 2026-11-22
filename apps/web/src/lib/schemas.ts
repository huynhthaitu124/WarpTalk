// zod schemas for API DTOs (camelCase JSON, see crates/modules/*/src/http/dto.rs).
import { z } from 'zod';

const uuid = z.string();
const ts = z.string();

export const authResponse = z.object({
  userId: uuid,
  email: z.string(),
  accessToken: z.string(),
  accessTokenExpiresAt: ts,
  refreshToken: z.string(),
});
export type AuthResponse = z.infer<typeof authResponse>;

export const registerResponse = z.object({ userId: uuid });

/** `GET /me`: account + profile in one call. */
export const profile = z.object({
  userId: uuid.optional(),
  email: z.string().optional(),
  fullName: z.string(),
  avatarUrl: z.string().nullable().optional(),
  preferredLanguage: z.string(),
  timezone: z.string().nullable().optional(),
  emailVerified: z.boolean().optional(),
  systemAdmin: z.boolean().optional(),
});
export type Profile = z.infer<typeof profile>;

export const WORKSPACE_ROLES = ['OWNER', 'ADMIN', 'MEMBER'] as const;
export type WorkspaceRole = (typeof WORKSPACE_ROLES)[number];

export const workspace = z.object({
  id: uuid,
  name: z.string(),
  slug: z.string(),
  description: z.string().nullable().optional(),
  logoUrl: z.string().nullable().optional(),
  ownerId: uuid,
  myRole: z.enum(WORKSPACE_ROLES),
  createdAt: ts,
});
export type Workspace = z.infer<typeof workspace>;

export const member = z.object({
  userId: uuid,
  role: z.enum(WORKSPACE_ROLES),
  joinedAt: ts,
  email: z.string().nullable().optional(),
  fullName: z.string().nullable().optional(),
  avatarUrl: z.string().nullable().optional(),
});
export type Member = z.infer<typeof member>;

export const INVITATION_STATUSES = ['pending', 'expired', 'revoked', 'accepted'] as const;
export const invitation = z.object({
  id: uuid,
  workspaceId: uuid,
  email: z.string(),
  role: z.enum(WORKSPACE_ROLES),
  status: z.string(),
  expiresAt: ts,
  acceptedAt: ts.nullable().optional(),
  deliveryStatus: z.string(),
  lastSentAt: ts.nullable().optional(),
  sentCount: z.number(),
  invitedBy: uuid,
  createdAt: ts,
  workspaceName: z.string().optional(),
  inviterEmail: z.string().optional(),
});
export type Invitation = z.infer<typeof invitation>;
export const acceptResult = z.object({ workspaceId: uuid, workspaceName: z.string(), role: z.enum(WORKSPACE_ROLES) });

export const workspaceSettings = z.object({
  maxParticipantsLimit: z.number(),
  allowGuestAccess: z.boolean(),
  defaultAiModel: z.string().nullable().optional(),
  audioRetentionDays: z.number(),
  // Absent on backends that predate the AI privacy switches.
  aiRetrievalAllowed: z.boolean().optional(),
  externalLlmAllowed: z.boolean().optional(),
  aiSuggestEnabled: z.boolean().optional(),
});
export type WorkspaceSettings = z.infer<typeof workspaceSettings>;

export const page = <T extends z.ZodType>(item: T) =>
  z.object({ items: z.array(item), page: z.number(), pageSize: z.number(), total: z.number() });

export const ROOM_STATUSES = [
  'SCHEDULED', 'WAITING', 'OPEN', 'IN_PROGRESS', 'PAUSED', 'ENDED', 'CANCELLED', 'EXPIRED', 'FAILED',
] as const;
export type RoomStatus = (typeof ROOM_STATUSES)[number];

export const bridgeState = z.object({
  provider: z.string(),
  meetingCode: z.string(),
  joinUrl: z.string().nullable().optional(),
  capturerUserId: uuid.nullable().optional(),
  capturerLeaseAt: ts.nullable().optional(),
  audioFlowing: z.boolean().nullable().optional(),
  lastAudioAt: ts.nullable().optional(),
  audioMode: z.string(),
  epoch: z.number(),
  /** Server-computed end of the capturer lease; compare with `serverTime`, never the client clock. */
  leaseExpiresAt: ts.nullable().optional(),
  serverTime: ts.optional(),
});
export type BridgeState = z.infer<typeof bridgeState>;

export const room = z.object({
  id: uuid,
  workspaceId: uuid,
  hostId: uuid,
  activeHostId: uuid.nullable().optional(),
  effectiveHostId: uuid,
  title: z.string(),
  roomCode: z.string(),
  status: z.enum(ROOM_STATUSES),
  sourceLanguage: z.string(),
  targetLanguages: z.array(z.string()),
  requiresApproval: z.boolean(),
  scheduledAt: ts.nullable().optional(),
  startedAt: ts.nullable().optional(),
  endedAt: ts.nullable().optional(),
  createdAt: ts,
  translationActive: z.boolean(),
  roomType: z.string(),
  /** Only on EXTERNAL_BRIDGE rooms. */
  bridge: bridgeState.optional(),
});
export type Room = z.infer<typeof room>;

export const PARTICIPANT_STATUSES = [
  'INVITED', 'WAITING', 'CONNECTED', 'DISCONNECTED', 'LEFT', 'KICKED', 'REJECTED',
] as const;
export const participant = z.object({
  id: uuid,
  userId: uuid.nullable().optional(),
  displayName: z.string(),
  role: z.string(),
  speakLanguage: z.string(),
  listenLanguage: z.string(),
  status: z.string(),
  isUsingVoiceClone: z.boolean(),
  joinedAt: ts.nullable().optional(),
  leftAt: ts.nullable().optional(),
});
export type Participant = z.infer<typeof participant>;

export const routes = z.object({
  generatedAt: ts,
  roomStatus: z.string(),
  translationActive: z.boolean(),
  roomLanguages: z.array(z.string()),
  routes: z.array(
    z.object({
      routeId: uuid,
      sourceParticipantId: uuid,
      targetParticipantId: uuid,
      sourceLanguage: z.string(),
      targetLanguage: z.string(),
      voiceCloneEnabled: z.boolean(),
      status: z.unknown(),
    }),
  ),
});
export type Routes = z.infer<typeof routes>;

export const joinGrant = z.object({
  roomName: z.string(),
  serverUrl: z.string(),
  token: z.string(),
  participantIdentity: z.string(),
});
export type JoinGrant = z.infer<typeof joinGrant>;

export const settingsRead = z.object({
  namespace: z.string(),
  scope: z.unknown().optional(),
  effective: z.boolean().optional(),
  value: z.unknown(),
  version: z.number().nullable().optional(),
});
export type SettingsRead = z.infer<typeof settingsRead>;

export const settingsWrite = z.object({ namespace: z.string(), version: z.number().optional() });

export const errorBody = z.object({ code: z.string(), message: z.string() });

export const claimResponse = room.extend({
  created: z.boolean(),
  bridgeRole: z.enum(['capturer', 'viewer']),
  heartbeatIntervalSeconds: z.number(),
  leaseSeconds: z.number(),
  leaseExpiresAt: ts.nullable().optional(),
  serverTime: ts.optional(),
});
export type ClaimResponse = z.infer<typeof claimResponse>;

/** Answer of bridge heartbeat / takeover / audio-mode: the bridge state flattened next to the room id. */
export const bridgeStateResponse = bridgeState.extend({
  roomId: uuid,
  heartbeatIntervalSeconds: z.number(),
  leaseSeconds: z.number(),
});
export type BridgeStateResponse = z.infer<typeof bridgeStateResponse>;

// ---- knowledge search (GET /knowledge/search)
export const citation = z.object({
  kind: z.string(),
  title: z.string(),
  refId: z.string().optional(),
  page: z.number().optional(),
  headingPath: z.array(z.string()).optional(),
  startMs: z.number().optional(),
  endMs: z.number().optional(),
});
export type Citation = z.infer<typeof citation>;
export const knowledgeHit = z.object({
  chunkId: z.string(),
  score: z.number(),
  sourceType: z.string(),
  sourceId: z.string(),
  text: z.string(),
  citation,
});
export type KnowledgeHit = z.infer<typeof knowledgeHit>;
export const knowledgeSearch = z.object({ chunks: z.array(knowledgeHit), metrics: z.object({ latencyMs: z.number().optional(), returned: z.number().optional() }).passthrough().optional() });

// ---- bring-your-own AI (ai/workspaces/{id}/config)
export const AI_PROVIDERS = ['openai', 'anthropic', 'gemini', 'openai_compatible'] as const;
export type AiProvider = (typeof AI_PROVIDERS)[number];
export const AI_PURPOSES = ['assistant', 'summary', 'suggestion_decide', 'suggestion_generate', 'embedding'] as const;
export type AiPurpose = (typeof AI_PURPOSES)[number];
export const aiPurposeConfig = z.object({
  provider: z.string(),
  model: z.string(),
  baseUrl: z.string().nullable().optional(),
  reasoning: z.string().nullable().optional(),
  maxOutputTokens: z.number().nullable().optional(),
  timeoutMs: z.number().nullable().optional(),
  dataBoundary: z.string(),
  fallbackPolicy: z.string(),
  enabled: z.boolean(),
  status: z.string(),
  lastValidatedAt: ts.nullable().optional(),
  lastErrorCode: z.string().nullable().optional(),
  version: z.number(),
});
export type AiPurposeConfig = z.infer<typeof aiPurposeConfig>;
/** Credentials never carry key material: only the last four characters. */
export const aiCredential = z.object({ provider: z.string(), last4: z.string(), createdAt: ts, rotatedAt: ts.nullable().optional(), validatedAt: ts.nullable().optional() });
export type AiCredential = z.infer<typeof aiCredential>;
export const aiConfig = z.object({
  byoEnabled: z.boolean(),
  purposes: z.record(z.string(), aiPurposeConfig),
  credentials: z.array(aiCredential),
  reindexRequired: z.boolean().optional(),
});
export type AiConfig = z.infer<typeof aiConfig>;
export const aiTest = z.object({ ok: z.boolean(), latencyMs: z.number(), errorCode: z.string().nullable().optional() });

// ---- meeting chat (crates/modules/meeting/src/http/dto.rs)
export const chatMessage = z.object({
  messageId: uuid,
  senderId: uuid.nullable().optional(),
  senderName: z.string(),
  content: z.string(),
  createdAt: ts,
  hidden: z.boolean(),
  /** Translations by target language code. */
  translations: z.record(z.string(), z.string()).default({}),
});
export type ChatMessageDto = z.infer<typeof chatMessage>;

// ---- glossary (crates/modules/transcript/src/http/glossary.rs)
export const glossary = z.object({
  id: uuid,
  workspaceId: uuid,
  name: z.string(),
  description: z.string().nullable().optional(),
  sourceLanguage: z.string(),
  targetLanguage: z.string(),
  termCount: z.number(),
  createdAt: ts,
  updatedAt: ts.nullable().optional(),
});
export type Glossary = z.infer<typeof glossary>;
export const glossaryTerm = z.object({
  id: uuid,
  glossaryId: uuid,
  sourceTerm: z.string(),
  targetTerm: z.string(),
  caseSensitive: z.boolean(),
  notes: z.string().nullable().optional(),
  createdAt: ts,
});
export type GlossaryTerm = z.infer<typeof glossaryTerm>;
export const importResult = z.object({ imported: z.number(), skipped: z.number() });

export const GLOBAL_STATUSES = ['draft', 'published', 'archived'] as const;
export type GlobalStatus = (typeof GLOBAL_STATUSES)[number];
export const globalTerm = z.object({
  id: uuid,
  term: z.string(),
  preferredTranslation: z.string(),
  sourceLanguage: z.string().nullable().optional(),
  targetLanguage: z.string().nullable().optional(),
  businessDomain: z.string().nullable().optional(),
  definition: z.string().nullable().optional(),
  usageNote: z.string().nullable().optional(),
  priority: z.number(),
  status: z.enum(GLOBAL_STATUSES),
  version: z.number(),
  createdAt: ts,
  updatedAt: ts.nullable().optional(),
});
export type GlobalTerm = z.infer<typeof globalTerm>;
export const globalAudit = z.object({
  id: uuid,
  termId: uuid,
  action: z.string(),
  beforeJson: z.unknown().optional(),
  afterJson: z.unknown().optional(),
  actorUserId: uuid,
  createdAt: ts,
});
export type GlobalAudit = z.infer<typeof globalAudit>;

// ---- documents (crates/modules/workspace/src/http/documents.rs)
export const DOC_CATEGORIES = ['GENERAL', 'TECHNICAL', 'FINANCIAL', 'LEGAL', 'MEETING_NOTES'] as const;
export const DOC_ACCESS_LEVELS = ['PUBLIC', 'MEMBERS_ONLY', 'ADMIN_ONLY'] as const;
export type DocAccessLevel = (typeof DOC_ACCESS_LEVELS)[number];
export const INGESTION_STATUSES = ['pending', 'completed', 'skipped', 'failed'] as const;
export const document = z.object({
  id: uuid,
  workspaceId: uuid,
  title: z.string(),
  fileName: z.string(),
  fileExtension: z.string(),
  mimeType: z.string(),
  sizeBytes: z.number(),
  category: z.string(),
  accessLevel: z.string(),
  aiRetrievalAllowed: z.boolean(),
  aiEligible: z.boolean(),
  ingestionStatus: z.string(),
  ingestionFailureReason: z.string().nullable().optional(),
  lastIndexedAt: ts.nullable().optional(),
  uploadedBy: uuid.nullable().optional(),
  createdAt: ts,
  updatedAt: ts.nullable().optional(),
});
export type WorkspaceDocument = z.infer<typeof document>;

// ---- voice profile (crates/modules/identity/src/http/dto.rs)
export const voiceStatus = z.object({
  consent: z.object({ granted: z.boolean(), consentVersion: z.string().nullable().optional(), grantedAt: ts.nullable().optional() }),
  profile: z.object({
    id: uuid,
    status: z.string(),
    language: z.string().nullable().optional(),
    qualityScore: z.number().nullable().optional(),
    createdAt: ts,
  }).nullable().optional(),
  cloneEnabled: z.boolean(),
});
export type VoiceStatus = z.infer<typeof voiceStatus>;
export const voiceEnrollResult = z.object({
  profileId: uuid,
  status: z.string(),
  qualityScore: z.number(),
  durationSeconds: z.number(),
  language: z.string(),
});
export type VoiceEnrollResult = z.infer<typeof voiceEnrollResult>;
