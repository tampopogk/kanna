/**
 * Case- and whitespace-insensitive account identity.
 *
 * Shared by the account sheet's E2E-only identity marker and the billing
 * review lane that compares that marker with the selected reviewer account.
 * Kept free of React Native imports so the Node-side E2E runner can load it.
 */
export function normalizeAccountIdentity(email: string): string {
  return email.trim().toLowerCase();
}
