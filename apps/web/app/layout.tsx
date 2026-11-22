import type { Metadata, Viewport } from 'next';
import type { ReactNode } from 'react';
import '@/styles/index.css';

export const metadata: Metadata = {
  title: 'WarpTalk',
  icons: {
    icon: [
      { url: '/favicon-32.png', sizes: '32x32', type: 'image/png' },
      { url: '/favicon.png', sizes: '192x192', type: 'image/png' },
    ],
    apple: '/favicon.png',
  },
};

export const viewport: Viewport = { width: 'device-width', initialScale: 1, colorScheme: 'dark light' };

/** Applies the persisted theme before first paint (see DESIGN_SYSTEM.md, principle 4). */
const THEME_SCRIPT = `try{var t=localStorage.getItem('wt.theme');if(t==='light'||t==='dark')document.documentElement.dataset.theme=t;}catch(e){}`;

export default function RootLayout({ children }: { children: ReactNode }) {
  return (
    // The theme script changes `data-theme` before hydration, hence suppressHydrationWarning.
    <html lang="en" data-theme="dark" suppressHydrationWarning>
      <head>
        <script dangerouslySetInnerHTML={{ __html: THEME_SCRIPT }} />
      </head>
      <body>
        <div id="root">{children}</div>
      </body>
    </html>
  );
}
