// Phone Cues tab — quick-cue button grid + add/edit/delete + collapsible type-to-speak.

import { useState } from "react";
import type { NowPlaying } from "../../shared/types";
import { cueAdd, cueRemove, cueUpdate, post, type Info, type QuickCue } from "../api";
import { ChevDownIcon, ChevUpIcon, PencilIcon, PlusIcon, PowerIcon } from "./icons";

interface Props {
  info: Info | null;
  now: NowPlaying;
  refreshInfo: () => Promise<void>;
}

interface SheetState {
  id: string | null; // null = new cue
  label: string;
  text: string;
}

export function CuesTab({ info, now, refreshInfo }: Props) {
  const [typingOpen, setTypingOpen] = useState(false);
  const [draft, setDraft] = useState("");
  const [sheet, setSheet] = useState<SheetState | null>(null);
  const [saving, setSaving] = useState(false);

  const cues = info?.cues_quick ?? [];
  const speaking = !!now.cue?.speaking;
  const speakingLabel = now.cue?.label ?? null;

  function openNew() {
    setSheet({ id: null, label: "", text: "" });
  }
  function openEdit(c: QuickCue) {
    setSheet({ id: c.id, label: c.label, text: c.text });
  }

  async function handleSave() {
    if (!sheet || !sheet.label.trim()) return;
    setSaving(true);
    try {
      if (sheet.id) {
        await cueUpdate(sheet.id, sheet.label.trim(), sheet.text);
      } else {
        await cueAdd(sheet.label.trim(), sheet.text);
      }
      await refreshInfo();
      setSheet(null);
    } catch {
      /* server error — keep sheet open so user can retry */
    } finally {
      setSaving(false);
    }
  }

  async function handleDelete() {
    if (!sheet?.id) return;
    setSaving(true);
    try {
      await cueRemove(sheet.id);
      await refreshInfo();
      setSheet(null);
    } catch {
      /* ignore */
    } finally {
      setSaving(false);
    }
  }

  return (
    <section className="tab-body" role="tabpanel" aria-label="Cues">
      {cues.length === 0 ? (
        <div className="cue-empty">
          <p>No saved cues yet.</p>
          <p className="cue-empty-sub">Tap below to add your first quick cue.</p>
          <button type="button" className="cue-empty-add" onClick={openNew}>
            <PlusIcon />
            New cue
          </button>
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
                <span
                  role="button"
                  tabIndex={0}
                  className="cue-btn-edit"
                  aria-label={`Edit ${c.label}`}
                  onClick={(e) => {
                    e.stopPropagation();
                    openEdit(c);
                  }}
                  onKeyDown={(e) => {
                    if (e.key === "Enter" || e.key === " ") {
                      e.stopPropagation();
                      openEdit(c);
                    }
                  }}
                >
                  <PencilIcon />
                </span>
              </button>
            );
          })}
          <button type="button" className="cue-btn cue-btn-new" onClick={openNew}>
            <PlusIcon />
            <span className="cue-btn-label">New cue</span>
          </button>
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

      {/* Bottom sheet for add / edit */}
      {sheet !== null && (
        <div className="cue-sheet-overlay" onClick={() => !saving && setSheet(null)}>
          <div className="cue-sheet" onClick={(e) => e.stopPropagation()}>
            <p className="cue-sheet-title">{sheet.id ? "Edit cue" : "New cue"}</p>

            <div className="cue-sheet-field">
              <span className="cue-sheet-lbl">Label</span>
              <input
                className="cue-sheet-input"
                type="text"
                placeholder="e.g. Verse 2"
                value={sheet.label}
                onChange={(e) =>
                  setSheet((s) => s && { ...s, label: e.target.value })
                }
                autoFocus
              />
            </div>

            <div className="cue-sheet-field">
              <span className="cue-sheet-lbl">Text to speak</span>
              <textarea
                className="cue-sheet-textarea"
                rows={3}
                placeholder="Words the PC will say…"
                value={sheet.text}
                onChange={(e) =>
                  setSheet((s) => s && { ...s, text: e.target.value })
                }
              />
            </div>

            <div className="cue-sheet-actions">
              <button
                type="button"
                className="sheet-btn sheet-btn-primary"
                onClick={handleSave}
                disabled={saving || !sheet.label.trim()}
              >
                {saving ? "Saving…" : "Save"}
              </button>
              <button
                type="button"
                className="sheet-btn sheet-btn-cancel"
                onClick={() => setSheet(null)}
                disabled={saving}
              >
                Cancel
              </button>
              {sheet.id && (
                <button
                  type="button"
                  className="sheet-btn sheet-btn-delete"
                  onClick={handleDelete}
                  disabled={saving}
                >
                  Delete
                </button>
              )}
            </div>
          </div>
        </div>
      )}
    </section>
  );
}
