// Phone Cues tab — quick-cue button grid + collapsible type-to-speak.
// Big tap targets; the speaking button pulses; Stop pins to the bottom only
// while live.

import { useState } from "react";
import type { NowPlaying } from "../../shared/types";
import { post, type Info } from "../api";
import { ChevDownIcon, ChevUpIcon, PowerIcon } from "./icons";

interface Props {
  info: Info | null;
  now: NowPlaying;
}

export function CuesTab({ info, now }: Props) {
  const [typingOpen, setTypingOpen] = useState(false);
  const [draft, setDraft] = useState("");
  const cues = info?.cues_quick ?? [];
  const speaking = !!now.cue?.speaking;
  const speakingLabel = now.cue?.label ?? null;

  return (
    <section className="tab-body" role="tabpanel" aria-label="Cues">
      {cues.length === 0 ? (
        <div className="cue-empty">
          <p>No saved cues yet.</p>
          <p className="cue-empty-sub">
            Add quick cues in the desktop app — they'll show up here as big tap buttons.
          </p>
        </div>
      ) : (
        <div className="cue-grid">
          {cues.map((c) => {
            const active = speaking && speakingLabel === c.label;
            return (
              <button
                key={c.id}
                type="button"
                className={`cue-btn${active ? " active" : ""}`}
                onClick={() => post(`/api/cue/quick/${encodeURIComponent(c.id)}`)}
              >
                <span className="cue-btn-label">{c.label}</span>
                {active && <span className="cue-btn-dot" />}
              </button>
            );
          })}
        </div>
      )}

      <div className={`cue-typing${typingOpen ? " open" : ""}`}>
        <button
          type="button"
          className="cue-typing-head"
          onClick={() => setTypingOpen((v) => !v)}
        >
          <span>Type to speak…</span>
          {typingOpen ? <ChevDownIcon /> : <ChevUpIcon />}
        </button>
        {typingOpen && (
          <div className="cue-typing-body">
            <textarea
              className="cue-typing-input"
              rows={2}
              placeholder="What should the PC say?"
              value={draft}
              onChange={(e) => setDraft(e.target.value)}
            />
            <button
              type="button"
              className="transport idle cue-speak-btn"
              disabled={!draft.trim()}
              onClick={() => {
                const text = draft.trim();
                if (!text) return;
                post("/api/cue/speak", { text });
                setDraft("");
              }}
            >
              Speak
            </button>
          </div>
        )}
      </div>

      {speaking ? (
        <button
          type="button"
          className="transport live"
          onClick={() => post("/api/cue/stop")}
        >
          <PowerIcon />
          Stop cue
        </button>
      ) : (
        <button type="button" className="transport idle" disabled>
          Tap a cue to speak
        </button>
      )}
      <p className="hint">{speakingLabel ? `Speaking: ${speakingLabel}` : ""}</p>
    </section>
  );
}
