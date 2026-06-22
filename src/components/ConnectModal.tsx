// Connect-phone modal — QR code + address pill + copy button.

import { useState } from "react";
import { QRCodeSVG } from "qrcode.react";
import type { ServerUrl } from "../lib/ipc";
import { Icon } from "./ui";

interface Props {
  url: string;
  addr: string;
  server: ServerUrl | null;
  onClose: () => void;
}

export function ConnectModal({ url, addr, server, onClose }: Props) {
  const [copied, setCopied] = useState(false);

  async function copy() {
    try {
      await navigator.clipboard.writeText(addr);
      setCopied(true);
      setTimeout(() => setCopied(false), 1400);
    } catch {
      /* clipboard unavailable */
    }
  }

  return (
    <div
      className="scrim"
      onClick={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <div className="modal" role="dialog" aria-modal="true" aria-label="Connect your phone">
        <button
          className="icon-btn"
          style={{ position: "absolute", top: 16, right: 16 }}
          title="Close"
          onClick={onClose}
        >
          <Icon name="x" size={15} stroke="var(--text-3)" />
        </button>

        <div className="modal-icon">
          <Icon name="wifi" size={22} stroke="var(--accent-ink)" />
        </div>
        <div className="modal-title">Connect your phone</div>
        <p className="modal-sub">
          Scan the code, or open the address in any browser on the same Wi-Fi.
        </p>

        <div className="qr">
          {url ? (
            <QRCodeSVG
              value={url}
              size={148}
              bgColor="transparent"
              fgColor="var(--text)"
              level="M"
            />
          ) : (
            <span className="mini-label">finding address…</span>
          )}
        </div>

        <div className="addr-pill">
          <span className="addr">{addr || "…"}</span>
          <button className="icon-btn" style={{ width: 38, height: 38 }} title="Copy" onClick={copy}>
            <Icon name={copied ? "check" : "copy"} size={16} stroke="var(--text-2)" />
          </button>
        </div>
        {server?.ip && (
          <div className="addr-sub">
            or {server.host}.local:{server.port}
          </div>
        )}
      </div>
    </div>
  );
}
