'use client';

import { useState, useEffect } from 'react';
import useSWR from 'swr';
import {
  listMissions,
  type Mission,
  listTelegramChannels,
  createTelegramChannel,
  updateTelegramChannel,
  deleteTelegramChannel,
  type TelegramChannel,
  type TelegramTriggerMode,
} from '@/lib/api';
import {
  MessageCircle,
  Plus,
  Trash2,
  Loader,
  Power,
  PowerOff,
  Bot,
  ChevronDown,
  ChevronUp,
  Settings,
} from 'lucide-react';
import { cn } from '@/lib/utils';
import { toast } from '@/components/toast';

const TRIGGER_MODE_LABELS: Record<TelegramTriggerMode, string> = {
  all: 'All messages',
  bot_mention: 'Bot @mentions only',
  reply: 'Replies to bot only',
  direct_message: 'Direct messages only',
};

export default function TelegramSettingsPage() {
  const { data: missions = [] } = useSWR('missions', listMissions, {
    revalidateOnFocus: false,
  });

  const assistantMissions = missions.filter(
    (m: Mission) => m.mission_mode === 'assistant'
  );
  const allMissions = missions;

  // Channel data keyed by mission ID
  const [channelsByMission, setChannelsByMission] = useState<Record<string, TelegramChannel[]>>({});
  const [loadingChannels, setLoadingChannels] = useState(true);
  const [expandedMissions, setExpandedMissions] = useState<Set<string>>(new Set());

  // Create dialog
  const [showCreateDialog, setShowCreateDialog] = useState(false);
  const [createMissionId, setCreateMissionId] = useState('');
  const [createBotToken, setCreateBotToken] = useState('');
  const [createBotUsername, setCreateBotUsername] = useState('');
  const [createTriggerMode, setCreateTriggerMode] = useState<TelegramTriggerMode>('all');
  const [createInstructions, setCreateInstructions] = useState('');
  const [createAllowedChatIds, setCreateAllowedChatIds] = useState('');
  const [creating, setCreating] = useState(false);

  // Edit instructions dialog
  const [editingChannel, setEditingChannel] = useState<TelegramChannel | null>(null);
  const [editInstructions, setEditInstructions] = useState('');
  const [editTriggerMode, setEditTriggerMode] = useState<TelegramTriggerMode>('all');
  const [saving, setSaving] = useState(false);

  // Load channels for all missions that have them
  useEffect(() => {
    if (missions.length === 0) return;

    const loadAll = async () => {
      setLoadingChannels(true);
      const results: Record<string, TelegramChannel[]> = {};
      // Load channels for assistant missions
      const targets = assistantMissions.length > 0 ? assistantMissions : [];
      await Promise.all(
        targets.map(async (m: Mission) => {
          try {
            const channels = await listTelegramChannels(m.id);
            if (channels.length > 0) {
              results[m.id] = channels;
            }
          } catch {
            // Mission may not have channels endpoint yet
          }
        })
      );
      setChannelsByMission(results);
      setLoadingChannels(false);
    };

    loadAll();
  }, [missions.length, assistantMissions.length]);

  const allChannels = Object.entries(channelsByMission).flatMap(([missionId, channels]) =>
    channels.map((ch) => ({ ...ch, missionId }))
  );

  const handleCreate = async () => {
    if (!createMissionId || !createBotToken.trim()) return;
    setCreating(true);
    try {
      const input: {
        bot_token: string;
        bot_username?: string;
        trigger_mode?: TelegramTriggerMode;
        instructions?: string;
        allowed_chat_ids?: number[];
      } = {
        bot_token: createBotToken.trim(),
      };
      if (createBotUsername.trim()) input.bot_username = createBotUsername.trim();
      if (createTriggerMode !== 'all') input.trigger_mode = createTriggerMode;
      if (createInstructions.trim()) input.instructions = createInstructions.trim();
      if (createAllowedChatIds.trim()) {
        input.allowed_chat_ids = createAllowedChatIds
          .split(',')
          .map((s) => parseInt(s.trim(), 10))
          .filter((n) => !isNaN(n));
      }

      const channel = await createTelegramChannel(createMissionId, input);
      setChannelsByMission((prev) => ({
        ...prev,
        [createMissionId]: [...(prev[createMissionId] || []), channel],
      }));
      setShowCreateDialog(false);
      resetCreateForm();
      toast.success(`Telegram channel created for @${channel.bot_username || 'bot'}`);
    } catch (err) {
      toast.error(err instanceof Error ? err.message : 'Failed to create channel');
    } finally {
      setCreating(false);
    }
  };

  const handleToggleActive = async (channel: TelegramChannel) => {
    try {
      const updated = await updateTelegramChannel(channel.id, {
        active: !channel.active,
      });
      setChannelsByMission((prev) => ({
        ...prev,
        [channel.mission_id]: (prev[channel.mission_id] || []).map((ch) =>
          ch.id === updated.id ? updated : ch
        ),
      }));
      toast.success(updated.active ? 'Channel activated' : 'Channel deactivated');
    } catch (err) {
      toast.error(err instanceof Error ? err.message : 'Failed to toggle channel');
    }
  };

  const handleDelete = async (channel: TelegramChannel) => {
    if (!confirm(`Delete Telegram channel @${channel.bot_username || channel.id.slice(0, 8)}?`)) return;
    try {
      await deleteTelegramChannel(channel.id);
      setChannelsByMission((prev) => ({
        ...prev,
        [channel.mission_id]: (prev[channel.mission_id] || []).filter((ch) => ch.id !== channel.id),
      }));
      toast.success('Telegram channel deleted');
    } catch (err) {
      toast.error(err instanceof Error ? err.message : 'Failed to delete channel');
    }
  };

  const handleSaveEdit = async () => {
    if (!editingChannel) return;
    setSaving(true);
    try {
      const updated = await updateTelegramChannel(editingChannel.id, {
        instructions: editInstructions.trim() || undefined,
        trigger_mode: editTriggerMode,
      });
      setChannelsByMission((prev) => ({
        ...prev,
        [editingChannel.mission_id]: (prev[editingChannel.mission_id] || []).map((ch) =>
          ch.id === updated.id ? updated : ch
        ),
      }));
      setEditingChannel(null);
      toast.success('Channel updated');
    } catch (err) {
      toast.error(err instanceof Error ? err.message : 'Failed to update channel');
    } finally {
      setSaving(false);
    }
  };

  const resetCreateForm = () => {
    setCreateMissionId('');
    setCreateBotToken('');
    setCreateBotUsername('');
    setCreateTriggerMode('all');
    setCreateInstructions('');
    setCreateAllowedChatIds('');
  };

  const getMissionTitle = (missionId: string) => {
    const m = allMissions.find((m: Mission) => m.id === missionId);
    return m?.title || missionId.slice(0, 8) + '...';
  };

  // ESC to close dialogs
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        if (showCreateDialog) setShowCreateDialog(false);
        if (editingChannel) setEditingChannel(null);
      }
    };
    document.addEventListener('keydown', handleKeyDown);
    return () => document.removeEventListener('keydown', handleKeyDown);
  }, [showCreateDialog, editingChannel]);

  if (loadingChannels && missions.length === 0) {
    return (
      <div className="flex items-center justify-center min-h-[calc(100vh-4rem)]">
        <Loader className="h-8 w-8 animate-spin text-white/40" />
      </div>
    );
  }

  return (
    <div className="flex-1 p-6 overflow-auto">
      <div className="max-w-4xl mx-auto space-y-8">
        {/* Header */}
        <div className="flex items-center justify-between">
          <div>
            <h1 className="text-2xl font-semibold text-white mb-2">Telegram</h1>
            <p className="text-white/50">
              Connect Telegram bots to assistant missions for conversational AI.
            </p>
          </div>
          <button
            onClick={() => setShowCreateDialog(true)}
            className="flex items-center gap-2 px-4 py-2 text-sm font-medium text-white bg-indigo-500 hover:bg-indigo-600 rounded-lg transition-colors"
          >
            <Plus className="h-4 w-4" />
            Add Channel
          </button>
        </div>

        {/* Channels list */}
        {allChannels.length === 0 ? (
          <div className="rounded-xl border border-white/[0.06] bg-white/[0.02] p-12 text-center">
            <MessageCircle className="h-12 w-12 text-white/20 mx-auto mb-4" />
            <h3 className="text-lg font-medium text-white mb-2">No Telegram channels</h3>
            <p className="text-sm text-white/50 mb-6 max-w-md mx-auto">
              Create a Telegram bot via @BotFather, then add a channel here to connect it
              to an assistant mission. The bot will receive and respond to messages.
            </p>
            <button
              onClick={() => setShowCreateDialog(true)}
              className="inline-flex items-center gap-2 px-4 py-2 text-sm font-medium text-white bg-indigo-500 hover:bg-indigo-600 rounded-lg transition-colors"
            >
              <Plus className="h-4 w-4" />
              Add Channel
            </button>
          </div>
        ) : (
          <div className="space-y-4">
            {allChannels.map((channel) => (
              <div
                key={channel.id}
                className="rounded-xl border border-white/[0.06] bg-white/[0.02] overflow-hidden"
              >
                <div className="p-4 flex items-center gap-4">
                  {/* Bot icon */}
                  <div
                    className={cn(
                      'flex h-10 w-10 items-center justify-center rounded-lg',
                      channel.active ? 'bg-emerald-500/10' : 'bg-white/[0.04]'
                    )}
                  >
                    <Bot
                      className={cn(
                        'h-5 w-5',
                        channel.active ? 'text-emerald-400' : 'text-white/40'
                      )}
                    />
                  </div>

                  {/* Info */}
                  <div className="flex-1 min-w-0">
                    <div className="flex items-center gap-2">
                      <span className="text-sm font-medium text-white">
                        @{channel.bot_username || 'unknown'}
                      </span>
                      <span
                        className={cn(
                          'inline-flex items-center rounded-full px-2 py-0.5 text-[10px] font-medium',
                          channel.active
                            ? 'bg-emerald-500/10 text-emerald-400'
                            : 'bg-white/[0.06] text-white/40'
                        )}
                      >
                        {channel.active ? 'Active' : 'Inactive'}
                      </span>
                      <span className="inline-flex items-center rounded bg-white/[0.06] px-1.5 py-0.5 text-[10px] text-white/40">
                        {TRIGGER_MODE_LABELS[channel.trigger_mode]}
                      </span>
                    </div>
                    <p className="text-xs text-white/40 mt-0.5">
                      Mission: {getMissionTitle(channel.mission_id)}
                    </p>
                  </div>

                  {/* Actions */}
                  <div className="flex items-center gap-1">
                    <button
                      onClick={() => {
                        setEditingChannel(channel);
                        setEditInstructions(channel.instructions || '');
                        setEditTriggerMode(channel.trigger_mode);
                      }}
                      className="p-2 rounded-lg text-white/40 hover:text-white hover:bg-white/[0.06] transition-colors"
                      title="Edit"
                    >
                      <Settings className="h-4 w-4" />
                    </button>
                    <button
                      onClick={() => handleToggleActive(channel)}
                      className={cn(
                        'p-2 rounded-lg transition-colors',
                        channel.active
                          ? 'text-emerald-400/60 hover:text-emerald-400 hover:bg-emerald-500/10'
                          : 'text-white/40 hover:text-white hover:bg-white/[0.06]'
                      )}
                      title={channel.active ? 'Deactivate' : 'Activate'}
                    >
                      {channel.active ? (
                        <Power className="h-4 w-4" />
                      ) : (
                        <PowerOff className="h-4 w-4" />
                      )}
                    </button>
                    <button
                      onClick={() => handleDelete(channel)}
                      className="p-2 rounded-lg text-red-400/60 hover:text-red-400 hover:bg-red-500/10 transition-colors"
                      title="Delete"
                    >
                      <Trash2 className="h-4 w-4" />
                    </button>
                  </div>
                </div>

                {/* Expandable details */}
                <button
                  onClick={() => {
                    setExpandedMissions((prev) => {
                      const next = new Set(prev);
                      if (next.has(channel.id)) next.delete(channel.id);
                      else next.add(channel.id);
                      return next;
                    });
                  }}
                  className="w-full flex items-center justify-center gap-1 py-1.5 border-t border-white/[0.04] text-[10px] text-white/30 hover:text-white/50 hover:bg-white/[0.02] transition-colors"
                >
                  {expandedMissions.has(channel.id) ? (
                    <>
                      <ChevronUp className="h-3 w-3" /> Less
                    </>
                  ) : (
                    <>
                      <ChevronDown className="h-3 w-3" /> Details
                    </>
                  )}
                </button>
                {expandedMissions.has(channel.id) && (
                  <div className="px-4 pb-4 space-y-2 border-t border-white/[0.04]">
                    <div className="grid grid-cols-2 gap-4 pt-3">
                      <div>
                        <p className="text-[10px] text-white/30 mb-1">Channel ID</p>
                        <p className="text-xs text-white/60 font-mono">{channel.id}</p>
                      </div>
                      <div>
                        <p className="text-[10px] text-white/30 mb-1">Mission ID</p>
                        <p className="text-xs text-white/60 font-mono">{channel.mission_id}</p>
                      </div>
                      <div>
                        <p className="text-[10px] text-white/30 mb-1">Allowed Chat IDs</p>
                        <p className="text-xs text-white/60">
                          {channel.allowed_chat_ids?.length
                            ? channel.allowed_chat_ids.join(', ')
                            : 'All chats'}
                        </p>
                      </div>
                      <div>
                        <p className="text-[10px] text-white/30 mb-1">Created</p>
                        <p className="text-xs text-white/60">
                          {new Date(channel.created_at).toLocaleString()}
                        </p>
                      </div>
                    </div>
                    {channel.instructions && (
                      <div className="pt-2">
                        <p className="text-[10px] text-white/30 mb-1">Instructions</p>
                        <p className="text-xs text-white/60 whitespace-pre-wrap bg-white/[0.02] rounded-lg p-2 border border-white/[0.04]">
                          {channel.instructions}
                        </p>
                      </div>
                    )}
                  </div>
                )}
              </div>
            ))}
          </div>
        )}

        {/* Info card */}
        <div className="rounded-xl border border-white/[0.06] bg-white/[0.02] p-6">
          <h3 className="text-base font-medium text-white mb-3">How it works</h3>
          <ol className="space-y-2 text-sm text-white/60 list-decimal list-inside">
            <li>Create a bot via <span className="text-white/80">@BotFather</span> on Telegram</li>
            <li>Create a mission (any backend: Claude Code, OpenCode, etc.)</li>
            <li>Add a Telegram channel here with the bot token</li>
            <li>The mission automatically becomes an <span className="text-indigo-400">Assistant</span> mission</li>
            <li>Messages to the bot are routed to the mission, responses streamed back</li>
          </ol>
          <p className="text-xs text-white/40 mt-4">
            For group chats, disable bot privacy mode via @BotFather (<code className="bg-white/[0.06] px-1 py-0.5 rounded">/setprivacy</code>) to let the bot see all messages.
          </p>
        </div>
      </div>

      {/* Create Dialog */}
      {showCreateDialog && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50">
          <div className="w-full max-w-lg p-6 rounded-xl bg-[#1a1a1c] border border-white/[0.06]">
            <h3 className="text-lg font-medium text-white mb-4">Add Telegram Channel</h3>
            <div className="space-y-4">
              <div>
                <label className="block text-sm text-white/60 mb-1">Mission</label>
                <select
                  value={createMissionId}
                  onChange={(e) => setCreateMissionId(e.target.value)}
                  className="w-full px-4 py-2 rounded-lg bg-white/[0.04] border border-white/[0.08] text-white focus:outline-none focus:border-indigo-500/50"
                >
                  <option value="">Select a mission...</option>
                  {allMissions.map((m: Mission) => (
                    <option key={m.id} value={m.id}>
                      {m.title || m.id.slice(0, 8) + '...'} ({m.backend || 'claudecode'})
                      {m.mission_mode === 'assistant' ? ' [Assistant]' : ''}
                    </option>
                  ))}
                </select>
              </div>
              <div>
                <label className="block text-sm text-white/60 mb-1">Bot Token</label>
                <input
                  type="password"
                  placeholder="123456:ABC-DEF1234ghIkl-zyx57W2v1u123ew11"
                  value={createBotToken}
                  onChange={(e) => setCreateBotToken(e.target.value)}
                  className="w-full px-4 py-2 rounded-lg bg-white/[0.04] border border-white/[0.08] text-white placeholder:text-white/20 focus:outline-none focus:border-indigo-500/50 font-mono text-sm"
                />
                <p className="text-[10px] text-white/30 mt-1">
                  Get this from @BotFather on Telegram
                </p>
              </div>
              <div>
                <label className="block text-sm text-white/60 mb-1">Bot Username (optional)</label>
                <input
                  type="text"
                  placeholder="my_bot"
                  value={createBotUsername}
                  onChange={(e) => setCreateBotUsername(e.target.value)}
                  className="w-full px-4 py-2 rounded-lg bg-white/[0.04] border border-white/[0.08] text-white placeholder:text-white/20 focus:outline-none focus:border-indigo-500/50"
                />
                <p className="text-[10px] text-white/30 mt-1">
                  Auto-detected from token if omitted
                </p>
              </div>
              <div>
                <label className="block text-sm text-white/60 mb-1">Trigger Mode</label>
                <select
                  value={createTriggerMode}
                  onChange={(e) => setCreateTriggerMode(e.target.value as TelegramTriggerMode)}
                  className="w-full px-4 py-2 rounded-lg bg-white/[0.04] border border-white/[0.08] text-white focus:outline-none focus:border-indigo-500/50"
                >
                  {Object.entries(TRIGGER_MODE_LABELS).map(([mode, label]) => (
                    <option key={mode} value={mode}>
                      {label}
                    </option>
                  ))}
                </select>
              </div>
              <div>
                <label className="block text-sm text-white/60 mb-1">Allowed Chat IDs (optional)</label>
                <input
                  type="text"
                  placeholder="-1001234567890, 987654321"
                  value={createAllowedChatIds}
                  onChange={(e) => setCreateAllowedChatIds(e.target.value)}
                  className="w-full px-4 py-2 rounded-lg bg-white/[0.04] border border-white/[0.08] text-white placeholder:text-white/20 focus:outline-none focus:border-indigo-500/50 font-mono text-sm"
                />
                <p className="text-[10px] text-white/30 mt-1">
                  Leave empty to allow all chats. Comma-separated Telegram chat IDs.
                </p>
              </div>
              <div>
                <label className="block text-sm text-white/60 mb-1">Instructions (optional)</label>
                <textarea
                  placeholder="You are Ana, a helpful assistant. Respond in plain text without markdown. Keep responses concise."
                  value={createInstructions}
                  onChange={(e) => setCreateInstructions(e.target.value)}
                  rows={3}
                  className="w-full px-4 py-2 rounded-lg bg-white/[0.04] border border-white/[0.08] text-white placeholder:text-white/20 focus:outline-none focus:border-indigo-500/50 resize-none text-sm"
                />
                <p className="text-[10px] text-white/30 mt-1">
                  Prepended to every message. Use this to set personality and formatting rules.
                </p>
              </div>
            </div>
            <div className="flex justify-end gap-2 mt-6">
              <button
                onClick={() => {
                  setShowCreateDialog(false);
                  resetCreateForm();
                }}
                className="px-4 py-2 text-sm text-white/60 hover:text-white"
              >
                Cancel
              </button>
              <button
                onClick={handleCreate}
                disabled={!createMissionId || !createBotToken.trim() || creating}
                className="px-4 py-2 text-sm font-medium text-white bg-indigo-500 hover:bg-indigo-600 rounded-lg disabled:opacity-50 disabled:cursor-not-allowed"
              >
                {creating ? 'Creating...' : 'Create Channel'}
              </button>
            </div>
          </div>
        </div>
      )}

      {/* Edit Dialog */}
      {editingChannel && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50">
          <div className="w-full max-w-lg p-6 rounded-xl bg-[#1a1a1c] border border-white/[0.06]">
            <h3 className="text-lg font-medium text-white mb-4">
              Edit @{editingChannel.bot_username || 'channel'}
            </h3>
            <div className="space-y-4">
              <div>
                <label className="block text-sm text-white/60 mb-1">Trigger Mode</label>
                <select
                  value={editTriggerMode}
                  onChange={(e) => setEditTriggerMode(e.target.value as TelegramTriggerMode)}
                  className="w-full px-4 py-2 rounded-lg bg-white/[0.04] border border-white/[0.08] text-white focus:outline-none focus:border-indigo-500/50"
                >
                  {Object.entries(TRIGGER_MODE_LABELS).map(([mode, label]) => (
                    <option key={mode} value={mode}>
                      {label}
                    </option>
                  ))}
                </select>
              </div>
              <div>
                <label className="block text-sm text-white/60 mb-1">Instructions</label>
                <textarea
                  placeholder="System instructions for this assistant..."
                  value={editInstructions}
                  onChange={(e) => setEditInstructions(e.target.value)}
                  rows={4}
                  className="w-full px-4 py-2 rounded-lg bg-white/[0.04] border border-white/[0.08] text-white placeholder:text-white/20 focus:outline-none focus:border-indigo-500/50 resize-none text-sm"
                />
              </div>
            </div>
            <div className="flex justify-end gap-2 mt-6">
              <button
                onClick={() => setEditingChannel(null)}
                className="px-4 py-2 text-sm text-white/60 hover:text-white"
              >
                Cancel
              </button>
              <button
                onClick={handleSaveEdit}
                disabled={saving}
                className="px-4 py-2 text-sm font-medium text-white bg-indigo-500 hover:bg-indigo-600 rounded-lg disabled:opacity-50"
              >
                {saving ? 'Saving...' : 'Save'}
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
