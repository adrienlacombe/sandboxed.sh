'use client';

import { useEffect } from 'react';
import { useRouter } from 'next/navigation';

export default function TelegramSettingsRedirect() {
  const router = useRouter();

  useEffect(() => {
    router.replace('/assistant');
  }, [router]);

  return (
    <div className="flex h-full items-center justify-center">
      <div className="text-sm text-white/40">Redirecting to Assistant...</div>
    </div>
  );
}
