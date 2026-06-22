// Four-bar "playing" indicator; each bar pulses on its own delay.

const BARS = [0, 1, 2, 3];

export function Meter() {
  return (
    <span className="meter">
      {BARS.map((i) => {
        const dur = 2.4 + (i % 4) * 0.5;
        const delay = i * 0.18;
        return (
          <span
            key={i}
            className="bar"
            style={{ animation: `meterPulse ${dur}s ease-in-out ${delay}s infinite` }}
          />
        );
      })}
    </span>
  );
}
