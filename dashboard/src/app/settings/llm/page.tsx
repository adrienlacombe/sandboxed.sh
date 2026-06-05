'use client';

import { useState } from 'react';
import Link from 'next/link';
import useSWR from 'swr';
import { toast } from '@/components/toast';
import {
  getLlmRoles,
  getSettings,
  updateSettings,
  type LlmRoleStatus,
} from '@/lib/api';
import {
  ArrowUpRight,
  Check,
  Key,
  Loader,
  RotateCcw,
  Sparkles,
  Type,
} from 'lucide-react';

const SOURCE_LABELS: Record<string, string> = {
  settings: 'Custom',
  env: 'Env override',
  auto: 'Auto',
};

/** Compact resolved provider/model chip with a real availability state. */
function RoleStatusChip({
  role,
  loading,
}: {
  role: LlmRoleStatus | undefined;
  loading: boolean;
}) {
  if (loading) {
    return (
      <div className="inline-flex items-center gap-2 rounded-lg border border-white/[0.06] bg-white/[0.02] px-2.5 py-1.5">
        <Loader className="h-3 w-3 animate-spin text-white/40" />
        <span className="text-xs text-white/40">Resolving...</span>
      </div>
    );
  }
  if (!role?.available) {
    return (
      <div className="inline-flex items-center gap-2 rounded-lg border border-amber-500/20 bg-amber-500/5 px-2.5 py-1.5">
        <span className="h-1.5 w-1.5 rounded-full bg-amber-400" />
        <span className="text-xs text-amber-400/90">No provider available</span>
      </div>
    );
  }
  return (
    <div className="inline-flex items-center gap-2 rounded-lg border border-white/[0.06] bg-white/[0.02] px-2.5 py-1.5">
      <span className="h-1.5 w-1.5 rounded-full bg-emerald-400" />
      <span className="text-xs text-white/70">{role.provider}</span>
      <span className="text-white/20">/</span>
      <span className="font-mono text-xs text-white/70">{role.model}</span>
    </div>
  );
}

