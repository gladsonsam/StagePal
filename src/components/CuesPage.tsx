// Cues page — quick-cue editor + free-form speak. Editing lives on desktop so
// phones can't rewrite labels mid-set.

import { useEffect, useRef, useState } from "react";
import {
  cueAdd,
  cueRemove,
  cueSpeak,
  cueSpeakQuick,
  cueStop,
  cueUpdate,
  listVoices,
  setCueRate,
  setCueSpeakKey,
  setCueVoice,
  setCueVolume,
  type QuickCue,
  type Settings,
  type VoiceInfo,
} from "../lib/ipc";
import { Card, Eyebrow, Icon, SelectField, Slider, Volume } from "./ui";

interface Props {
  settings: Settings;
  /** Label being spoken, or null. Drives the "Speaking" banner. */
  speakingLabel: string | null;
  speaking: boolean;
  guard: (fn: () => Promise<unknown>) => Promise<void>;
  refreshSettings: () => Promise<void>;
}

export function CuesPage({
  settings,
  speakingLabel,
  speaking,
  guard,
  refreshSettings,
}: Props) {
  const [voices, setVoices] = useState<VoiceInfo[]>([]);
  const [draft, setDraft] = useState("");

  useEffect(() => {
    listVoices()
      .then(setVoices)
      .catch(() => setVoices([]));
  }, []);

  const cues = settings.cues.quick;
  const voiceOptions = [
    { value: "", label: "System default" },
    ...voices.map((v) => ({ value: v.id, label: v.name })),
  ];

  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 22 }}>
      <Card pad={24}>
        <div style={{ display: "flex", alignItems: "center", gap: 9, marginBottom: 16 }}>
          <Icon name="mic" size={18} stroke="var(--accent-ink)" />
          <Eyebrow style={{ letterSpacing: "0.1em" }}>Voice</Eyebrow>
        </div>

        <div style={{ display: "grid", gridTemplateColumns: "minmax(220px, 1.4fr) 1fr 1fr", gap: 14 }}>
          <SelectField
            label="Voice"
            value={settings.cues.voice ?? ""}
            options={voiceOptions}
            onChange={(v) =>
              guard(async () => {
                await setCueVoice(v || null);
                await refreshSettings();
              })
            }
          />
          <div style={{ display: "flex", flexDirection: "column", gap: 7 }}>
            <Eyebrow style={{ letterSpacing: "0.08em" }}>Rate</Eyebrow>
            <div style={{ flex: 1, display: "flex", alignItems: "center", gap: 10 }}>
              <Slider
                value={settings.cues.rate}
                min={-10}
                max={10}
                step={1}
                onChange={(v) =>
                  guard(async () => {
                    await setCueRate(v);
                    await refreshSettings();
                  })
                }
              />
              <span className="mono" style={{ fontSize: 13, color: "var(--text-2)", width: 28, textAlign: "right" }}>
                {settings.cues.rate > 0 ? `+${settings.cues.rate}` : settings.cues.rate}
              </span>
            </div>
          </div>
          <div style={{ display: "flex", flexDirection: "column", gap: 7 }}>
            <Eyebrow style={{ letterSpacing: "0.08em" }}>Cue volume</Eyebrow>
            <Volume
              value={Math.round(settings.cues.volume * 100)}
              onChange={(v) =>
                guard(async () => {
                  await setCueVolume(v / 100);
                  await refreshSettings();
                })
              }
            />
          </div>
        </div>

        <div style={{ display: "flex", gap: 10, marginTop: 18 }}>
          <button
            className="btn btn-ghost"
            onClick={() => guard(() => cueSpeak("Cue check, one two."))}
          >
            <Icon name="play" size={14} stroke="var(--text-2)" /> Test
          </button>
          {speaking && (
            <div className="cue-speaking">
              <span className="cue-speaking-dot" />
              <span className="cue-speaking-label">
                Speaking{speakingLabel ? `: ${speakingLabel}` : "…"}
              </span>
              <button className="btn btn-danger" onClick={() => guard(() => cueStop())}>
                <Icon name="stop" size={14} stroke="var(--danger)" /> Stop
              </button>
            </div>
          )}
        </div>
      </Card>

      <Card pad={24}>
        <div style={{ display: "flex", alignItems: "center", gap: 9, marginBottom: 14 }}>
          <Icon name="speaker" size={17} stroke="var(--accent-ink)" />
          <Eyebrow style={{ letterSpacing: "0.1em" }}>Auto cues</Eyebrow>
        </div>
        <label className="click-toggle">
          <input
            type="checkbox"
            checked={settings.cues.speak_key_on_change}
            onChange={(e) =>
              guard(async () => {
                await setCueSpeakKey(e.target.checked);
                await refreshSettings();
              })
            }
          />
          Speak key when pad changes
        </label>
        <p className="helper-note">
          When you tap a new pad, the PC announces it on the cue bus (e.g. <code>Key of G</code>).
        </p>
      </Card>

      <Card pad={24}>
        <div style={{ display: "flex", alignItems: "center", gap: 9, marginBottom: 16 }}>
          <Icon name="grid" size={17} stroke="var(--accent-ink)" />
          <Eyebrow style={{ letterSpacing: "0.1em" }}>Quick cues</Eyebrow>
          <div style={{ flex: 1 }} />
          <span className="mono" style={{ fontSize: 11.5, color: "var(--text-3)" }}>
            {cues.length} saved
          </span>
        </div>

        {cues.length === 0 ? (
          <p className="empty-note">
            No quick cues yet. Add one — a label is what shows on the phone button (e.g. <code>Verse 2</code>),
            and the text is what the PC speaks.
          </p>
        ) : (
          <div style={{ display: "flex", flexDirection: "column", gap: 10 }}>
            {cues.map((cue) => (
              <CueRow
                key={cue.id}
                cue={cue}
                guard={guard}
                refreshSettings={refreshSettings}
                speakingLabel={speakingLabel}
              />
            ))}
          </div>
        )}

        <div style={{ marginTop: 14 }}>
          <button
            className="btn btn-ghost"
            onClick={() =>
              guard(async () => {
                const label = `New cue ${cues.length + 1}`;
                await cueAdd(label, label);
                await refreshSettings();
              })
            }
          >
            <Icon name="plus" size={15} stroke="var(--text-2)" /> Add cue
          </button>
        </div>
      </Card>

      <Card pad={24}>
        <div style={{ display: "flex", alignItems: "center", gap: 9, marginBottom: 14 }}>
          <Icon name="waves" size={17} stroke="var(--accent-ink)" />
          <Eyebrow style={{ letterSpacing: "0.1em" }}>Type to speak</Eyebrow>
        </div>
        <textarea
          className="cue-textarea"
          rows={3}
          placeholder="What should the PC say?"
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => {
            if ((e.metaKey || e.ctrlKey) && e.key === "Enter" && draft.trim()) {
              e.preventDefault();
              guard(() => cueSpeak(draft));
            }
          }}
        />
        <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", marginTop: 10 }}>
          <span className="mini-label">⌘/Ctrl + Enter to speak</span>
          <button
            className="btn btn-accent"
            disabled={!draft.trim()}
            onClick={() => guard(() => cueSpeak(draft))}
          >
            <Icon name="play" size={14} stroke="var(--on-accent)" /> Speak
          </button>
        </div>
      </Card>
    </div>
  );
}

