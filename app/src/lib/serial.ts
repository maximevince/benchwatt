import type { SerialSettings } from './backend';

/** How a serial port is opened unless the user says otherwise. */
export const defaultSerialSettings: SerialSettings = { baud: 115200, data_bits: 8, parity: 'none', stop_bits: 1, flow: 'none', dtr: true, rts: true };
