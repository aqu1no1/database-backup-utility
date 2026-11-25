const RECIPIENT = /^age1[qpzry9x8gf2tvdw0s3jn54khce6mua7l]{58}$/;

export type RecipientProblem = 'invalid' | 'duplicate';

export function checkRecipient(
  candidate: string,
  existing: readonly string[],
): RecipientProblem | null {
  if (!RECIPIENT.test(candidate)) {
    return 'invalid';
  }
  return existing.includes(candidate) ? 'duplicate' : null;
}

export function shortKey(key: string): string {
  return key.length <= 19 ? key : `${key.slice(0, 12)}…${key.slice(-6)}`;
}
