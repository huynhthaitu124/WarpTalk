"use client";
/** Adapters that render app-router `page.tsx` / `layout.tsx` modules as react-router elements. */
import { Suspense, type ComponentType, type LazyExoticComponent, type ReactNode } from "react";
import { Outlet, useParams, useSearchParams } from "react-router-dom";

// Next passes `params`/`searchParams` as promises; they must be stable across renders for `use()`.
const promises = new Map<string, Promise<unknown>>();
function stable<T>(value: T): Promise<T> {
  const key = JSON.stringify(value);
  let p = promises.get(key);
  if (!p) {
    p = Promise.resolve(value);
    if (promises.size > 500) promises.clear();
    promises.set(key, p);
  }
  return p as Promise<T>;
}

type AnyComponent = LazyExoticComponent<ComponentType<any>>; // eslint-disable-line @typescript-eslint/no-explicit-any

function Fallback(): ReactNode {
  return null;
}

export function Page({ c: C }: { c: AnyComponent }) {
  const params = useParams();
  const [search] = useSearchParams();
  return (
    <Suspense fallback={<Fallback />}>
      <C params={stable(params)} searchParams={stable(Object.fromEntries(search))} />
    </Suspense>
  );
}

export function Layout({ c: C }: { c: AnyComponent }) {
  const params = useParams();
  return (
    <Suspense fallback={<Fallback />}>
      <C params={stable(params)}>
        <Outlet />
      </C>
    </Suspense>
  );
}
