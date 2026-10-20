export type AlertDetails = {
  title: string;
  description: string;
  severity: 'warning' | 'error';
};

const alertDetails: Record<number, AlertDetails> = {
  1: { title: 'Overtemperature', description: 'Device temperature exceeded the safe limit.', severity: 'error' },
  2: { title: 'VBUS undervoltage', description: 'VBUS voltage is below the expected level.', severity: 'warning' },
  3: { title: 'DC input overvoltage', description: 'DC input voltage exceeded the safe limit.', severity: 'error' },
  4: { title: 'Voltage calibration failed', description: 'Voltage calibration could not be completed.', severity: 'error' },
  5: { title: 'Output overvoltage', description: 'Output voltage exceeded the configured limit.', severity: 'error' },
  6: { title: 'Overcurrent', description: 'Output switched off (cutoff mode).', severity: 'error' },
  7: { title: 'Load switch overload', description: 'The load switch exceeded its safe operating limit.', severity: 'error' },
  8: { title: 'DC/DC overload', description: 'The DC/DC converter exceeded its safe operating limit.', severity: 'error' },
  9: { title: 'Pre-regulator overload', description: 'The pre-regulator exceeded its safe operating limit.', severity: 'error' },
  10: { title: 'Sink overload', description: 'The sink exceeded its safe operating limit.', severity: 'error' },
  11: { title: 'Negative internal resistance', description: 'The device detected negative internal resistance.', severity: 'error' },
  12: { title: 'Bad firmware', description: 'The device firmware is invalid or incompatible.', severity: 'error' },
};

export function describeAlert(id: number): AlertDetails {
  return alertDetails[id] ?? {
    title: `Device alert ${id}`,
    description: 'The device reported an unspecified alert.',
    severity: 'error',
  };
}
