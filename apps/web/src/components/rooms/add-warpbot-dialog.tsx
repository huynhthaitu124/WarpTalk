"use client";

import { useTranslations } from "next-intl";
import { useState } from "react";
import { toast } from "sonner";

import { Dialog, DialogContent, DialogDescription, DialogTitle } from "@/components/ui/dialog";
import { useClaimExternalMeetingRoom } from "@/hooks/use-translationRooms";
import { getErrorMessage } from "@/lib/api/errors";
import { EXTERNAL_PROVIDER_LABEL, parseExternalMeetingLink } from "@/lib/meeting/external-meeting-link";
import { useRouter } from "@/lib/next-shim/navigation";
import { roomDetailPath } from "@/lib/workspace/workspace-routes";
import { useUIStore } from "@/stores/ui-store";
import { useWorkspaceStore } from "@/stores/workspace-store";
import { externalMeetingHref } from "@v2/lib/bridge";
import { getNative } from "@v2/lib/native";

const DEFAULT_LANGUAGES = ["en-US", "vi-VN"];

/**
 * "New meeting": paste a call link, WarpBot joins it. External Meeting is the only kind a room
 * can be created as, so the full form (title, languages, schedule, options) is gone from the
 * create path; editing an existing room still uses CreateRoomDialog. The link is optional: an
 * empty field starts a translation with no call attached.
 */
export function AddWarpBotDialog() {
  const t = useTranslations("rooms.create");
  const router = useRouter();
  const createOpen = useUIStore((s) => s.createRoomModalOpen);
  const editRoomId = useUIStore((s) => s.editRoomId);
  const setOpen = useUIStore((s) => s.setCreateRoomModalOpen);
  const workspaceId = useWorkspaceStore((s) => s.activeWorkspaceId);
  const workspaceSlug = useWorkspaceStore((s) => s.activeWorkspaceSlug);
  const claim = useClaimExternalMeetingRoom();

  const [link, setLink] = useState("");
  const [error, setError] = useState<string | null>(null);
  const open = createOpen && !editRoomId;

  function close(next: boolean) {
    setOpen(next);
    if (!next) {
      setLink("");
      setError(null);
    }
  }

  async function submit() {
    setError(null);
    const parsed = link.trim() ? parseExternalMeetingLink(link) : null;
    if (parsed && !parsed.ok) {
      setError(t(`externalLink.errors.${parsed.error}`));
      return;
    }
    if (!workspaceId) {
      setError(t("errors.selectWorkspace"));
      return;
    }
    const title = parsed?.ok
      ? t("quick.titleWithProvider", { provider: EXTERNAL_PROVIDER_LABEL[parsed.provider] })
      : t("quick.titleDefault");
    try {
      const res = await claim.mutateAsync({
        workspaceId,
        ...(parsed?.ok
          ? {
              provider: parsed.provider,
              ...(parsed.url ? { meetingUrl: parsed.url } : { code: parsed.code }),
            }
          : {}),
        title,
        sourceLanguage: DEFAULT_LANGUAGES[0],
        targetLanguages: DEFAULT_LANGUAGES,
      });
      toast.success(t(res.created ? "externalLink.created" : "externalLink.joined"));
      // macOS app: the copilot pill is the meeting UI, open it straight away.
      if (getNative()) {
        window.location.assign(
          externalMeetingHref({
            id: res.room.id,
            title: res.room.title,
            sourceLanguage: res.room.sourceLanguage ?? DEFAULT_LANGUAGES[0],
            targetLanguages: res.room.targetLanguages ?? DEFAULT_LANGUAGES,
          }),
        );
        return;
      }
      if (workspaceSlug) router.push(roomDetailPath(workspaceSlug, res.room.id));
      close(false);
    } catch (err) {
      setError(getErrorMessage(err, t("errors.createFailed")));
    }
  }

  return (
    <Dialog open={open} onOpenChange={close}>
      <DialogContent
        overlayClassName="!bg-black/40 !backdrop-blur-none"
        className="w-full max-w-[calc(100vw-2rem)] gap-0 rounded-2xl border-border/60 bg-surface-1 p-5 shadow-[0_8px_40px_-12px_rgba(0,0,0,0.3)] sm:max-w-[560px]"
      >
        <DialogTitle className="text-[15px] font-medium text-ink">{t("quick.title")}</DialogTitle>
        <DialogDescription className="sr-only">{t("srDescription")}</DialogDescription>
        <form
          className="mt-4"
          onSubmit={(e) => {
            e.preventDefault();
            void submit();
          }}
        >
          <label htmlFor="add-warpbot-link" className="text-[13px] text-ink">
            {t("quick.label")}
          </label>
          <div className="mt-2 flex items-center gap-2">
            <input
              id="add-warpbot-link"
              autoFocus
              value={link}
              onChange={(e) => {
                setLink(e.target.value);
                setError(null);
              }}
              placeholder={t("quick.placeholder")}
              aria-invalid={error ? true : undefined}
              className="h-9 min-w-0 flex-1 rounded-lg bg-surface-2 px-3 text-[13px] text-ink outline-none placeholder:text-ink-muted/70 focus:ring-1 focus:ring-border"
            />
            <button
              type="submit"
              disabled={claim.isPending}
              className="h-9 shrink-0 rounded-lg px-3 text-[13px] text-ink-muted transition-colors hover:bg-surface-2 hover:text-ink disabled:opacity-50"
            >
              {t("quick.submit")}
            </button>
          </div>
          {error ? (
            <p role="alert" className="mt-2 text-[12px] text-red-500">
              {error}
            </p>
          ) : null}
        </form>
      </DialogContent>
    </Dialog>
  );
}
