// The transcript consumer (`warptalk-worker`) is not part of the e2e backend; specs that need segments persisted start one.
import { spawn, type ChildProcess } from 'node:child_process';
import { existsSync } from 'node:fs';
import path from 'node:path';

const BIN = path.resolve(process.cwd(), '../../target/debug/warptalk-worker');
export const workerAvailable = () => existsSync(BIN);

export async function startWorker(): Promise<() => void> {
  const child: ChildProcess = spawn(BIN, [], {
    stdio: 'ignore',
    env: { ...process.env, DATABASE_URL: process.env.DATABASE_URL ?? 'postgres://postgres@localhost:5432/warptalk', REDIS_URL: process.env.E2E_REDIS ?? 'redis://localhost:6379', RUST_LOG: 'warn' },
  });
  await new Promise((r) => setTimeout(r, 1500));
  return () => { child.kill('SIGTERM'); };
}
