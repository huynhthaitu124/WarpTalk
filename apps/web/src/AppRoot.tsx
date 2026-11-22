import { useState } from 'react';
import { BrowserRouter } from 'react-router-dom';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { App } from './App';
import { AuthProvider } from '@/lib/auth';
import { ThemeProvider } from '@/lib/theme';
import { ToastProvider, TooltipProvider } from '@/ui';
import { HarnessBridge } from './HarnessBridge';

export function AppRoot() {
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
