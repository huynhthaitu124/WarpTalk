export const LANGUAGES = [
  { code: 'vi', label: 'Vietnamese' },
  { code: 'en', label: 'English' },
  { code: 'ja', label: 'Japanese' },
] as const;
export const languageLabel = (code: string) => LANGUAGES.find((l) => l.code === code)?.label ?? code.toUpperCase();
