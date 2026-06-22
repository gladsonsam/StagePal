// Design-system building blocks ported from the Claude Design handoff.
// Visuals come from CSS tokens in App.css; these components add structure,
// state, and real interactivity.

import { useEffect, useRef, useState } from "react";
import type { CSSProperties, ReactNode } from "react";
import { useHoldRepeat } from "../shared/useHoldRepeat";

/* ── line icons (1.6 stroke, 20×20 viewbox) ───────────────────────── */
export type IconName =
  | "chevron"
  | "chevronR"
  | "x"
  | "plus"
  | "power"
  | "wifi"
  | "speaker"
  | "sliders"
  | "folder"
  | "check"
  | "phone"
  | "waves"
  | "pencil"
  | "trash"
  | "copy"
  | "grid"
  | "piano"
  | "metronome"
  | "play"
  | "stop"
  | "minus"
  | "mic";

export function Icon({
  name,
  size = 16,
  stroke = "currentColor",
  sw = 1.6,
}: {
  name: IconName;
  size?: number;
  stroke?: string;
  sw?: number;
}) {
  const p = {
    fill: "none",
    stroke,
    strokeWidth: sw,
    strokeLinecap: "round" as const,
    strokeLinejoin: "round" as const,
  };
  const paths: Record<IconName, ReactNode> = {
    chevron: <path d="M5 7.5l5 5 5-5" {...p} />,
    chevronR: <path d="M7.5 5l5 5-5 5" {...p} />,
    x: <path d="M5 5l10 10M15 5L5 15" {...p} />,
    plus: <path d="M10 4v12M4 10h12" {...p} />,
    power: (
      <g {...p}>
        <path d="M10 3v7" />
        <path d="M5.5 6a6 6 0 1 0 9 0" />
      </g>
    ),
    wifi: (
      <g {...p}>
        <path d="M3 7.5a11 11 0 0 1 14 0" />
        <path d="M5.5 10.5a7 7 0 0 1 9 0" />
        <path d="M8 13.5a3 3 0 0 1 4 0" />
        <circle cx="10" cy="16" r=".6" fill={stroke} stroke="none" />
      </g>
    ),
    speaker: (
      <g {...p}>
        <path d="M4 8v4h3l4 3V5L7 8H4z" />
        <path d="M13.5 7.5a4 4 0 0 1 0 5" />
      </g>
    ),
    sliders: (
      <g {...p}>
        <path d="M5 4v12M15 4v12" />
        <circle cx="5" cy="11" r="2" />
        <circle cx="15" cy="7" r="2" />
      </g>
    ),
    folder: (
      <path
        d="M3 6.5A1.5 1.5 0 0 1 4.5 5H8l1.5 1.8h6A1.5 1.5 0 0 1 17 8.3V14a1.5 1.5 0 0 1-1.5 1.5h-11A1.5 1.5 0 0 1 3 14V6.5z"
        {...p}
      />
    ),
    check: <path d="M4 10.5l3.5 3.5L16 6" {...p} />,
    phone: (
      <g {...p}>
        <rect x="6" y="3" width="8" height="14" rx="1.6" />
        <path d="M9 14.5h2" />
      </g>
    ),
    waves: (
      <g {...p}>
        <path d="M3 10c1.6 0 1.6-4 3.2-4S7.8 14 9.4 14 11 6 12.6 6 14.2 10 17 10" />
      </g>
    ),
    pencil: (
      <g {...p}>
        <path d="M12.5 4.2l3.3 3.3" />
        <path d="M11 5.7L4.3 12.4 3.6 16l3.6-.7 6.7-6.7-2.9-2.9z" />
      </g>
    ),
    trash: (
      <g {...p}>
        <path d="M4.5 6h11" />
        <path d="M8 6V4.4h4V6" />
        <path d="M6 6l.7 9.4a1 1 0 0 0 1 .9h4.6a1 1 0 0 0 1-.9L14 6" />
      </g>
    ),
    copy: (
      <g {...p}>
        <rect x="7" y="7" width="9" height="9" rx="1.6" />
        <path d="M4 13V5a1 1 0 0 1 1-1h8" />
      </g>
    ),
    grid: (
      <g {...p}>
        <rect x="4" y="4" width="5" height="5" rx="1.2" />
        <rect x="11" y="4" width="5" height="5" rx="1.2" />
        <rect x="4" y="11" width="5" height="5" rx="1.2" />
        <rect x="11" y="11" width="5" height="5" rx="1.2" />
      </g>
    ),
    piano: (
      <g {...p}>
        <rect x="3.5" y="4.5" width="13" height="11" rx="1.6" />
        <path d="M7.3 4.5v11M10 4.5v11M12.7 4.5v11" />
      </g>
    ),
    metronome: (
      <g {...p}>
        <path d="M6.2 4h7.6l2 13H4.2l2-13z" />
        <path d="M5.5 12.5h9" />
        <path d="M10 14.5l3-6.5" />
      </g>
    ),
    play: <path d="M6 4l10 6-10 6V4z" {...p} />,
    stop: <rect x="5" y="5" width="10" height="10" rx="1.6" {...p} />,
    minus: <path d="M4 10h12" {...p} />,
    mic: (
      <g {...p}>
        <rect x="8" y="3" width="4" height="9" rx="2" />
        <path d="M5 10a5 5 0 0 0 10 0" />
        <path d="M10 15v2" />
      </g>
    ),
  };
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 20 20"
      style={{ display: "block", flexShrink: 0 }}
      aria-hidden
    >
      {paths[name]}
    </svg>
  );
}

