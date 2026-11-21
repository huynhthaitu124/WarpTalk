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

export const profile = z.object({
  fullName: z.string(),
  avatarUrl: z.string().nullable().optional(),
  preferredLanguage: z.string(),
  timezone: z.string().nullable().optional(),
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

export const workspaceSettings = z.object({
  maxParticipantsLimit: z.number(),
  allowGuestAccess: z.boolean(),
  defaultAiModel: z.string().nullable().optional(),
  audioRetentionDays: z.number(),
});
export type WorkspaceSettings = z.infer<typeof workspaceSettings>;

export const page = <T extends z.ZodType>(item: T) =>
  z.object({ items: z.array(item), page: z.number(), pageSize: z.number(), total: z.number() });

export const ROOM_STATUSES = [
  'SCHEDULED', 'WAITING', 'OPEN', 'IN_PROGRESS', 'PAUSED', 'ENDED', 'CANCELLED', 'EXPIRED', 'FAILED',
] as const;
export type RoomStatus = (typeof ROOM_STATUSES)[number];

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
