import { useEffect, useRef } from "react";
import { ALL_KEYS, type Key, type NowPlaying } from "../../shared/types";
import { post, type Info } from "../api";
import { Meter } from "./Meter";
import { ChevDownIcon, PadsTabIcon, PianoIcon, PowerIcon, VolumeIcon } from "./icons";

const SHARPS: ReadonlySet<Key> = new Set<Key>(["C#", "D#", "F#", "G#", "A#"]);
const BLACK_AFTER: Partial<Record<Key, Key>> = {
  C: "C#",
  D: "D#",
  F: "F#",
  G: "G#",
  A: "A#",
};

export type PadStyle = "grid" | "piano";

interface Props {
  info: Info | null;
  now: NowPlaying;
  padStyle: PadStyle;
  onPadStyle: (s: PadStyle) => void;
}

export function PadsTab({ info, now, padStyle, onPadStyle }: Props) {
  const mapped = new Set<Key>((info?.mapped_keys ?? []) as Key[]);
  const files = (info?.files ?? {}) as Partial<Record<Key, string>>;

  const triggerKey = (k: Key) => {
    if (now.playing && now.key === k) post("/api/stop");
    else post(`/api/play/${encodeURIComponent(k)}`);
  };

  return (
    <section className="tab-body" role="tabpanel" aria-label="Pads">
      <div className="bank-row">
        <div className="field">
          <select
            aria-label="Bank"
            value={info?.active_preset ?? ""}
            onChange={(e) => post(`/api/preset/${encodeURIComponent(e.target.value)}`)}
          >
            {info && info.presets.length === 0 && (
              <option disabled>No banks — add one in the desktop app</option>
            )}
            {info?.presets.map((p) => (
              <option key={p.id} value={p.id}>
                {p.name}
              </option>
            ))}
          </select>
          <span className="chev">
            <ChevDownIcon />
          </span>
        </div>
        <div className="seg">
          <button
            type="button"
            title="Grid"
            aria-label="Grid view"
            className={padStyle === "grid" ? "on" : ""}
            onClick={() => onPadStyle("grid")}
          >
            <PadsTabIcon />
          </button>
          <button
            type="button"
            title="Piano"
            aria-label="Piano view"
            className={padStyle === "piano" ? "on" : ""}
            onClick={() => onPadStyle("piano")}
          >
            <PianoIcon />
          </button>
        </div>
      </div>

      <div className={`pads ${padStyle}`}>
        {padStyle === "grid"
          ? ALL_KEYS.map((k) => (
              <PadButton
                key={k}
                k={k}
                file={files[k]}
                mapped={mapped.has(k) || !!files[k]}
                active={now.playing && now.key === k}
                onTrigger={triggerKey}
              />
            ))
          : ALL_KEYS.filter((k) => !SHARPS.has(k)).map((white) => {
              const black = BLACK_AFTER[white];
              return (
                <PianoKeyPair
                  key={white}
                  white={white}
                  black={black}
                  files={files}
                  mapped={mapped}
                  now={now}
                  onTrigger={triggerKey}
                />
              );
            })}
      </div>

      <Volume value={now.volume} />

      <Transport now={now} />
    </section>
  );
}

function PadButton({
  k,
  file,
  mapped,
  active,
  onTrigger,
}: {
  k: Key;
  file: string | undefined;
  mapped: boolean;
  active: boolean;
  onTrigger: (k: Key) => void;
}) {
  const synth = !mapped;
  return (
    <button
      type="button"
      className={`pad${active ? " active" : ""}${synth ? " synth" : ""}`}
      onClick={() => onTrigger(k)}
    >
      <span className="pad-top">
        <span className="note">{k}</span>
        {active && <Meter />}
      </span>
      <span className={`file${synth ? " synth" : ""}`}>{file ?? "generated"}</span>
    </button>
  );
}

function PianoKeyPair({
  white,
  black,
  files,
  mapped,
  now,
  onTrigger,
}: {
  white: Key;
  black: Key | undefined;
  files: Partial<Record<Key, string>>;
  mapped: Set<Key>;
  now: NowPlaying;
  onTrigger: (k: Key) => void;
}) {
  const whiteHas = !!files[white] || mapped.has(white);
  const whiteActive = now.playing && now.key === white;
  const blackHas = black ? !!files[black] || mapped.has(black) : false;
  const blackActive = black ? now.playing && now.key === black : false;

  return (
    <div className="pkey-wrap">
      <button
        type="button"
        className={`pkey${whiteActive ? " active" : ""}${whiteHas ? "" : " synth"}`}
        onClick={() => onTrigger(white)}
      >
        {whiteActive && (
          <span style={{ marginBottom: 4 }}>
            <Meter />
          </span>
        )}
        <span className="note">{white}</span>
        <span className={`dot${whiteHas ? " has" : " synth"}`} />
      </button>
      {black && (
        <button
          type="button"
          className={`bkey${blackActive ? " active" : ""}${blackHas ? "" : " synth"}`}
          onClick={() => onTrigger(black)}
        >
          <span className="note">{black}</span>
          <span className={`dot${blackHas ? " has" : " synth"}`} />
        </button>
      )}
    </div>
  );
}

/** Volume slider — keeps its own DOM value so a mid-drag WS update doesn't jump it. */
function Volume({ value }: { value: number }) {
  const ref = useRef<HTMLInputElement>(null);
  const numRef = useRef<HTMLSpanElement>(null);

  useEffect(() => {
    const el = ref.current;
    if (!el || document.activeElement === el) return;
    const v = Math.round((value ?? 0.8) * 100);
    el.value = String(v);
    el.style.setProperty("--pct", `${v}%`);
    if (numRef.current) numRef.current.textContent = String(v);
  }, [value]);

  return (
    <div className="vol">
      <VolumeIcon />
      <input
        ref={ref}
        type="range"
        min={0}
        max={100}
        defaultValue={80}
        aria-label="Volume"
        onInput={(e) => {
          const el = e.currentTarget;
          el.style.setProperty("--pct", `${el.value}%`);
          if (numRef.current) numRef.current.textContent = el.value;
        }}
        onChange={(e) => post("/api/volume", { volume: Number(e.currentTarget.value) / 100 })}
      />
      <span className="num" ref={numRef}>
        80
      </span>
    </div>
  );
}

function Transport({ now }: { now: NowPlaying }) {
  if (now.playing) {
    return (
      <>
        <button
          type="button"
          className="transport live"
          onClick={() => post("/api/stop")}
        >
          <PowerIcon />
          Stop / Fade out
        </button>
        <p className="hint">Tap the glowing pad again to fade it out.</p>
      </>
    );
  }
  return (
    <>
      <button type="button" className="transport idle" disabled>
        Tap a pad to begin
      </button>
      <p className="hint" />
    </>
  );
}
