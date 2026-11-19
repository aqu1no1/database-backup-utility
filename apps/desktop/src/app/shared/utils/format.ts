const UNITS = ['B', 'KB', 'MB', 'GB', 'TB'] as const;

export function formatBytes(bytes: number, locale: string): string {
  let value = Math.max(0, bytes);
  let unit = 0;
  while (value >= 1024 && unit < UNITS.length - 1) {
    value /= 1024;
    unit++;
  }
  const digits = unit === 0 ? 0 : value < 10 ? 2 : value < 100 ? 1 : 0;
  const number = new Intl.NumberFormat(locale, { maximumFractionDigits: digits }).format(value);
  return `${number} ${UNITS[unit]}`;
}

export function formatDuration(milliseconds: number, locale: string): string {
  const total = Math.max(0, milliseconds) / 1000;
  if (total < 10) {
    const seconds = new Intl.NumberFormat(locale, { maximumFractionDigits: 1 }).format(total);
    return `${seconds} s`;
  }
  const seconds = Math.round(total);
  if (seconds < 60) {
    return `${seconds} s`;
  }
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) {
    return `${minutes} min ${pad(seconds % 60)} s`;
  }
  return `${Math.floor(minutes / 60)} h ${pad(minutes % 60)} min`;
}

export function formatDateTime(value: Date | string | number, locale: string): string {
  return new Intl.DateTimeFormat(locale, { dateStyle: 'short', timeStyle: 'short' }).format(
    new Date(value),
  );
}

function pad(value: number): string {
  return value.toString().padStart(2, '0');
}
