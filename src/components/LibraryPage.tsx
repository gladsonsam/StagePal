// Library page — pad banks, each expandable to key→file mappings plus a
// resolver for files the scanner couldn't auto-key.

import { useRef, useState } from "react";
import {
  ALL_KEYS,
  assignKey,
  clearKey,
  removePreset,
  renamePreset,
  setPreset,
  type Key,
  type Preset,
  type Settings,
} from "../lib/ipc";
import { baseName } from "../lib/baseName";
import { Card, Eyebrow, Icon, NOTES, SelectField } from "./ui";

interface Props {
  settings: Settings;
  onAddFolder: () => Promise<void> | void;
  guard: (fn: () => Promise<unknown>) => Promise<void>;
  refreshSettings: () => Promise<void>;
}

export function LibraryPage({ settings, onAddFolder, guard, refreshSettings }: Props) {
  return (
    <Card pad={24}>
      <div style={{ display: "flex", alignItems: "center", gap: 9, marginBottom: 16 }}>
        <Icon name="folder" size={17} stroke="var(--accent-ink)" />
        <Eyebrow style={{ letterSpacing: "0.1em" }}>Pad library</Eyebrow>
        <div style={{ flex: 1 }} />
        <button className="btn btn-ghost" onClick={onAddFolder}>
          <Icon name="plus" size={15} stroke="var(--text-2)" /> Add pad folder
        </button>
      </div>

      {settings.presets.length === 0 ? (
        <p className="empty-note">
          No pads yet. Add a folder of audio files — keys are detected from the file names
          (e.g. <code>C.wav</code>, <code>F# Pad.mp3</code>).
        </p>
      ) : (
        <div style={{ display: "flex", flexDirection: "column", gap: 12 }}>
          {settings.presets.map((p) => (
            <Bank
              key={p.id}
              preset={p}
              active={p.id === settings.active_preset}
              onActivate={() =>
                guard(async () => {
                  await setPreset(p.id);
                  await refreshSettings();
                })
              }
              onRemove={() =>
                guard(async () => {
                  await removePreset(p.id);
                  await refreshSettings();
                })
              }
              onRename={(name) =>
                guard(async () => {
                  await renamePreset(p.id, name);
                  await refreshSettings();
                })
              }
              onAssign={(key, path) =>
                guard(async () => {
                  await assignKey(p.id, key, path);
                  await refreshSettings();
                })
              }
              onClear={(key) =>
                guard(async () => {
                  await clearKey(p.id, key);
                  await refreshSettings();
                })
              }
            />
          ))}
        </div>
      )}
    </Card>
  );
}

/** One pad bank: header + collapsible slot grid + unmapped resolver. */
function Bank({
  preset,
  active,
  onActivate,
  onRemove,
  onRename,
  onAssign,
  onClear,
}: {
  preset: Preset;
  active: boolean;
  onActivate: () => void;
  onRemove: () => void;
  onRename: (name: string) => void;
  onAssign: (key: Key, path: string) => void;
  onClear: (key: Key) => void;
}) {
  const [open, setOpen] = useState(active);
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(preset.name);
  const inputRef = useRef<HTMLInputElement>(null);

  const files = preset.files;
  const count = Object.keys(files).length;
  const unmapped = preset.unmapped ?? [];

  function commitName() {
    setEditing(false);
    const name = draft.trim();
    if (name && name !== preset.name) onRename(name);
    else setDraft(preset.name);
  }

  return (
    <div className="bank">
      <div className={`bank-head${open ? " open" : ""}`}>
        <button
          className={`bank-dot${active ? " on" : ""}`}
          title={active ? "Active bank" : "Make active"}
          onClick={onActivate}
        />
        {editing ? (
          <input
            ref={inputRef}
            className="bank-name-input"
            value={draft}
            autoFocus
            onChange={(e) => setDraft(e.target.value)}
            onBlur={commitName}
            onKeyDown={(e) => {
              if (e.key === "Enter") commitName();
              if (e.key === "Escape") {
                setDraft(preset.name);
                setEditing(false);
              }
            }}
          />
        ) : (
          <button className="bank-name" onDoubleClick={() => setEditing(true)} onClick={onActivate}>
            {preset.name}
          </button>
        )}
        <span className={`bank-pill${count === 12 ? " full" : ""}`}>{count}/12 keys</span>
        <div style={{ flex: 1 }} />
        <div style={{ display: "flex", gap: 6 }}>
          <button className="icon-btn" title="Rename" onClick={() => setEditing(true)}>
            <Icon name="pencil" size={15} stroke="var(--text-2)" />
          </button>
          <button className="icon-btn" title="Remove" onClick={onRemove}>
            <Icon name="trash" size={15} stroke="var(--text-2)" />
          </button>
        </div>
        <button
          className="bank-chevron"
          title={open ? "Collapse" : "Expand"}
          onClick={() => setOpen((o) => !o)}
        >
          <Icon name={open ? "chevron" : "chevronR"} size={16} stroke="var(--text-3)" />
        </button>
      </div>

      {open && (
        <div className="bank-body">
          <div className="slot-grid">
            {NOTES.map((n) => {
              const path = files[n];
              return (
                <div key={n} className={`slot${path ? "" : " empty"}`}>
                  <span className="slot-key">{n}</span>
                  <span className="slot-file" title={path}>
                    {path ? baseName(path) : "empty"}
                  </span>
                  {path && (
                    <button
                      className="slot-clear"
                      title="Unmap this key"
                      onClick={() => onClear(n as Key)}
                    >
                      <Icon name="x" size={13} stroke="currentColor" />
                    </button>
                  )}
                </div>
              );
            })}
          </div>

          {unmapped.length > 0 && (
            <div className="resolver">
              <div className="resolver-head">
                <strong>
                  {unmapped.length} file{unmapped.length > 1 ? "s" : ""} need a key.
                </strong>{" "}
                The filename didn't match — assign each one.
              </div>
              {unmapped.map((path) => (
                <div key={path} className="resolver-row">
                  <Icon name="waves" size={16} stroke="var(--text-3)" />
                  <span className="resolver-file" title={path}>
                    {baseName(path)}
                  </span>
                  <SelectField
                    style={{ width: 150 }}
                    value=""
                    placeholder="Assign to…"
                    options={ALL_KEYS.map((k) => ({
                      value: k,
                      label: files[k] ? `${k} (replace)` : k,
                    }))}
                    onChange={(v) => onAssign(v as Key, path)}
                  />
                </div>
              ))}
            </div>
          )}
        </div>
      )}
    </div>
  );
}
