"use client";
/** `next/link` for the ported UI, on react-router (see ./navigation.ts). */
import { forwardRef, type AnchorHTMLAttributes, type ReactNode } from "react";
import { Link as RRLink } from "react-router-dom";

type UrlObject = { pathname?: string | null; query?: Record<string, string | number | boolean | string[] | undefined> | string | null; hash?: string | null };

export interface LinkProps extends Omit<AnchorHTMLAttributes<HTMLAnchorElement>, "href"> {
  href: string | UrlObject;
  replace?: boolean;
  scroll?: boolean;
  prefetch?: boolean | null | "auto";
  shallow?: boolean;
  passHref?: boolean;
  legacyBehavior?: boolean;
  locale?: string | false;
  children?: ReactNode;
}

function toHref(href: string | UrlObject): string {
  if (typeof href === "string") return href;
  let out = href.pathname ?? "";
  if (href.query) {
    if (typeof href.query === "string") out += `?${href.query}`;
    else {
      const q = new URLSearchParams();
      for (const [k, v] of Object.entries(href.query)) {
        if (v === undefined) continue;
        if (Array.isArray(v)) v.forEach((x) => q.append(k, x));
        else q.set(k, String(v));
      }
      const s = q.toString();
      if (s) out += `?${s}`;
    }
  }
  if (href.hash) out += href.hash.startsWith("#") ? href.hash : `#${href.hash}`;
  return out;
}

const Link = forwardRef<HTMLAnchorElement, LinkProps>(function Link(
  { href, replace, scroll: _scroll, prefetch: _prefetch, shallow: _shallow, passHref: _passHref, legacyBehavior: _legacy, locale: _locale, target, ...rest },
  ref,
) {
  const to = toHref(href);
  if (/^([a-z][a-z0-9+.-]*:|\/\/|#)/i.test(to) || target === "_blank" || rest.download !== undefined) {
    return <a ref={ref} href={to} target={target} {...rest} />;
  }
  return <RRLink ref={ref} to={to} replace={replace} target={target} {...rest} />;
});

export default Link;
