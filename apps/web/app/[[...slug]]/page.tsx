import { ClientRoot } from '@/ClientRoot';

/**
 * The whole product is a client-rendered SPA (react-router inside one catch-all page): see ClientRoot.
 * `output: 'export'` emits only `index.html` for it; the macOS bundle server and any static host answer
 * unknown extensionless paths (`/rooms/<id>`, `/<workspaceSlug>/home`) with that file.
 */
export function generateStaticParams() {
  return [{ slug: [] }];
}

export default function Page() {
  return <ClientRoot />;
}
