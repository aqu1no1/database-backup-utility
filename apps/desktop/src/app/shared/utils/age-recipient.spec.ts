import { checkRecipient, shortKey } from './age-recipient';

const KEY = 'age1ql3z7hjy54pw3hyww5ayyfg7zqgvc7w3j2elw8zmrj2kg5sfn9aqmcac8p';
const OTHER = 'age1lggyhqrw2nlhcxprm67z43rta597azn8gknawjehu9d9dl0jq3yqqvfafg';

describe('checkRecipient', () => {
  it('accepts a well-formed X25519 recipient that is not in the list yet', () => {
    expect(checkRecipient(KEY, [OTHER])).toBeNull();
  });

  it('rejects a recipient that is already in the list', () => {
    expect(checkRecipient(KEY, [OTHER, KEY])).toBe('duplicate');
  });

  it.each([
    ['empty text', ''],
    ['a private key', 'AGE-SECRET-KEY-1QQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQ'],
    ['an SSH key', 'ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAI'],
    ['a truncated recipient', KEY.slice(0, 40)],
    ['upper case', KEY.toUpperCase()],
    ['a character outside bech32', `${KEY.slice(0, -1)}b`],
  ])('rejects %s', (_, candidate) => {
    expect(checkRecipient(candidate, [])).toBe('invalid');
  });
});

describe('shortKey', () => {
  it('keeps the first 12 and last 6 characters of a long key', () => {
    expect(shortKey(KEY)).toBe('age1ql3z7hjy…mcac8p');
  });

  it('leaves short values untouched', () => {
    expect(shortKey('age1short')).toBe('age1short');
  });
});
