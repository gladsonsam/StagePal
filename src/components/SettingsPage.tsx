// Settings page — output device, three bus routings (pad/click/cue),
// crossfade duration, and the duck-click toggle. Each card is a self-contained
// concern; adding a fourth bus is one more card.

import { useState } from "react";
import {
  runAudioOutputTest,
  setAudioOutput,
  setClickChannels,
  setCrossfade,
  setCueChannels,
  setCueDuckClick,
  type AudioDebugReport,
  type DeviceInfo,
  type Settings,
} from "../lib/ipc";
import { Card, Eyebrow, Icon, SelectField, Slider } from "./ui";
import { RoutingPicker } from "./RoutingPicker";

interface Props {
  settings: Settings;
  devices: DeviceInfo[];
  channelCount: number;
  guard: (fn: () => Promise<unknown>) => Promise<void>;
  refreshSettings: () => Promise<void>;
}

// Stable composite value so the device dropdown can disambiguate same-named
// drivers that exist on both WASAPI and ASIO (e.g. some USB interfaces).
const DEVICE_SEP = "|>|";
const deviceKey = (host: string, name: string): string =>
  host + DEVICE_SEP + name;
const splitDeviceKey = (key: string): [string, string] => {
  const i = key.indexOf(DEVICE_SEP);
  return i < 0
    ? ["WASAPI", key]
    : [key.slice(0, i), key.slice(i + DEVICE_SEP.length)];
};

