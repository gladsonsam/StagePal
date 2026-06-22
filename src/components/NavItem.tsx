// Sidebar nav entry.

import { Icon, type IconName } from "./ui";

export function NavItem({
  icon,
  label,
  active,
  onClick,
}: {
  icon: IconName;
  label: string;
  active: boolean;
  onClick: () => void;
}) {
  return (
    <button className={`nav-item${active ? " on" : ""}`} onClick={onClick}>
      <Icon
        name={icon}
        size={17}
        stroke={active ? "var(--accent-ink)" : "var(--text-3)"}
      />
      <span>{label}</span>
    </button>
  );
}