function CueRow({
  cue,
  guard,
  refreshSettings,
  speakingLabel,
}: {
  cue: QuickCue;
  guard: (fn: () => Promise<unknown>) => Promise<void>;
  refreshSettings: () => Promise<void>;
  speakingLabel: string | null;
}) {
  const [label, setLabel] = useState(cue.label);
  const [text, setText] = useState(cue.text);
  const lastSaved = useRef({ label: cue.label, text: cue.text });

  // Pick up upstream edits without trashing in-progress text.
  useEffect(() => {
    if (lastSaved.current.label === cue.label && lastSaved.current.text === cue.text) {
      setLabel(cue.label);
      setText(cue.text);
      lastSaved.current = { label: cue.label, text: cue.text };
    }
  }, [cue.label, cue.text]);

  const dirty = label !== lastSaved.current.label || text !== lastSaved.current.text;
  const active = speakingLabel === cue.label;

  async function commit() {
    if (!dirty) return;
    await guard(async () => {
      await cueUpdate(cue.id, label, text);
      lastSaved.current = { label, text };
      await refreshSettings();
    });
  }

  return (
    <div className={`cue-row${active ? " active" : ""}`}>
      <input
        className="cue-label"
        value={label}
        onChange={(e) => setLabel(e.target.value)}
        onBlur={commit}
        placeholder="Label"
      />
      <input
        className="cue-text"
        value={text}
        onChange={(e) => setText(e.target.value)}
        onBlur={commit}
        placeholder="What to speak"
      />
      <button
        className="btn btn-ghost"
        title="Speak this cue"
        onClick={() => guard(() => cueSpeakQuick(cue.id))}
      >
        <Icon name="play" size={14} stroke="var(--text-2)" /> Speak
      </button>
      <button
        className="icon-btn"
        title="Delete cue"
        onClick={() =>
          guard(async () => {
            await cueRemove(cue.id);
            await refreshSettings();
          })
        }
      >
        <Icon name="trash" size={15} stroke="var(--text-2)" />
      </button>
    </div>
  );
}
