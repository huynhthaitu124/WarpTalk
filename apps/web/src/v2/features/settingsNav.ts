import { BookBookmark, Brain, GearSix, SlidersHorizontal, User } from '@phosphor-icons/react';

export const SETTINGS_SECTIONS = [
  { id: 'account', label: 'Account', icon: User, group: 'Personal', admin: false },
  { id: 'general', label: 'General', icon: GearSix, group: 'Workspace', admin: false },
  { id: 'ai', label: 'AI providers', icon: Brain, group: 'Workspace', admin: true },
  { id: 'advanced', label: 'Advanced', icon: SlidersHorizontal, group: 'Workspace', admin: true, system: false },
  /** Platform-wide: only for system administrators, listed right under Advanced. */
  { id: 'global-glossary', label: 'Global glossary', icon: BookBookmark, group: 'Workspace', admin: false, system: true },
] as const;

/** Sections the person may see (owner/admin-only and system-admin-only ones are filtered). */
export function visibleSettingsSections(admin: boolean, sysAdmin: boolean) {
  return SETTINGS_SECTIONS.filter((s) => (!s.admin || admin) && (!('system' in s) || !s.system || sysAdmin));
}
