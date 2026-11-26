"use client";

/**
 * Dev test mode: type the lines of a scripted conversation and they go into the meeting as real
 * final transcript segments (`POST /meetings/{id}/dev/transcript`), so the real suggestion engine
 * answers from the API's test script. Rendered only when the API reports test mode, which it
 * refuses outside a debug build — production never shows this box.
 */
import { useEffect, useState, type FormEvent } from "react";
import { useAuthStore } from "@/stores/auth-store";
import { sameUserId, useTranslationRoomStore } from "@/stores/translationRoom-store";
import { sendTestTranscriptLine, suggestionTestModeEnabled } from "@/lib/realtime/meeting-suggestions";
import type { TranscriptSegmentDto } from "@/types/realtime";

export function TestScriptBox({ roomId }: { roomId: string }) {
  const [enabled, setEnabled] = useState(false);
  const [text, setText] = useState("");
  const [speaker, setSpeaker] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const addTranscriptSegment = useTranslationRoomStore((s) => s.addTranscriptSegment);
  const selfName = useAuthStore((s) => s.user?.fullName ?? s.user?.email ?? "");
  const selfId = useAuthStore((s) => s.user?.id);
  // The line is tagged with the language this person speaks in the room (the API defaults to English).
  const speakLanguage = useTranslationRoomStore((s) => s.participants.find((p) => sameUserId(p.userId, selfId))?.speakLanguage);

  useEffect(() => {
    let live = true;
    void suggestionTestModeEnabled().then((on) => live && setEnabled(on));
    return () => {
      live = false;
    };
  }, []);

  if (!enabled) return null;

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    const line = text.trim();
    if (!line || busy) return;
    setBusy(true);
    setError(null);
    try {
      const who = speaker.trim();
      const r = await sendTestTranscriptLine(roomId, {
        text: line,
        language: speakLanguage || undefined,
        speaker: who || undefined,
      });
      // The typed line does not travel through the audio pipeline's captions, so show it here;
      // the suggestion the engine makes for it arrives over the SSE stream with this segment id.
      addTranscriptSegment({
        segmentId: r.segmentId,
        speakerId: r.speakerId,
        speakerName: r.speakerName || who || selfName,
        originalText: r.text,
        originalLanguage: r.language,
        translations: {},
        confidence: 1,
        startTimeMs: r.timestampMs,
        endTimeMs: r.timestampMs,
        receivedAt: Date.now(),
        sequenceOrder: r.timestampMs,
      } as TranscriptSegmentDto);
      setText("");
    } catch {
      setError("Không gửi được dòng test");
    } finally {
      setBusy(false);
    }
  };

  return (
    <form
      onSubmit={submit}
      className="shrink-0 border-t border-dashed border-amber-400/60 bg-amber-50/60 px-3 py-2 dark:bg-amber-500/5"
      aria-label="Test script input"
    >
      <p className="mb-1.5 text-[10px] font-semibold uppercase tracking-wide text-amber-700 dark:text-amber-400">
        Test mode · kịch bản suggestion
      </p>
      <div className="flex gap-1.5">
        <input
          value={speaker}
          onChange={(e) => setSpeaker(e.target.value)}
          placeholder="Người nói"
          aria-label="Speaker name"
          className="w-20 shrink-0 rounded-md border border-border bg-surface-2 px-2 py-1 text-[12px] text-ink outline-none focus:border-amber-500"
        />
        <input
          value={text}
          onChange={(e) => setText(e.target.value)}
          placeholder="Gõ câu thoại rồi Enter…"
          aria-label="Transcript line"
          maxLength={1000}
          className="min-w-0 flex-1 rounded-md border border-border bg-surface-2 px-2 py-1 text-[12px] text-ink outline-none focus:border-amber-500"
        />
        <button
          type="submit"
          disabled={busy || !text.trim()}
          className="shrink-0 rounded-md bg-amber-500 px-2.5 py-1 text-[12px] font-medium text-white disabled:opacity-50"
        >
          Gửi
        </button>
      </div>
      {error ? <p className="mt-1 text-[11px] text-red-600">{error}</p> : null}
    </form>
  );
}
