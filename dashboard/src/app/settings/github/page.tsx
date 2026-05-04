'use client';

import useSWR from 'swr';
import {
  disconnectGithubOAuth,
  getGithubOAuthStatus,
  startGithubOAuth,
} from '@/lib/api';
import { toast } from '@/components/toast';
import {
  AlertCircle,
  CheckCircle,
  ExternalLink,
  Github,
  Loader,
  RefreshCw,
  Unlink,
} from 'lucide-react';
import { useState } from 'react';

export default function GithubSettingsPage() {
  const { data: status, isLoading, mutate } = useSWR(
    'github-oauth-status',
    getGithubOAuthStatus,
    { revalidateOnFocus: true }
  );
  const [connecting, setConnecting] = useState(false);
  const [disconnecting, setDisconnecting] = useState(false);

  const handleConnect = async () => {
    setConnecting(true);
    const authWindow = window.open('about:blank', '_blank');
    if (authWindow) {
      authWindow.opener = null;
    }
    try {
      const response = await startGithubOAuth();
      if (authWindow) {
        authWindow.location.href = response.url;
      } else {
        window.location.href = response.url;
      }
      toast.success('GitHub authorization opened');
      setTimeout(() => void mutate(), 2000);
    } catch (error) {
      authWindow?.close();
      toast.error(error instanceof Error ? error.message : 'Failed to start GitHub OAuth');
    } finally {
      setConnecting(false);
    }
  };

  const handleDisconnect = async () => {
    setDisconnecting(true);
    try {
      await disconnectGithubOAuth();
      await mutate();
      toast.success('GitHub account disconnected');
    } catch (error) {
      toast.error(error instanceof Error ? error.message : 'Failed to disconnect GitHub');
    } finally {
      setDisconnecting(false);
    }
  };

  const blockedReason = status
    ? !status.configured
      ? 'Set GITHUB_OAUTH_CLIENT_ID and GITHUB_OAUTH_CLIENT_SECRET on the server.'
      : !status.can_decrypt
        ? 'Unlock the secrets store or set SANDBOXED_SECRET_PASSPHRASE.'
        : status.message
    : null;

  return (
    <div className="h-full overflow-y-auto p-6">
      <div className="mx-auto max-w-3xl space-y-6">
        <div>
          <h1 className="text-xl font-semibold text-white">GitHub</h1>
          <p className="mt-1 text-sm text-white/50">
            Connect the current sandbox user to GitHub for mission and Telegram bot git operations.
          </p>
        </div>

        <section className="rounded-xl border border-white/[0.08] bg-white/[0.03] p-5">
          <div className="flex items-start justify-between gap-4">
            <div className="flex min-w-0 items-start gap-3">
              <div className="flex h-10 w-10 shrink-0 items-center justify-center rounded-lg bg-white/[0.06]">
                <Github className="h-5 w-5 text-white/80" />
              </div>
              <div className="min-w-0">
                <div className="flex items-center gap-2">
                  <h2 className="text-sm font-medium text-white">User OAuth Account</h2>
                  {isLoading ? (
                    <Loader className="h-4 w-4 animate-spin text-white/40" />
                  ) : status?.connected ? (
                    <CheckCircle className="h-4 w-4 text-emerald-400" />
                  ) : (
                    <AlertCircle className="h-4 w-4 text-amber-400" />
                  )}
                </div>
                <p className="mt-1 text-sm text-white/50">
                  {status?.connected
                    ? `Connected as @${status.login ?? 'github-user'}`
                    : 'No GitHub account connected for this sandbox user.'}
                </p>
              </div>
            </div>

            <button
              type="button"
              onClick={() => void mutate()}
              className="inline-flex h-9 items-center gap-2 rounded-lg border border-white/[0.08] px-3 text-xs text-white/70 hover:bg-white/[0.06]"
            >
              <RefreshCw className="h-3.5 w-3.5" />
              Refresh
            </button>
          </div>

          {status?.connected && (
            <dl className="mt-5 grid gap-3 rounded-lg border border-white/[0.06] bg-black/20 p-4 text-sm sm:grid-cols-2">
              <div>
                <dt className="text-white/35">Account</dt>
                <dd className="mt-1 truncate text-white">{status.login}</dd>
              </div>
              <div>
                <dt className="text-white/35">Email</dt>
                <dd className="mt-1 truncate text-white">
                  {status.email ?? 'GitHub noreply fallback'}
                </dd>
              </div>
              <div>
                <dt className="text-white/35">Scopes</dt>
                <dd className="mt-1 truncate text-white">
                  {status.scopes ?? 'Default OAuth scopes'}
                </dd>
              </div>
              <div>
                <dt className="text-white/35">Connected</dt>
                <dd className="mt-1 truncate text-white">
                  {status.connected_at
                    ? new Date(status.connected_at).toLocaleString()
                    : 'Unknown'}
                </dd>
              </div>
            </dl>
          )}

          {blockedReason && !status?.connected && (
            <div className="mt-5 rounded-lg border border-amber-400/20 bg-amber-400/10 px-4 py-3 text-sm text-amber-100">
              {blockedReason}
            </div>
          )}

          <div className="mt-5 flex flex-wrap gap-3">
            <button
              type="button"
              onClick={handleConnect}
              disabled={connecting || !status?.configured || !status?.can_decrypt}
              className="inline-flex items-center gap-2 rounded-lg bg-white px-4 py-2 text-sm font-medium text-black transition hover:bg-white/90 disabled:cursor-not-allowed disabled:opacity-50"
            >
              {connecting ? (
                <Loader className="h-4 w-4 animate-spin" />
              ) : (
                <ExternalLink className="h-4 w-4" />
              )}
              {status?.connected ? 'Reconnect GitHub' : 'Connect GitHub'}
            </button>

            {status?.connected && (
              <button
                type="button"
                onClick={handleDisconnect}
                disabled={disconnecting}
                className="inline-flex items-center gap-2 rounded-lg border border-red-400/20 px-4 py-2 text-sm font-medium text-red-200 transition hover:bg-red-400/10 disabled:cursor-not-allowed disabled:opacity-50"
              >
                {disconnecting ? (
                  <Loader className="h-4 w-4 animate-spin" />
                ) : (
                  <Unlink className="h-4 w-4" />
                )}
                Disconnect
              </button>
            )}
          </div>
        </section>
      </div>
    </div>
  );
}
