// Pads page — pick a chord, set master volume, big stop button.

import {
  BUILTIN_SYNTH_ID,
  playKey,
  setVolume,
  stopPads,
  type Key,
  type NowPlaying,
  type Preset,
  type Settings,
} from "../lib/ipc";
import {
  Card,
  Icon,
  Meter,
  PadCluster,
  Segmented,
  Volume,
  type Note,
  type PadStyle,
} from "./ui";

interface Props {
  settings: Settings;
  now: NowPlaying | null;
  padStyle: PadStyle;
  onPadStyleChange: (v: PadStyle) => void;
  activePreset: Preset | null;
  assignments: Partial<Record<Note, string>>;
  guard: (fn: () => Promise<unknown>) => Promise<void>;
  onGoLibrary: () => void;
}

export function PadsPage({
  settings,
  now,
  padStyle,
  onPadStyleChange,
  activePreset,
  assignments,
  guard,
  onGoLibrary,
}: Props) {
  const playing = !!now?.playing;
  const playingKey = playing ? ((now?.key ?? null) as Note | null) : null;
  const liveVolume = Math.round((now?.volume ?? settings.master_volume) * 100);
  const mappedCount = Object.keys(assignments).length;
  const isBuiltinSynth = activePreset?.id === BUILTIN_SYNTH_ID;

  return (
    <Card pad={28} style={{ display: "flex", flexDirection: "column" }}>
      <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between" }}>
        <div style={{ display: "flex", alignItems: "baseline", gap: 13, minWidth: 0 }}>
          <span
            className="display"
            style={{ fontSize: 26, lineHeight: 1, whiteSpace: "nowrap" }}
          >
            {activePreset ? activePreset.name : "No bank selected"}
          </span>
          {activePreset && (
            <span className="mono" style={{ fontSize: 12, color: "var(--text-3)" }}>
              {isBuiltinSynth ? "generated" : `${mappedCount} / 12 keys`}
            </span>
          )}
        </div>
        <div style={{ display: "flex", alignItems: "center", gap: 14 }}>
          <Segmented<PadStyle>
            value={padStyle}
            onChange={onPadStyleChange}
            options={[
              { key: "grid", label: "Grid", icon: "grid" },
              { key: "piano", label: "Piano", icon: "piano" },
            ]}
          />
          <Meter live={playing} h={26} />
        </div>
      </div>

      <div style={{ marginTop: 24, marginBottom: 4 }}>
        <PadCluster
          variant={padStyle}
          playing={playingKey}
          assignments={assignments}
          cols={6}
          h={88}
          big
          onTrigger={(n) => guard(() => playKey(n as Key))}
        />
      </div>

      <div className="pads-synth-hint">
        <Icon name="waves" size={14} stroke="var(--text-3)" />
        <span>
          {isBuiltinSynth
            ? "Every key plays a generated worship pad. "
            : mappedCount === 12
              ? "Every key has a file."
              : "Keys without a file play a generated worship pad. "}
          {mappedCount < 12 && (
            <button className="linklike" onClick={onGoLibrary}>
              {isBuiltinSynth ? "Add your own pads in the Library" : "Add your own in the Library"}
            </button>
          )}
        </span>
      </div>

      <div
        style={{
          marginTop: "auto",
          paddingTop: 30,
          display: "flex",
          alignItems: "center",
          gap: 20,
        }}
      >
        <Volume
          value={liveVolume}
          big
          onChange={(v) => guard(() => setVolume(v / 100))}
        />
        <button
          className="btn btn-danger"
          disabled={!playing}
          onClick={() => guard(() => stopPads())}
        >
          <Icon name="power" size={16} stroke="var(--danger)" /> Stop / Fade
        </button>
      </div>
    </Card>
  );
}
