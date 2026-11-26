import type { Metadata, Viewport } from 'next';
import type { ReactNode } from 'react';
import '@/screens/globals.css';

export const metadata: Metadata = {
  title: { default: 'WarpTalk — AI-Powered Meeting Translation', template: '%s | WarpTalk' },
  description: 'Real-time multilingual meeting translation and transcription platform for global teams.',
  icons: { icon: [{ url: '/favicon-32.png', sizes: '32x32', type: 'image/png' }, { url: '/favicon.png', sizes: '192x192', type: 'image/png' }], apple: '/favicon.png' },
};

const THEME_BOOTSTRAP = `(function(){try{var t=localStorage.getItem('theme');var d=t==='dark'||((!t||t==='system')&&matchMedia('(prefers-color-scheme: dark)').matches);var e=document.documentElement;e.classList.toggle('dark',d);e.style.colorScheme=d?'dark':'light';}catch(_){}})();`;

export const viewport: Viewport = { width: 'device-width', initialScale: 1 };

export default function RootLayout({ children }: { children: ReactNode }) {
  return (
    <html lang="en" suppressHydrationWarning>
      <head>
        {/* Applies the saved theme before first paint (same storage key and values as next-themes). */}
        <script dangerouslySetInnerHTML={{ __html: THEME_BOOTSTRAP }} />
      </head>
      <body className="min-h-screen bg-canvas text-ink font-sans antialiased" suppressHydrationWarning>
        {children}
      </body>
    </html>
  );
}
