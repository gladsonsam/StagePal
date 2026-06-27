import { useEffect, useState, type ComponentType, type ReactElement } from "react";
import { useRemoteState } from "./hooks/useRemoteState";
import {
  ClickTabIcon,
  CuesTabIcon,
  MarkLogoIcon,
  PadsTabIcon,
} from "./components/icons";
import { PadsTab, type PadStyle } from "./components/PadsTab";
import { ClickTab } from "./components/ClickTab";
import { CuesTab } from "./components/CuesTab";
import type { NowPlaying } from "../shared/types";
import type { Info } from "./api";

type Tab = "pads" | "click" | "cues";

const STYLE_KEY = "stagepal.remote.padStyle";
const TAB_KEY = "stagepal.remote.tab";

/** Passed to each tab's renderer. */
interface TabContext {
  info: Info | null;
  now: NowPlaying;
  padStyle: PadStyle;
  onPadStyle: (s: PadStyle) => void;
  refreshInfo: () => Promise<void>;
}

interface TabDef {
  key: Tab;
  label: string;
  Icon: ComponentType;
  render: (ctx: TabContext) => ReactElement;
}

// Single source of truth for tabs.
const TABS: TabDef[] = [
  {
    key: "pads",
    label: "Pads",
    Icon: PadsTabIcon,
    render: ({ info, now, padStyle, onPadStyle }) => (
      <PadsTab info={info} now={now} padStyle={padStyle} onPadStyle={onPadStyle} />
    ),
  },
  {
    key: "click",
    label: "Click",
    Icon: ClickTabIcon,
    render: ({ now }) => <ClickTab now={now} />,
  },
  {
    key: "cues",
    label: "Cues",
    Icon: CuesTabIcon,
    render: ({ info, now, refreshInfo }) => (
      <CuesTab info={info} now={now} refreshInfo={refreshInfo} />
    ),
  },
];

function loadPadStyle(): PadStyle {
  const v = localStorage.getItem(STYLE_KEY);
  return v === "piano" ? "piano" : "grid";
}
function loadTab(): Tab {
  const v = localStorage.getItem(TAB_KEY);
  return TABS.some((t) => t.key === v) ? (v as Tab) : "pads";
}

export default function App() {
  const { info, now, conn, refreshInfo } = useRemoteState();
  const [tab, setTab] = useState<Tab>(loadTab);
  const [padStyle, setPadStyle] = useState<PadStyle>(loadPadStyle);

  useEffect(() => {
    localStorage.setItem(TAB_KEY, tab);
  }, [tab]);
  useEffect(() => {
    localStorage.setItem(STYLE_KEY, padStyle);
  }, [padStyle]);

  const active = TABS.find((t) => t.key === tab) ?? TABS[0];
  const ctx: TabContext = { info, now, padStyle, onPadStyle: setPadStyle, refreshInfo };

  return (
    <>
      <header>
        <span className="mark" aria-hidden>
          <MarkLogoIcon />
        </span>
        <h1>StagePal</h1>
        <span
          className={`status${conn === "reconnecting" ? " reconnecting" : ""}`}
          title="connection"
        />
      </header>

      <div className="tab-bar" role="tablist">
        {TABS.map((t) => (
          <button
            key={t.key}
            type="button"
            role="tab"
            className={tab === t.key ? "on" : ""}
            onClick={() => setTab(t.key)}
          >
            <t.Icon />
            <span className="tab-label">{t.label}</span>
          </button>
        ))}
      </div>

      {active.render(ctx)}
    </>
  );
}
