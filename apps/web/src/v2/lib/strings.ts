// User-facing copy for the assistant, suggestion feedback, tasks, quality and document-reindex features.
// Kept in one place so wording can be reviewed (and later translated) without touching components.

export const ASK = {
  title: 'Ask AI',
  description: 'Ask about this meeting. Answers cite the transcript and your documents.',
  inputLabel: 'Question for the AI assistant',
  placeholder: 'What did we decide about the launch date?',
  send: 'Ask',
  stop: 'Stop',
  scopeLabel: 'Search in',
  scopes: {
    meeting: 'This meeting',
    'meeting+docs': 'Meeting and documents',
    workspace: 'Whole workspace',
  },
  recapGroup: 'Quick recap',
  recap: (min: number | null) => (min === null ? 'Recap everything' : `Recap ${min} min`),
  recapUser: (min: number | null) => (min === null ? 'Recap of the whole meeting' : `Recap of the last ${min} minutes`),
  empty: 'No questions yet. Ask something or request a recap.',
  historyLoading: 'Loading earlier questions',
  loadEarlier: 'Load earlier',
  historyError: 'Could not load earlier questions.',
  you: 'You',
  assistant: 'WarpBot',
  thinking: 'Thinking…',
  cached: 'From a recent recap',
  incomplete: 'The answer was cut off.',
  sources: 'Sources',
  openDocument: (label: string) => `Open document ${label}`,
  jumpToLine: (label: string) => `Show transcript line: ${label}`,
  docCitationNoTime: 'Document',
  tooLong: (max: number) => `Questions are limited to ${max} characters.`,
  errors: {
    EXTERNAL_LLM_NOT_ALLOWED: 'This workspace does not allow sending meeting content to an external AI provider. A workspace admin can change this in Settings, AI providers.',
    ASSISTANT_CHAT_DISABLED: 'The AI assistant is turned off for this workspace.',
    LLM_NOT_CONFIGURED: 'No AI provider is configured for this workspace. A workspace admin can add one in Settings, AI providers.',
    RATE_LIMITED: 'You are asking too quickly. Wait a moment and try again.',
    QUESTION_EMPTY: 'Type a question first.',
    QUESTION_TOO_LONG: 'That question is too long. Shorten it and try again.',
    INVALID_RECAP_WINDOW: 'That recap window is not available. Pick another one.',
    INVALID_SINCE_MS: 'The start of the time range is not valid.',
    ANSWER_BLOCKED: 'The answer was withheld because it did not pass the safety checks. Try rephrasing your question.',
    LLM_UNAVAILABLE: 'The AI provider is not reachable right now. Try again in a moment.',
    LLM_FAILED: 'The AI provider failed to answer. Try again.',
    LLM_EMPTY: 'The AI provider returned an empty answer. Try again.',
    FORBIDDEN: 'You are not allowed to use the assistant in this meeting.',
    NOT_FOUND: 'This meeting was not found.',
    NETWORK: 'The connection was lost before the answer finished. Check your network and try again.',
  } as Record<string, string>,
} as const;

export const FEEDBACK = {
  group: 'Rate this suggestion',
  used: 'Used it',
  helpful: 'Helpful',
  notHelpful: 'Not helpful',
  wrong: 'Wrong',
  commentLabel: 'What was wrong? (optional)',
  commentPlaceholder: 'A short note helps us improve suggestions',
  send: 'Send',
  skip: 'Skip',
  thanks: 'Thanks for the feedback',
  failed: 'Could not send your feedback. Try again.',
  sent: (label: string) => `You marked this: ${label}`,
} as const;

export const CATEGORY_LABELS: Record<string, string> = {
  answer: 'Suggested answer',
};

export const TASKS = {
  title: 'My tasks',
  description: 'Action items assigned to you across your workspaces.',
  open: 'Open',
  done: 'Done',
  filterLabel: 'Task status',
  emptyOpen: 'No open tasks',
  emptyOpenHint: 'Action items from your meetings that are assigned to you appear here.',
  emptyDone: 'No completed tasks yet',
  emptyDoneHint: 'Tasks you finish show up here.',
  loadError: 'Could not load your tasks',
  fromMeeting: 'From meeting',
  openMeeting: (title: string) => `Open source meeting: ${title}`,
  nav: 'My tasks',
  workspace: 'Workspace',
} as const;

export const PRIOR = {
  title: 'From previous meetings',
  openItems: 'Still open from earlier',
  lastSummaries: 'Last time',
  empty: 'Nothing carried over from earlier meetings.',
  loadError: 'Could not load context from previous meetings.',
  statusDone: 'Done',
  statusOpen: 'Still open',
  previousItems: 'Follow-up on earlier action items',
  sameSeries: 'Same series',
} as const;

export const QUALITY = {
  title: 'Suggestion quality',
  description: 'How people rate live suggestions in this workspace.',
  from: 'From',
  to: 'To',
  loadError: 'Could not load suggestion quality',
  restricted: 'Only workspace owners and admins can see suggestion quality.',
  empty: 'No suggestions were shown in this period.',
  invalidRange: 'The start date must be before the end date.',
  nav: 'Suggestion quality',
  columns: {
    category: 'Category',
    shown: 'Shown',
    used: 'Used',
    dismissed: 'Dismissed',
    helpful: 'Helpful',
    notHelpful: 'Not helpful',
    wrong: 'Wrong',
    usedRate: 'Used rate',
    wrongRate: 'Wrong rate',
  },
  total: 'All categories',
} as const;

export const DOCS = {
  skipReasons: {
    ocr_unavailable: { label: 'No OCR', hint: 'This is an image or scanned file and text recognition (OCR) is not available on this server, so the AI assistant cannot read it.' },
    unsupported_type: { label: 'Unsupported type', hint: 'This file type cannot be indexed for the AI assistant. You can still download it.' },
    invalid_encoding: { label: 'Unreadable text', hint: 'The file is not valid text (wrong encoding), so it could not be indexed. Re-save it as UTF-8 and upload it again.' },
    external_llm_disallowed: { label: 'External AI off', hint: 'This workspace does not allow sending content to an external AI provider, so the document was not indexed.' },
  } as Record<string, { label: string; hint: string }>,
  reindex: 'Reindex',
  reindexing: 'Reindex queued',
  reindexQueued: 'The document is queued for indexing.',
  reindexAlready: 'The document is already waiting to be indexed.',
  reindexFailed: 'Could not reindex the document',
  reindexSkipped: 'Reindex skipped',
  reindexSkippedHint: 'Queue every skipped document in this workspace for indexing again.',
  bulkResultTitle: 'Reindex requested',
  bulkNothing: 'No skipped documents needed reindexing.',
  bulkFailed: 'Could not start the bulk reindex',
  imageNote: 'Images (PNG, JPG, WEBP) are accepted. They are indexed only when OCR is available.',
} as const;

export const STT = {
  badge: 'Server STT',
  hint: 'Speech recognition for this channel runs on the server because direct recognition was not available.',
  recovered: 'Direct speech recognition is back.',
} as const;
