"use client";

import { useTranslations } from "next-intl";
import { Check, ChevronDown, X } from "lucide-react";
import { useMemo, useState } from "react";
import { toast } from "sonner";

import { Dialog, DialogContent, DialogDescription, DialogTitle } from "@/components/ui/dialog";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { useClaimExternalMeetingRoom } from "@/hooks/use-translationRooms";
import { useUserSettings } from "@/hooks/use-user-settings";
import { getLanguageName, languagesInScope, normalizeLanguageCode } from "@/lib/language/languages";
import { getErrorMessage } from "@/lib/api/errors";
import { EXTERNAL_PROVIDER_LABEL, parseExternalMeetingLink } from "@/lib/meeting/external-meeting-link";
import { useRouter } from "@/lib/next-shim/navigation";
import { roomDetailPath } from "@/lib/workspace/workspace-routes";
import { useUIStore } from "@/stores/ui-store";
import { useWorkspaceStore } from "@/stores/workspace-store";
import { externalMeetingHref } from "@v2/lib/bridge";
import { getNative } from "@v2/lib/native";

// "auto": the pipeline detects what each speaker says (no fixed hint). The languages the user
// picks are stored as the room's target languages: routes are planned per listener, so extra
// room languages cost no translation, but the pipeline hands them to STT as the meeting's
// languages (code-switch hints, prompt, and the guard against stray detections). Bare ISO-639
// codes, the same ones the pill's language picker sends.
const SOURCE_LANGUAGE = "auto";
const STORAGE_KEY = "warptalk.addWarpBot.languages";

function storedLanguages(): string[] | null {
  try {
    const raw = window.localStorage.getItem(STORAGE_KEY);
    const parsed: unknown = raw ? JSON.parse(raw) : null;
    return Array.isArray(parsed) && parsed.every((c) => typeof c === "string") && parsed.length
      ? parsed
      : null;
  } catch {
    return null;
  }
}

function rememberLanguages(codes: string[]) {
  try {
    window.localStorage.setItem(STORAGE_KEY, JSON.stringify(codes));
  } catch {
    // Best effort: the picker still works, it just forgets the choice.
  }
}

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

  const settings = useUserSettings();
  const offered = useMemo(() => languagesInScope("meeting").map((l) => l.code), []);

  const [link, setLink] = useState("");
  const [error, setError] = useState<string | null>(null);
  // null = untouched: follow the remembered choice, else the user's own language plus English.
  const [picked, setPicked] = useState<string[] | null>(null);
  const open = createOpen && !editRoomId;

  // The language the creator reads captions in: their profile's listen language, else the
  // browser's. Never a hardcoded one.
  const ownLanguage = useMemo(() => {
    const candidates = [
      settings.data?.defaultListenLanguage,
      typeof navigator !== "undefined" ? navigator.language : undefined,
    ].map((c) => normalizeLanguageCode(c));
    return candidates.find((c) => offered.includes(c)) ?? "en";
  }, [settings.data?.defaultListenLanguage, offered]);

  const languages = useMemo(() => {
    const base = picked ?? storedLanguages() ?? [ownLanguage, "en"];
    const valid = base.map((c) => normalizeLanguageCode(c)).filter((c) => offered.includes(c));
    return Array.from(new Set(valid.length ? valid : [ownLanguage]));
  }, [picked, ownLanguage, offered]);
  const listenLanguage = languages.includes(ownLanguage) ? ownLanguage : languages[0];

  function toggleLanguage(code: string) {
    const next = languages.includes(code) ? languages.filter((c) => c !== code) : [...languages, code];
    if (next.length) setPicked(next);
  }

  function close(next: boolean) {
    setOpen(next);
    if (!next) {
      setLink("");
      setError(null);
      setPicked(null);
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
    // "Google Meet meeting · Thu, Nov 27, 10:15" so each meeting in the list is distinguishable;
    // the user can rename it inline on the meeting page.
    const when = new Date().toLocaleString(undefined, { weekday: "short", month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" });
    const title = `${
      parsed?.ok
        ? t("quick.titleWithProvider", { provider: EXTERNAL_PROVIDER_LABEL[parsed.provider] })
        : t("quick.titleDefault")
    } · ${when}`;
    rememberLanguages(languages);
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
        sourceLanguage: SOURCE_LANGUAGE,
        targetLanguages: languages,
        listenLanguage,
      });
      toast.success(t(res.created ? "externalLink.created" : "externalLink.joined"));
      // macOS app: the copilot pill is the meeting UI, open it straight away.
      if (getNative()) {
        window.location.assign(
          externalMeetingHref({
            id: res.room.id,
            title: res.room.title,
            sourceLanguage: res.room.sourceLanguage ?? SOURCE_LANGUAGE,
            targetLanguages: res.room.targetLanguages ?? languages,
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
          <div className="mt-4">
            <span className="text-[13px] text-ink">{t("quick.languagesLabel")}</span>
            <div className="mt-2 flex flex-wrap items-center gap-1.5">
              {languages.map((code) => (
                <span
                  key={code}
                  className="inline-flex h-7 items-center gap-1 rounded-full bg-surface-2 pl-2.5 pr-1 text-[12px] text-ink"
                >
                  {getLanguageName(code)}
                  {languages.length > 1 ? (
                    <button
                      type="button"
                      onClick={() => toggleLanguage(code)}
                      aria-label={t("quick.removeLanguage", { language: getLanguageName(code) })}
                      className="grid size-5 place-items-center rounded-full text-ink-muted hover:bg-surface-1 hover:text-ink"
                    >
                      <X className="size-3" />
                    </button>
                  ) : (
                    <span className="w-1" />
                  )}
                </span>
              ))}
              <DropdownMenu>
                <DropdownMenuTrigger
                  className="inline-flex h-7 items-center gap-1 rounded-full border border-dashed border-border px-2.5 text-[12px] text-ink-muted hover:text-ink"
                >
                  {t("quick.addLanguage")}
                  <ChevronDown className="size-3" />
                </DropdownMenuTrigger>
                <DropdownMenuContent align="start" className="min-w-44">
                  {offered.map((code) => (
                    <DropdownMenuItem
                      key={code}
                      closeOnClick={false}
                      onClick={() => toggleLanguage(code)}
                      className="flex items-center justify-between gap-3 text-[13px]"
                    >
                      {getLanguageName(code)}
                      {languages.includes(code) ? <Check className="size-3.5" /> : null}
                    </DropdownMenuItem>
                  ))}
                </DropdownMenuContent>
              </DropdownMenu>
            </div>
            <p className="mt-1.5 text-[12px] text-ink-muted">{t("quick.languagesHint")}</p>
          </div>
          <p className="mt-3 text-[12px] leading-relaxed text-ink-muted">{t("quick.hint")}</p>
        </form>
      </DialogContent>
    </Dialog>
  );
}
