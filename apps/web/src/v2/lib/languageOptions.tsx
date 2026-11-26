import { Flag, type SelectOption } from '@v2/ui';
import { LANGUAGES, languageLabel } from './languages';

/** Select options for language codes, each with its flag. */
export const languageOptions = (codes: readonly string[] = LANGUAGES.map((l) => l.code)): SelectOption[] =>
  codes.map((c) => ({ value: c, label: languageLabel(c), icon: <Flag code={c} /> }));
