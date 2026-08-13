/** "45 MB" / "2.5 GB": download sizes, one implementation. This existed
 * four times (both model pages, onboarding, the accuracy picker) and had
 * nothing keeping the copies in step. */
export function formatBytes(bytes: number | undefined | null): string {
  if (!bytes || bytes <= 0) return '';
  const gb = bytes / 1e9;
  return gb >= 1 ? `${gb.toFixed(1)} GB` : `${Math.round(bytes / 1e6)} MB`;
}
