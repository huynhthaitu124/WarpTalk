export const LANGUAGES = [
  { code: 'vi', label: 'Vietnamese' },
  { code: 'en', label: 'English' },
  { code: 'ja', label: 'Japanese' },
] as const;
export const languageLabel = (code: string) => LANGUAGES.find((l) => l.code === code)?.label ?? code.toUpperCase();

/**
 * The language the user reads captions in when nothing more specific is known: the browser
 * locale if it is one we offer, else English. Never a hardcoded product language.
 */
export function defaultReadingLanguage(): string {
  const locales = typeof navigator === 'undefined' ? [] : [...(navigator.languages ?? []), navigator.language];
  for (const l of locales) {
    const code = l?.toLowerCase().split(/[-_]/)[0];
    if (code && LANGUAGES.some((x) => x.code === code)) return code;
  }
  return 'en';
}
