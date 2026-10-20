export type Reading = { number: string; unit: string };
const ranges = {
  current: [['A', 1], ['mA', 1e-3], ['µA', 1e-6], ['nA', 1e-9]],
  power: [['W', 1], ['mW', 1e-3], ['µW', 1e-6], ['nW', 1e-9]],
  voltage: [['V', 1], ['mV', 1e-3], ['µV', 1e-6]],
} as const;
export function formatReading(value: number | null | undefined, kind: keyof typeof ranges): Reading {
  const units = ranges[kind];
  if (value == null || !Number.isFinite(value)) return { number: '—'.padStart(8), unit: units[0][0] };
  const idle = kind === 'voltage' ? units[0] : units[1];
  const [unit, scale] = value === 0 ? idle : units.find(([, factor]) => Math.abs(value) >= factor) ?? units[units.length - 1];
  // Fixed decimal width keeps the readout aligned while the magnitude or unit changes.
  const scaled = value / scale;
  const decimals = kind === 'voltage' && scale === 1 ? 4 : Math.abs(scaled) >= 100 ? 1 : Math.abs(scaled) >= 10 ? 2 : 3;
  return { number: scaled.toFixed(decimals).padStart(8), unit };
}