export function SettingsPage({
  settings,
  devices,
  channelCount,
  guard,
  refreshSettings,
}: Props) {
  const [debugReport, setDebugReport] = useState<AudioDebugReport | null>(null);
  const [debugError, setDebugError] = useState<string | null>(null);
  const [debugBusy, setDebugBusy] = useState(false);

  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 22 }}>
      <Card pad={22}>
        <div
          style={{
            display: "flex",
            alignItems: "center",
            gap: 9,
            marginBottom: 16,
          }}
        >
          <Icon name="sliders" size={17} stroke="var(--accent-ink)" />
          <Eyebrow style={{ letterSpacing: "0.1em" }}>Output device</Eyebrow>
        </div>
        <SelectField
          label="Device"
          value={
            settings.output_device
              ? deviceKey(settings.output_host, settings.output_device)
              : ""
          }
          placeholder="Select a device…"
          options={devices.map((d) => ({
            value: deviceKey(d.host, d.name),
            label: `[${d.host}] ${d.name} · ${d.channels}ch @ ${Math.round(
              d.default_sample_rate / 1000,
            )} kHz${d.is_default ? " — default" : ""}`,
          }))}
          onChange={(v) =>
            guard(async () => {
              const [host, name] = splitDeviceKey(v);
              // The backend restores this device's saved routing if we've used
              // it before; the 1/2 pad pair here is only the fallback for a
              // device seen for the first time.
              await setAudioOutput(host, name, 0, 1);
              await refreshSettings();
            })
          }
        />
        <div
          style={{
            display: "flex",
            gap: 10,
            alignItems: "center",
            marginTop: 14,
          }}
        >
          <button
            type="button"
            className="btn btn-ghost"
            disabled={!settings.output_device || debugBusy}
            onClick={() =>
              guard(async () => {
                setDebugBusy(true);
                setDebugError(null);
                setDebugReport(null);
                try {
                  setDebugReport(await runAudioOutputTest());
                } catch (e) {
                  setDebugError(e instanceof Error ? e.message : String(e));
                } finally {
                  setDebugBusy(false);
                }
              })
            }
          >
            <Icon name="play" size={15} />
            {debugBusy ? "Testing..." : "Test pad output"}
          </button>
        </div>
        {(debugReport || debugError) && (
          <div
            className="mono"
            style={{
              marginTop: 12,
              padding: 12,
              border: "1px solid var(--line)",
              borderRadius: 8,
              background: "var(--surface-2)",
              color: debugError ? "var(--danger)" : "var(--text-2)",
              fontSize: 12,
              lineHeight: 1.6,
              overflowX: "auto",
            }}
          >
            {debugError ? (
              `audio test failed: ${debugError}`
            ) : debugReport ? (
              <>
                {`host=${debugReport.host} format=${debugReport.sample_format} rate=${debugReport.sample_rate} channels=${debugReport.channels}`}
                <br />
                {`pad=${debugReport.pad_channels[0] + 1}/${debugReport.pad_channels[1] + 1} callbacks=${debugReport.callback_calls} frames=${debugReport.frames_written} nonzero=${debugReport.nonzero_frames} peak=${debugReport.peak.toFixed(3)}`}
              </>
            ) : null}
          </div>
        )}
      </Card>

      <Card pad={22}>
        <div
          style={{
            display: "flex",
            alignItems: "center",
            gap: 9,
            marginBottom: 14,
          }}
        >
          <Icon name="waves" size={17} stroke="var(--accent-ink)" />
          <Eyebrow style={{ letterSpacing: "0.1em" }}>Pad output</Eyebrow>
        </div>

        <RoutingPicker
          channelCount={channelCount}
          channelLeft={settings.channel_left}
          channelRight={settings.channel_right}
          onChange={(l, r) =>
            guard(async () => {
              await setAudioOutput(
                settings.output_host,
                settings.output_device ?? "",
                l,
                r,
              );
              await refreshSettings();
            })
          }
        />

        <div style={{ marginTop: 20 }}>
          <div
            style={{
              display: "flex",
              justifyContent: "space-between",
              marginBottom: 10,
            }}
          >
            <Eyebrow style={{ letterSpacing: "0.08em" }}>Crossfade</Eyebrow>
            <span
              className="mono"
              style={{ fontSize: 12, color: "var(--text-2)" }}
            >
              {(settings.crossfade_ms / 1000).toFixed(1)} s
            </span>
          </div>
          <Slider
            value={settings.crossfade_ms}
            min={200}
            max={8000}
            step={100}
            onChange={(ms) =>
              guard(async () => {
                await setCrossfade(ms);
                await refreshSettings();
              })
            }
          />
          <div
            style={{
              display: "flex",
              justifyContent: "space-between",
              marginTop: 7,
            }}
          >
            <span className="mini-label">Instant</span>
            <span className="mini-label">Slow fade</span>
          </div>
        </div>

        <p className="helper-note">
          Route the pads to a spare output pair on your interface so they don't
          land on your main mix.
        </p>
      </Card>

      <Card pad={22}>
        <div
          style={{
            display: "flex",
            alignItems: "center",
            gap: 9,
            marginBottom: 14,
          }}
        >
          <Icon name="metronome" size={17} stroke="var(--accent-ink)" />
          <Eyebrow style={{ letterSpacing: "0.1em" }}>Click output</Eyebrow>
        </div>

        <RoutingPicker
          channelCount={channelCount}
          channelLeft={settings.click.channel_left}
          channelRight={settings.click.channel_right}
          onChange={(l, r) =>
            guard(async () => {
              await setClickChannels(l, r);
              await refreshSettings();
            })
          }
        />

        <p className="helper-note">
          The click is mono. Use stereo only if your IEM bus expects a stereo
          pair — most click busses are mono.
        </p>
      </Card>

      <Card pad={22}>
        <div
          style={{
            display: "flex",
            alignItems: "center",
            gap: 9,
            marginBottom: 14,
          }}
        >
          <Icon name="mic" size={17} stroke="var(--accent-ink)" />
          <Eyebrow style={{ letterSpacing: "0.1em" }}>Cue output</Eyebrow>
        </div>

        <RoutingPicker
          channelCount={channelCount}
          channelLeft={settings.cues.channel_left}
          channelRight={settings.cues.channel_right}
          onChange={(l, r) =>
            guard(async () => {
              await setCueChannels(l, r);
              await refreshSettings();
            })
          }
        />

        <label className="click-toggle" style={{ marginTop: 16 }}>
          <input
            type="checkbox"
            checked={settings.cues.duck_click}
            onChange={(e) =>
              guard(async () => {
                await setCueDuckClick(e.target.checked);
                await refreshSettings();
              })
            }
          />
          Duck click while speaking
        </label>

        <p className="helper-note">
          Pick a spare output pair for spoken cues — they're meant for the
          band's IEMs, not the main mix. To share the click bus, point this at
          the same channels.
        </p>
      </Card>
    </div>
  );
}