export default function LLMSettingsPage() {
  const {
    data: serverSettings,
    isLoading: settingsLoading,
    mutate: mutateSettings,
  } = useSWR('settings', getSettings, { revalidateOnFocus: false });
  const {
    data: roles,
    isLoading: rolesLoading,
    mutate: mutateRoles,
  } = useSWR('llm-roles', getLlmRoles, { revalidateOnFocus: false });

  const [askModelValue, setAskModelValue] = useState<string | null>(null);
  const [savingAskModel, setSavingAskModel] = useState(false);

  const savedModel = serverSettings?.ask_assistant_model ?? '';
  const effectiveValue = askModelValue ?? savedModel;
  const dirty = askModelValue !== null && askModelValue.trim() !== savedModel;

  const saveAskModel = async (value: string) => {
    setSavingAskModel(true);
    try {
      const trimmed = value.trim();
      // Send "" (not null) to clear: a present empty string is normalized to
      // None server-side, whereas JSON null is treated as "no change".
      await updateSettings({ ask_assistant_model: trimmed });
      setAskModelValue(null);
      mutateSettings();
      mutateRoles();
      toast.success(
        trimmed ? 'Assistant model updated' : 'Assistant model reset to default'
      );
    } catch (err) {
      toast.error(
        `Failed to save: ${err instanceof Error ? err.message : 'Unknown error'}`
      );
    } finally {
      setSavingAskModel(false);
    }
  };

  const anyUnavailable =
    !rolesLoading && roles && (!roles.assistant.available || !roles.metadata.available);

  return (
    <div className="flex-1 flex flex-col items-center p-6 overflow-auto">
      <div className="w-full max-w-4xl space-y-6">
        <header>
          <h1 className="text-xl font-semibold text-white">LLM</h1>
          <p className="mt-1 text-sm text-white/50">
            Server-side models powering the Ask assistant and mission metadata
          </p>
        </header>

        <div className="space-y-5">
          <div className="grid gap-5 md:grid-cols-2">
            {/* Ask Assistant */}
            <div className="rounded-xl bg-white/[0.02] border border-white/[0.04] p-5 flex flex-col">
              <div className="flex items-start justify-between gap-3 mb-4">
                <div className="flex items-center gap-3 min-w-0">
                  <div className="flex h-10 w-10 items-center justify-center rounded-xl bg-sky-500/10 flex-shrink-0">
                    <Sparkles className="h-5 w-5 text-sky-400" />
                  </div>
                  <div className="min-w-0">
                    <h2 className="text-sm font-medium text-white">
                      Ask Assistant
                    </h2>
                    <p className="text-xs text-white/40">
                      Sidecar co-pilot for missions
                    </p>
                  </div>
                </div>
                {!rolesLoading && roles && (
                  <span className="rounded-md bg-white/[0.04] px-2 py-0.5 text-[10px] font-medium uppercase tracking-wide text-white/40 flex-shrink-0">
                    {SOURCE_LABELS[roles.assistant_source] ?? roles.assistant_source}
                  </span>
                )}
              </div>

              <div className="mb-4">
                <RoleStatusChip role={roles?.assistant} loading={rolesLoading} />
              </div>

              <div className="mt-auto">
                <label className="block text-xs font-medium text-white/60 mb-1.5">
                  Model ID
                </label>
                {settingsLoading ? (
                  <div className="flex items-center gap-2 py-2.5">
                    <Loader className="h-4 w-4 animate-spin text-white/40" />
                    <span className="text-sm text-white/40">Loading...</span>
                  </div>
                ) : (
                  <div className="space-y-2">
                    <input
                      type="text"
                      value={effectiveValue}
                      onChange={(e) => setAskModelValue(e.target.value)}
                      placeholder="gpt-oss-120b"
                      className="w-full rounded-lg border border-white/[0.06] bg-white/[0.02] px-3 py-2 text-sm text-white font-mono placeholder:text-white/20 focus:outline-none focus:border-sky-500/50"
                      onKeyDown={(e) => {
                        if (e.key === 'Enter') saveAskModel(effectiveValue);
                      }}
                    />
                    <div className="flex items-center gap-2">
                      <button
                        onClick={() => saveAskModel(effectiveValue)}
                        disabled={savingAskModel || !dirty}
                        className="flex items-center gap-1.5 rounded-lg bg-indigo-500 px-3 py-1.5 text-xs text-white hover:bg-indigo-600 transition-colors cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed"
                      >
                        {savingAskModel ? (
                          <Loader className="h-3 w-3 animate-spin" />
                        ) : (
                          <Check className="h-3 w-3" />
                        )}
                        Save
                      </button>
                      {savedModel && (
                        <button
                          onClick={() => saveAskModel('')}
                          disabled={savingAskModel}
                          className="flex items-center gap-1.5 rounded-lg border border-white/[0.06] px-3 py-1.5 text-xs text-white/60 hover:bg-white/[0.04] transition-colors cursor-pointer disabled:opacity-50"
                        >
                          <RotateCcw className="h-3 w-3" />
                          Reset to default
                        </button>
                      )}
                    </div>
                    <p className="text-xs text-white/30">
                      Leave blank for the default (gpt-oss-120b). Served via
                      Cerebras when configured, e.g. zai-glm-4.7 for a larger,
                      slower model.
                    </p>
                  </div>
                )}
              </div>
            </div>

            {/* Mission metadata */}
            <div className="rounded-xl bg-white/[0.02] border border-white/[0.04] p-5 flex flex-col">
              <div className="flex items-center gap-3 mb-4">
                <div className="flex h-10 w-10 items-center justify-center rounded-xl bg-amber-500/10 flex-shrink-0">
                  <Type className="h-5 w-5 text-amber-400" />
                </div>
                <div className="min-w-0">
                  <h2 className="text-sm font-medium text-white">
                    Mission Titles & Status
                  </h2>
                  <p className="text-xs text-white/40">
                    Summarizes missions after each turn
                  </p>
                </div>
              </div>

              <div className="mb-4">
                <RoleStatusChip role={roles?.metadata} loading={rolesLoading} />
              </div>

              <p className="text-xs text-white/40 leading-relaxed">
                Generated server-side from conversation history. The model is
                picked automatically from your configured providers, fastest
                first, and follows provider changes without a restart.
              </p>
            </div>
          </div>

          {/* Providers */}
          <div className="rounded-xl bg-white/[0.02] border border-white/[0.04] p-5">
            <div className="flex flex-wrap items-center justify-between gap-4">
              <div className="flex items-center gap-3 min-w-0">
                <div className="flex h-10 w-10 items-center justify-center rounded-xl bg-indigo-500/10 flex-shrink-0">
                  <Key className="h-5 w-5 text-indigo-400" />
                </div>
                <div className="min-w-0">
                  <h2 className="text-sm font-medium text-white">Providers</h2>
                  <p className="text-xs text-white/40">
                    Both roles need an OpenAI-compatible provider: Cerebras is
                    preferred, then OpenRouter, Groq, OpenAI, or Gemini.
                  </p>
                </div>
              </div>
              <Link
                href="/settings/providers"
                className="flex items-center gap-1.5 rounded-lg border border-white/[0.08] bg-white/[0.02] px-3 py-1.5 text-xs text-white/70 hover:bg-white/[0.04] transition-colors flex-shrink-0"
              >
                Manage providers
                <ArrowUpRight className="h-3 w-3" />
              </Link>
            </div>

            {anyUnavailable && (
              <p className="mt-3 rounded-lg border border-amber-500/20 bg-amber-500/5 px-3 py-2 text-xs text-amber-400/90">
                {!roles?.assistant.available && !roles?.metadata.available
                  ? 'No usable provider found: the Ask assistant and title generation are disabled until one is configured.'
                  : !roles?.assistant.available
                    ? 'No usable provider found for the Ask assistant; it is disabled until one is configured.'
                    : 'No usable provider found for mission titles; they fall back to raw text until one is configured.'}
              </p>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}
