import { LegacyRoot } from '@/LegacyRoot';

/**
 * The whole product UI (ported from warptalk-web) is a client-rendered SPA: react-router inside this one
 * catch-all page, see src/LegacyRoot.tsx. `output: 'export'` emits only `index.html` for it; the macOS bundle
 * server and any static host answer unknown extensionless paths with that file.
 */
export function generateStaticParams() {
  return [{ slug: [] }];
}

export default function Page() {
  return <LegacyRoot />;
}
