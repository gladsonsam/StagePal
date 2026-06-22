// App shell: boot, top-level state, sidebar nav, error banner, modal toggle.
// Each page is a self-contained component under src/components/.

import { useEffect, useMemo, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import {
  getServerUrl,
  getSettings,
  getState,
  listAudioDevices,
  onNowPlaying,
  scanLibrary,
  setClickEnabled,
  type DeviceInfo,
  type NowPlaying,
  type ServerUrl,
  type Settings,
} from "./lib/ipc";
import { baseName } from "./lib/baseName";
import { Icon, Mark, type Note, type PadStyle } from "./components/ui";
import { NavItem } from "./components/NavItem";
import { PadsPage } from "./components/PadsPage";
import { ClickPage } from "./components/ClickPage";
import { CuesPage } from "./components/CuesPage";
import { LibraryPage } from "./components/LibraryPage";
import { SettingsPage } from "./components/SettingsPage";
import { ConnectModal } from "./components/ConnectModal";
import "./App.css";

const PAD_STYLE_KEY = "stagepal.padStyle";
const PAGE_KEY = "stagepal.page";

type Page = "pads" | "click" | "cues" | "library" | "settings";
const PAGES: Page[] = ["pads", "click", "cues", "library", "settings"];

function App() {
  const [settings, setSettings] = useState<Settings | null>(null);
  const [devices, setDevices] = useState<DeviceInfo[]>([]);
  const [now, setNow] = useState<NowPlaying | null>(null);
  const [server, setServer] = useState<ServerUrl | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [connectOpen, setConnectOpen] = useState(false);
  const [page, setPage] = useState<Page>(() => {
    const saved = localStorage.getItem(PAGE_KEY);
    return PAGES.includes(saved as Page) ? (saved as Page) : "pads";
  });
  const [padStyle, setPadStyle] = useState<PadStyle>(
    () => (localStorage.getItem(PAD_STYLE_KEY) as PadStyle) || "grid",
  );

  function navigate(p: Page) {
    setPage(p);
    localStorage.setItem(PAGE_KEY, p);
  }
  function changePadStyle(v: PadStyle) {
    setPadStyle(v);
    localStorage.setItem(PAD_STYLE_KEY, v);
  }

  async function refreshSettings() {
    setSettings(await getSettings());
  }

  useEffect(() => {
    (async () => {
      try {
        setDevices(await listAudioDevices());
        setSettings(await getSettings());
        setNow(await getState());
        setServer(await getServerUrl());
      } catch (e) {
        setError(String(e));
      }
    })();
    const unlisten = onNowPlaying(setNow);
    return () => {
      unlisten.then((u) => u());
    };
  }, []);

  // Keyboard: Space toggles click globally as long as no text input is focused
  // so editing a bank name (etc.) isn't hijacked. Useful from any page.
  useEffect(() => {
    function isTextTarget(el: EventTarget | null) {
      if (!(el instanceof HTMLElement)) return false;
      const tag = el.tagName;
      return tag === "INPUT" || tag === "TEXTAREA" || el.isContentEditable;
    }
    function onKey(e: KeyboardEvent) {
      if (e.repeat || isTextTarget(document.activeElement)) return;
      if (e.code === "Space") {
        e.preventDefault();
        const next = !(now?.click?.enabled ?? false);
        setClickEnabled(next).catch((err) => setError(String(err)));
      }
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [now?.click?.enabled]);

  const activePreset = useMemo(
    () => settings?.presets.find((p) => p.id === settings.active_preset) ?? null,
    [settings],
  );

  const assignments = useMemo<Partial<Record<Note, string>>>(() => {
    const out: Partial<Record<Note, string>> = {};
    for (const [k, path] of Object.entries(activePreset?.files ?? {})) {
      if (path) out[k as Note] = baseName(path);
    }
    return out;
  }, [activePreset]);

  const selectedDevice = devices.find(
    (d) => d.name === settings?.output_device && d.host === settings?.output_host,
  );
  const channelCount = selectedDevice?.channels ?? 2;

  async function guard(fn: () => Promise<unknown>) {
    try {
      setError(null);
      await fn();
    } catch (e) {
      setError(String(e));
    }
  }

  async function chooseFolder() {
    const folder = await open({ directory: true, multiple: false });
    if (typeof folder === "string") {
      await guard(async () => {
        await scanLibrary(folder);
        await refreshSettings();
      });
    }
  }

  const phoneUrl = server
    ? server.ip
      ? `http://${server.ip}:${server.port}`
      : `http://${server.host}.local:${server.port}`
    : "";
  const phoneAddr = server
    ? server.ip
      ? `${server.ip}:${server.port}`
      : `${server.host}.local:${server.port}`
    : "";

  if (!settings) {
    return (
      <div className="app">
        <div className="boot">
          <Mark size={52} />
          <p>Starting the pad server… {error}</p>
        </div>
      </div>
    );
  }

  return (
    <div className="app">
      <header className="app-header">
        <Mark size={42} />
        <div className="app-title">StagePal</div>
        <div style={{ flex: 1 }} />
        <button className="btn btn-ghost" onClick={() => setConnectOpen(true)}>
          <Icon name="phone" size={16} stroke="var(--text-2)" /> Connect phone
        </button>
      </header>

      <div className="app-shell">
        <nav className="sidebar" aria-label="Main navigation">
          <NavItem icon="grid" label="Pads" active={page === "pads"} onClick={() => navigate("pads")} />
          <NavItem icon="metronome" label="Click" active={page === "click"} onClick={() => navigate("click")} />
          <NavItem icon="mic" label="Cues" active={page === "cues"} onClick={() => navigate("cues")} />
          <NavItem icon="folder" label="Library" active={page === "library"} onClick={() => navigate("library")} />
          <NavItem icon="sliders" label="Settings" active={page === "settings"} onClick={() => navigate("settings")} />
        </nav>

        <main className="page">
          {error && <div className="error-banner">{error}</div>}

          {page === "pads" && (
            <PadsPage
              settings={settings}
              now={now}
              padStyle={padStyle}
              onPadStyleChange={changePadStyle}
              activePreset={activePreset}
              assignments={assignments}
              guard={guard}
              onGoLibrary={() => navigate("library")}
            />
          )}
          {page === "click" && (
            <ClickPage settings={settings} click={now?.click ?? null} guard={guard} />
          )}
          {page === "cues" && (
            <CuesPage
              settings={settings}
              speaking={!!now?.cue?.speaking}
              speakingLabel={now?.cue?.label ?? null}
              guard={guard}
              refreshSettings={refreshSettings}
            />
          )}
          {page === "library" && (
            <LibraryPage
              settings={settings}
              onAddFolder={chooseFolder}
              guard={guard}
              refreshSettings={refreshSettings}
            />
          )}
          {page === "settings" && (
            <SettingsPage
              settings={settings}
              devices={devices}
              channelCount={channelCount}
              guard={guard}
              refreshSettings={refreshSettings}
            />
          )}
        </main>
      </div>

      {connectOpen && (
        <ConnectModal
          url={phoneUrl}
          addr={phoneAddr}
          server={server}
          onClose={() => setConnectOpen(false)}
        />
      )}
    </div>
  );
}

export default App;
