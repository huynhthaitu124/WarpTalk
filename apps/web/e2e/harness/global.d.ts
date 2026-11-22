// `window.__wtHarness` is installed by src/HarnessBridge.tsx when NEXT_PUBLIC_E2E_HARNESS=1.
export {};
declare global {
  interface Window {
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    __wtHarness?: Record<'captions' | 'meeting' | 'audio' | 'suggestions', () => Promise<any>>;
  }
}