/* ── brand mark: rounded tile + still-water wave ──────────────────── */
export function Mark({ size = 34 }: { size?: number }) {
  const rad = size * 0.28;
  return (
    <div
      style={{
        width: size,
        height: size,
        borderRadius: rad,
        flexShrink: 0,
        background:
          "linear-gradient(160deg, color-mix(in oklch, var(--accent) 92%, white 8%), var(--accent))",
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        boxShadow: "0 1px 2px rgba(0,0,0,.12), inset 0 1px 0 rgba(255,255,255,.25)",
      }}
    >
      <svg width={size * 0.62} height={size * 0.62} viewBox="0 0 22 22">
        <path
          d="M2 13c2.4 0 2.4-5 4.8-5S9.2 17 11.6 17 14 8 16.4 8 19 13 20 13"
          fill="none"
          stroke="var(--on-accent)"
          strokeWidth="1.7"
          strokeLinecap="round"
          strokeLinejoin="round"
          opacity="0.95"
        />
        <path
          d="M2 17.5h18"
          fill="none"
          stroke="var(--on-accent)"
          strokeWidth="1.5"
          strokeLinecap="round"
          opacity="0.4"
        />
      </svg>
    </div>
  );
}

/* ── ambient level meter (calm vertical bars) ─────────────────────── */
export function Meter({
  live,
  bars = 7,
  h = 28,
}: {
  live: boolean;
  bars?: number;
  h?: number;
}) {
  return (
    <div className={`meter${live ? " live" : ""}`} style={{ height: h }}>
      {Array.from({ length: bars }).map((_, i) => (
        <div
          key={i}
          className="meter-bar"
          style={{
            height: h,
            animation: live
              ? `meterPulse ${2.4 + (i % 4) * 0.5}s ease-in-out ${i * 0.18}s infinite`
              : "none",
          }}
        />
      ))}
    </div>
  );
}

export function Eyebrow({ children, style }: { children: ReactNode; style?: CSSProperties }) {
  return (
    <div className="eyebrow" style={style}>
      {children}
    </div>
  );
}

export function Card({
  children,
  style,
  pad = 24,
}: {
  children: ReactNode;
  style?: CSSProperties;
  pad?: number;
}) {
  return (
    <div className="card" style={{ padding: pad, ...style }}>
      {children}
    </div>
  );
}

