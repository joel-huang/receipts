/**
 * Lucide "server" icon, shown in front of file paths when the chats come from another machine
 * through `receipts remote`. The tooltip names that machine.
 */
export function RemoteIcon({ remote }: { remote: string | null | undefined }) {
  if (!remote) return null;
  return (
    <span className="remote-icon" title={`On ${remote}`} aria-label={`On ${remote}`}>
      <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
        <rect width="20" height="8" x="2" y="2" rx="2" ry="2" />
        <rect width="20" height="8" x="2" y="14" rx="2" ry="2" />
        <line x1="6" x2="6.01" y1="6" y2="6" />
        <line x1="6" x2="6.01" y1="18" y2="18" />
      </svg>
    </span>
  );
}
