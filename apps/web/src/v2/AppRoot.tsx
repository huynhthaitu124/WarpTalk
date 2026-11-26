import { useState } from 'react';
import { BrowserRouter } from 'react-router-dom';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { App } from './App';
import { AuthProvider } from '@v2/lib/auth';
import { ThemeProvider } from '@v2/lib/theme';
import { ToastProvider, TooltipProvider } from '@v2/ui';
import { HarnessBridge } from './HarnessBridge';
import { OverlayPage } from '@v2/features/Overlay';

/** The native copilot window (`/overlay`) renders bare: no auth, no API calls. */
export function AppRoot() {
  if (typeof window !== 'undefined' && window.location.pathname.startsWith('/overlay')) return <OverlayPage />;
  return <MainRoot />;
}

function MainRoot() {
  const [queryClient] = useState(
    () => new QueryClient({ defaultOptions: { queries: { staleTime: 5_000, refetchOnWindowFocus: false, retry: 1 } } }),
  );
  return (
    <>
      <ThemeProvider>
        <QueryClientProvider client={queryClient}>
          <TooltipProvider>
            <ToastProvider>
              <BrowserRouter>
                <AuthProvider>
                  <App />
                </AuthProvider>
              </BrowserRouter>
            </ToastProvider>
          </TooltipProvider>
        </QueryClientProvider>
      </ThemeProvider>
      <HarnessBridge />
    </>
  );
}