/* ── select-style field ───────────────────────────────────────────── */
export function SelectField({
  label,
  value,
  options,
  onChange,
  mono,
  disabled,
  placeholder,
  style,
}: {
  label?: string;
  value: string;
  options: { value: string; label: string }[];
  onChange?: (v: string) => void;
  mono?: boolean;
  disabled?: boolean;
  placeholder?: string;
  style?: CSSProperties;
}) {
  return (
    <div className="field" style={style}>
      {label && (
        <Eyebrow style={{ letterSpacing: "0.08em" }}>{label}</Eyebrow>
      )}
      <div className={`field-box${mono ? " mono" : ""}`}>
        <select
          value={value}
          disabled={disabled}
          onChange={(e) => onChange?.(e.target.value)}
        >
          {placeholder !== undefined && (
            <option value="" disabled>
              {placeholder}
            </option>
          )}
          {options.map((o) => (
            <option key={o.value} value={o.value}>
              {o.label}
            </option>
          ))}
        </select>
        <span className="chev">
          <Icon name="chevron" size={15} stroke="var(--text-3)" />
        </span>
      </div>
    </div>
  );
}

/* ── segmented toggle ─────────────────────────────────────────────── */
export function Segmented<T extends string>({
  value,
  options,
  onChange,
}: {
  value: T;
  options: { key: T; label?: string; icon?: IconName }[];
  onChange: (v: T) => void;
}) {
  return (
    <div className="seg">
      {options.map((o) => {
        const on = value === o.key;
        return (
          <button
            key={o.key}
            type="button"
            className={`seg-opt${on ? " on" : ""}${o.label ? "" : " icon-only"}`}
            onClick={() => onChange(o.key)}
            title={o.label ?? o.key}
          >
            {o.icon && (
              <Icon
                name={o.icon}
                size={15}
                stroke={on ? "var(--accent-ink)" : "var(--text-3)"}
              />
            )}
            {o.label}
          </button>
        );
      })}
    </div>
  );
}

/* ── volume (icon + range + numeric) ──────────────────────────────── */
export function Volume({
  value,
  onChange,
  big = false,
}: {
  value: number;
  onChange: (v: number) => void;
  big?: boolean;
}) {
  return (
    <div className={`vol${big ? " vol--big" : ""}`}>
      <Icon name="speaker" size={big ? 19 : 17} stroke="var(--text-3)" />
      <input
        type="range"
        className={`range${big ? " range--big" : ""}`}
        min={0}
        max={100}
        value={value}
        style={{ "--pct": `${value}%` } as CSSProperties}
        onChange={(e) => onChange(Number(e.target.value))}
      />
      <div className="vol-num">{value}</div>
    </div>
  );
}

/* ── bare slider (no icon / no readout) — crossfade ───────────────── */
export function Slider({
  value,
  min = 0,
  max = 100,
  step = 1,
  onChange,
}: {
  value: number;
  min?: number;
  max?: number;
  step?: number;
  onChange: (v: number) => void;
}) {
  const pct = ((value - min) / (max - min)) * 100;
  return (
    <input
      type="range"
      className="range"
      min={min}
      max={max}
      step={step}
      value={value}
      style={{ "--pct": `${pct}%` } as CSSProperties}
      onChange={(e) => onChange(Number(e.target.value))}
    />
  );
}

/* ── pad cluster (grid or piano) ──────────────────────────────────── */
export const NOTES = [
  "C",
  "C#",
  "D",
  "D#",
  "E",
  "F",
  "F#",
  "G",
  "G#",
  "A",
  "A#",
  "B",
] as const;
export type Note = (typeof NOTES)[number];
const SHARPS = new Set<Note>(["C#", "D#", "F#", "G#", "A#"]);

export type PadStyle = "grid" | "piano";

export function PadCluster({
  variant,
  playing,
  assignments,
  cols = 6,
  h = 78,
  big = false,
  onTrigger,
}: {
  variant: PadStyle;
  playing: Note | null;
  assignments: Partial<Record<Note, string>>;
  cols?: number;
  h?: number;
  big?: boolean;
  onTrigger: (n: Note) => void;
}) {
  if (variant === "piano") {
    return <Piano playing={playing} assignments={assignments} onTrigger={onTrigger} />;
  }
  return (
    <div className="pad-grid" style={{ gridTemplateColumns: `repeat(${cols}, minmax(0, 1fr))` }}>
      {NOTES.map((n) => {
        const file = assignments[n];
        const empty = !file;
        const active = playing === n;
        return (
          <button
            key={n}
            type="button"
            className={`pad${active ? " active" : ""}${empty ? " empty" : ""}`}
            style={{ height: h, padding: big ? "12px 14px" : "10px 12px" }}
            disabled={empty}
            onClick={() => onTrigger(n)}
            title={empty ? `${n} — no pad mapped` : `${n} · ${file}`}
          >
            <div className="pad-top">
              <span className="pad-note" style={{ fontSize: big ? 24 : 20 }}>
                {n}
              </span>
              {active && <Meter live bars={4} h={16} />}
            </div>
            <span className={`pad-file${file ? " mono" : " empty-label"}`}>
              {file || "empty"}
            </span>
          </button>
        );
      })}
    </div>
  );
}

