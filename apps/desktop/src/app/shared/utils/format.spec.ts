import { formatBytes, formatDateTime, formatDuration } from './format';

describe('formatBytes', () => {
  it.each([
    [0, 'pt-BR', '0 B'],
    [512, 'pt-BR', '512 B'],
    [1536, 'pt-BR', '1,5 KB'],
    [1024 * 1024 * 1024 * 1.24, 'pt-BR', '1,24 GB'],
    [1024 * 1024 * 1024 * 1.24, 'en-US', '1.24 GB'],
    [1024 * 1024 * 512.3, 'pt-BR', '512 MB'],
    [1024 * 1024 * 42.37, 'pt-BR', '42,4 MB'],
  ])('formats %d bytes in %s as %s', (bytes, locale, expected) => {
    expect(formatBytes(bytes, locale)).toBe(expected);
  });

  it('never shows a negative size', () => {
    expect(formatBytes(-10, 'pt-BR')).toBe('0 B');
  });
});

describe('formatDuration', () => {
  it.each([
    [400, 'pt-BR', '0,4 s'],
    [400, 'en-US', '0.4 s'],
    [12_300, 'pt-BR', '12 s'],
    [134_000, 'pt-BR', '2 min 14 s'],
    [3_900_000, 'pt-BR', '1 h 05 min'],
  ])('formats %d ms in %s as %s', (milliseconds, locale, expected) => {
    expect(formatDuration(milliseconds, locale)).toBe(expected);
  });
});

describe('formatDateTime', () => {
  const instant = new Date(2026, 9, 20, 14, 25);

  it('uses the day-first order in pt-BR', () => {
    expect(formatDateTime(instant, 'pt-BR')).toMatch(/^20\/10\/2026,? 14:25$/);
  });

  it('uses the month-first order in en-US', () => {
    expect(formatDateTime(instant, 'en-US')).toMatch(/^10\/20\/26,? 2:25\sPM$/);
  });
});
