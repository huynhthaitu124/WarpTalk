"use client";

import { use } from "react";

import { SharedMinutesView } from "@/components/rooms/shared-minutes-view";

/** A meeting record opened from a share link: outside the app shell, readable without an account. */
export default function SharedMinutesPage({ params }: { params: Promise<{ token: string }> }) {
  const { token } = use(params);
  return <SharedMinutesView token={token} />;
}