/* ── click: BPM display + steppers ─────────────────────────────────── */
// Steppers: tap = ±1, hold = auto-repeat with acceleration.
// Number: click to edit; Enter/blur commits, Esc cancels.
export function BpmDisplay({
  value,
  onChange,
  min = 30,
  max = 300,
}: {
  value: number;
  onChange: (v: number) => void;
  min?: number;
  max?: number;
}) {
  const clamp = (v: number) => Math.max(min, Math.min(max, Math.round(v)));
  const apply = (v: number) => onChange(clamp(v));
  const minusProps = useHoldRepeat(value, -1, apply);
  const plusProps = useHoldRepeat(value, +1, apply);

  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState("");
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (editing && inputRef.current) {
      inputRef.current.focus();
      inputRef.current.select();
    }
  }, [editing]);

  const startEdit = () => {
    setDraft(String(Math.round(value)));
    setEditing(true);
  };
  const commit = () => {
    const n = Number(draft);
    if (Number.isFinite(n) && draft.trim() !== "") onChange(clamp(n));
    setEditing(false);
  };

  return (
    <div className="bpm">
      <button
        type="button"
        className="bpm-step"
        title="−1 BPM (hold to ramp)"
        aria-label="Decrease BPM"
        {...minusProps}
      >
        <Icon name="minus" size={16} stroke="var(--text-2)" />
      </button>
      <div
        className="bpm-num"
        onClick={editing ? undefined : startEdit}
        onKeyDown={(e) => {
          if (editing) return;
          if (e.key === "Enter" || e.key === " ") {
            e.preventDefault();
            startEdit();
          }
        }}
        role="button"
        tabIndex={editing ? -1 : 0}
        title={editing ? undefined : "Click to type BPM"}
      >
        <span className="bpm-glyph">♩=</span>
        {editing ? (
          <input
            ref={inputRef}
            className="bpm-input"
            type="text"
            inputMode="numeric"
            pattern="[0-9]*"
            value={draft}
            onChange={(e) =>
              setDraft(e.target.value.replace(/[^0-9]/g, "").slice(0, 4))
            }
            onBlur={commit}
            onKeyDown={(e) => {
              if (e.key === "Enter") commit();
              else if (e.key === "Escape") setEditing(false);
            }}
          />
        ) : (
          <span className="bpm-val">{Math.round(value)}</span>
        )}
      </div>
      <button
        type="button"
        className="bpm-step"
        title="+1 BPM (hold to ramp)"
        aria-label="Increase BPM"
        {...plusProps}
      >
        <Icon name="plus" size={16} stroke="var(--text-2)" />
      </button>
    </div>
  );
}

/* ── click: beat dots, predict locally from started_at + bpm ──────── */
export function BeatDots({
  beatsPerBar,
  bpm,
  startedAtMs,
  size = 10,
}: {
  beatsPerBar: number;
  bpm: number;
  /** Wall-clock unix ms when the click was last (re)started; null when off. */
  startedAtMs: number | null;
  size?: number;
}) {
  const [tick, setTick] = useState(0);
  useEffect(() => {
    if (startedAtMs == null) {
      setTick(0);
      return;
    }
    // setInterval, not requestAnimationFrame: RAF is suspended in background
    // tabs and headless browsers (which leaves the dots frozen). 50 ms is
    // smooth enough for a 4-dot indicator and costs near-nothing.
    const id = window.setInterval(() => setTick((t) => t + 1), 50);
    return () => window.clearInterval(id);
  }, [startedAtMs]);

  const live = startedAtMs != null && bpm > 0 && beatsPerBar > 0;
  const current = live
    ? Math.floor(((Date.now() - startedAtMs) * bpm) / 60_000) % beatsPerBar
    : -1;

  // touch `tick` so React keeps re-rendering during animation
  void tick;

  return (
    <div className="beat-dots">
      {Array.from({ length: Math.max(1, beatsPerBar) }).map((_, i) => (
        <span
          key={i}
          className={`beat-dot${live && i === current ? " on" : ""}${i === 0 ? " one" : ""}`}
          style={{ width: size, height: size }}
        />
      ))}
    </div>
  );
}

