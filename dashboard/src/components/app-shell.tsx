'use client';

import { useState } from 'react';
import { usePathname } from 'next/navigation';
import { List } from '@phosphor-icons/react';
import { Sidebar } from '@/components/sidebar';
import { BrainLogo } from '@/components/icons';

/**
 * Authenticated app shell. On desktop (lg+) it renders exactly the original
 * layout: a fixed sidebar plus a left-margined <main>. Below lg the sidebar
 * collapses into an off-canvas drawer toggled from a mobile top bar, so the
 * 224px sidebar no longer permanently eats the viewport on phones.
 */
export function AppShell({ children }: { children: React.ReactNode }) {
  const [navOpen, setNavOpen] = useState(false);
  const pathname = usePathname();
  const [lastPathname, setLastPathname] = useState(pathname);

  // Close the drawer whenever the route changes so navigating from it doesn't
  // leave the overlay covering the destination page. Adjusting state during
  // render (rather than in an effect) avoids an extra commit and satisfies the
  // react-hooks/set-state-in-effect rule.
  if (pathname !== lastPathname) {
    setLastPathname(pathname);
    setNavOpen(false);
  }

  return (
    <>
      {/* Mobile top bar — never rendered at lg+, so desktop is untouched. */}
      <div className="lg:hidden sticky top-0 z-30 flex h-12 items-center gap-3 border-b border-white/[0.06] glass-panel px-4">
        <button
          type="button"
          onClick={() => setNavOpen(true)}
          aria-label="Open navigation"
          className="flex h-8 w-8 items-center justify-center rounded-lg text-white/70 transition-colors hover:bg-white/[0.06] hover:text-white"
        >
          <List className="h-5 w-5" />
        </button>
        <BrainLogo size={24} />
        <span className="text-sm font-medium text-white">Sandboxed.sh</span>
      </div>

      {/* Backdrop behind the open drawer (mobile only). */}
      {navOpen && (
        <div
          className="lg:hidden fixed inset-0 z-30 bg-black/50 backdrop-blur-sm"
          onClick={() => setNavOpen(false)}
          aria-hidden="true"
        />
      )}

      <Sidebar open={navOpen} onClose={() => setNavOpen(false)} />

      <main className="lg:ml-56 min-h-[calc(100vh-3rem)] lg:min-h-screen">
        {children}
      </main>
    </>
  );
}