/* ── click: tap-tempo button ──────────────────────────────────────── */
/**
 * Maintains a rolling window of recent tap timestamps and reports a fresh BPM
 * once we have ≥2 taps. A gap of >2 s resets the window (treated as a new
 * attempt). Caller is responsible for actually committing the BPM upstream.
 */
export function TapButton({
  onTap,
  big = false,
}: {
  /** Called with the latest median-derived BPM, after the 2nd tap onward. */
  onTap: (bpm: number) => void;
  big?: boolean;
}) {
  const tapsRef = useRef<number[]>([]);
  const [count, setCount] = useState(0);
  const idleTimer = useRef<number | null>(null);

  function tap() {
    const now = performance.now();
    const last = tapsRef.current[tapsRef.current.length - 1];
    if (last != null && now - last > 2000) {
      tapsRef.current = [];
    }
    tapsRef.current.push(now);
    if (tapsRef.current.length > 5) tapsRef.current.shift();

    if (tapsRef.current.length >= 2) {
      const deltas: number[] = [];
      for (let i = 1; i < tapsRef.current.length; i++) {
        deltas.push(tapsRef.current[i] - tapsRef.current[i - 1]);
      }
      const sorted = [...deltas].sort((a, b) => a - b);
      const median = sorted[Math.floor(sorted.length / 2)];
      const bpm = Math.max(30, Math.min(300, 60_000 / median));
      onTap(Math.round(bpm * 10) / 10);
    }
    setCount(tapsRef.current.length);

    if (idleTimer.current != null) window.clearTimeout(idleTimer.current);
    idleTimer.current = window.setTimeout(() => {
      tapsRef.current = [];
      setCount(0);
    }, 2200);
  }

  let label = "TAP";
  if (count === 1) label = "tap again…";
  else if (count >= 2 && count < 4) label = `tap ${count} of 4…`;
  else if (count >= 4) label = "TAP";

  return (
    <button
      type="button"
      className={`tap-btn${count > 0 ? " hot" : ""}${big ? " tap-btn--big" : ""}`}
      onClick={tap}
    >
      {label}
    </button>
  );
}

function Piano({
  playing,
  assignments,
  onTrigger,
}: {
  playing: Note | null;
  assignments: Partial<Record<Note, string>>;
  onTrigger: (n: Note) => void;
}) {
  const whites = NOTES.filter((n) => !SHARPS.has(n));
  const blackAfter: Partial<Record<Note, Note>> = {
    C: "C#",
    D: "D#",
    F: "F#",
    G: "G#",
    A: "A#",
  };
  return (
    <div className="piano" style={{ height: 200 }}>
      {whites.map((n) => {
        const active = playing === n;
        const has = !!assignments[n];
        const bn = blackAfter[n];
        const bActive = bn && playing === bn;
        const bHas = bn && !!assignments[bn];
        return (
          <div key={n} className="pkey-wrap">
            <button
              type="button"
              className={`pkey${active ? " active" : ""}`}
              disabled={!has}
              onClick={() => onTrigger(n)}
              title={has ? n : `${n} — no pad mapped`}
            >
              {active && (
                <div style={{ marginBottom: 6 }}>
                  <Meter live bars={4} h={18} />
                </div>
              )}
              <span className="pkey-note">{n}</span>
              <span className={`pkey-dot${has ? " has" : ""}`} />
            </button>
            {bn && (
              <button
                type="button"
                className={`bkey${bActive ? " active" : ""}`}
                disabled={!bHas}
                onClick={() => onTrigger(bn)}
                title={bHas ? bn : `${bn} — no pad mapped`}
              >
                <span className="bkey-note">{bn}</span>
                <span className={`bkey-dot${bHas ? " has" : ""}`} />
              </button>
            )}
          </div>
        );
      })}
    </div>
  );
}
