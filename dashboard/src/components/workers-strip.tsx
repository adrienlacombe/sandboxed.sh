'use client';

import { memo, useMemo } from 'react';
import {
  Loader2,
  CheckCircle,
  XCircle,
  AlertTriangle,
  Clock,
  Ban,
  ArrowLeft,
  Crown,
} from 'lucide-react';
import { cn } from '@/lib/utils';
import { getMissionShortName } from '@/lib/mission-display';
import type { Mission, RunningMissionInfo } from '@/lib/api';

interface WorkersStripProps {
  /** Workers shown as chips. On a boss view these are children; on a
   * worker view they are siblings (so you can hop between workers). */
  childMissions: Mission[];
  runningMissions: RunningMissionInfo[];
  viewingMissionId?: string | null;
  onSelectWorker: (missionId: string) => void;
  /** When set, the strip renders a leading "Back to Boss" pill that
   * navigates to this mission. Use for worker views. */
  parentMission?: Mission | null;
  className?: string;
}

type ChipStatus = {
  icon: React.ReactNode;
  color: string;
  bg: string;
  activity: string | null;
  isActive: boolean;
};

function chipStatusFor(mission: Mission, info?: RunningMissionInfo): ChipStatus {
  if (info) {
    if (info.state === 'running') {
      return {
        icon: <Loader2 className="h-3 w-3 animate-spin" />,
        color: 'text-indigo-700 dark:text-indigo-300',
        bg: 'bg-indigo-500/10 border-indigo-500/25 hover:bg-indigo-500/15',
        activity: info.current_activity || null,
        isActive: true,
      };
    }
    if (info.state === 'waiting_for_tool') {
      return {
        icon: <Clock className="h-3 w-3" />,
        color: 'text-amber-700 dark:text-amber-300',
        bg: 'bg-amber-500/10 border-amber-500/25 hover:bg-amber-500/15',
        activity: info.current_activity || 'Waiting for tool',
        isActive: true,
      };
    }
    if (info.state === 'queued') {
      return {
        icon: <Clock className="h-3 w-3" />,
        color: 'text-foreground/65 dark:text-white/60',
        bg: 'bg-white/[0.03] border-white/[0.08] hover:bg-white/[0.06]',
        activity: 'Queued',
        isActive: false,
      };
    }
  }

  switch (mission.status) {
    case 'completed':
      return {
        icon: <CheckCircle className="h-3 w-3" />,
        color: 'text-emerald-700 dark:text-emerald-300',
        bg: 'bg-emerald-500/10 border-emerald-500/20 hover:bg-emerald-500/15',
        activity: null,
        isActive: false,
      };
    case 'failed':
      return {
        icon: <XCircle className="h-3 w-3" />,
        color: 'text-red-700 dark:text-red-300',
        bg: 'bg-red-500/10 border-red-500/20 hover:bg-red-500/15',
        activity: null,
        isActive: false,
      };
    case 'interrupted':
      return {
        icon: <AlertTriangle className="h-3 w-3" />,
        color: 'text-amber-700 dark:text-amber-300',
        bg: 'bg-amber-500/10 border-amber-500/20 hover:bg-amber-500/15',
        activity: null,
        isActive: false,
      };
    case 'not_feasible':
      return {
        icon: <Ban className="h-3 w-3" />,
        color: 'text-rose-700 dark:text-rose-300',
        bg: 'bg-rose-500/10 border-rose-500/20 hover:bg-rose-500/15',
        activity: null,
        isActive: false,
      };
    case 'active':
      return {
        icon: <Loader2 className="h-3 w-3 animate-spin" />,
        color: 'text-indigo-700 dark:text-indigo-300',
        bg: 'bg-indigo-500/10 border-indigo-500/25 hover:bg-indigo-500/15',
        activity: null,
        isActive: true,
      };
    default:
      return {
        icon: <Clock className="h-3 w-3" />,
        color: 'text-foreground/60 dark:text-white/50',
        bg: 'bg-white/[0.03] border-white/[0.08] hover:bg-white/[0.06]',
        activity: null,
        isActive: false,
      };
  }
}

/**
 * Horizontal, sticky strip of worker chips. Sits at the top of the chat
 * container so the boss can see active workers without opening the side
 * panel. Click-to-switch into a worker. Self-hides when there are no
 * children.
 *
 * Performance note: memoized; the sort + chip-info derivation is
 * recomputed only when `childMissions` or `runningMissions` change. The
 * chat scroll never re-renders this strip because it lives outside the
 * scrolling region.
 */
export const WorkersStrip = memo(function WorkersStrip({
  childMissions,
  runningMissions,
  viewingMissionId,
  onSelectWorker,
  parentMission,
  className,
}: WorkersStripProps) {
  const chips = useMemo(() => {
    if (childMissions.length === 0) return [];
    const running = new Map<string, RunningMissionInfo>();
    for (const rm of runningMissions) running.set(rm.mission_id, rm);

    return [...childMissions]
      .map((m) => ({ mission: m, info: running.get(m.id), status: chipStatusFor(m, running.get(m.id)) }))
      .sort((a, b) => {
        // Active first, then by updated_at desc.
        if (a.status.isActive !== b.status.isActive) return a.status.isActive ? -1 : 1;
        const at = a.mission.updated_at || a.mission.created_at || '';
        const bt = b.mission.updated_at || b.mission.created_at || '';
        return bt.localeCompare(at);
      });
  }, [childMissions, runningMissions]);

  // Nothing to show: no parent link AND no worker chips.
  if (!parentMission && chips.length === 0) return null;

  const onWorkerView = Boolean(parentMission);
  const parentTitle = parentMission
    ? parentMission.title?.trim() || getMissionShortName(parentMission.id)
    : null;

  return (
    <div
      className={cn(
        'flex items-center gap-2 px-4 py-2 border-b border-white/[0.06] overflow-x-auto',
        'scrollbar-thin scrollbar-thumb-white/10 scrollbar-track-transparent',
        className
      )}
      aria-label={onWorkerView ? 'Worker navigation' : 'Active workers'}
    >
      {parentMission && (
        <>
          <button
            type="button"
            onClick={() => onSelectWorker(parentMission.id)}
            className={cn(
              'shrink-0 inline-flex items-center gap-1.5 rounded-full border border-violet-500/30',
              'bg-violet-500/10 hover:bg-violet-500/20 text-violet-300',
              'px-2.5 py-1 text-xs transition-colors max-w-[300px]'
            )}
            title={`Back to boss: ${parentTitle}`}
            aria-label={`Back to boss mission ${parentTitle}`}
          >
            <ArrowLeft className="h-3 w-3 shrink-0" />
            <Crown className="h-3 w-3 shrink-0" />
            <span className="hidden sm:inline text-violet-400/80">Boss:</span>
            <span className="truncate">{parentTitle}</span>
          </button>
          {chips.length > 0 && (
            <span
              aria-hidden
              className="shrink-0 h-4 w-px bg-white/10 mx-1"
            />
          )}
        </>
      )}
      {chips.length > 0 && (
        <span className="shrink-0 text-[10px] uppercase tracking-wider text-white/40 mr-1">
          {onWorkerView ? 'Siblings' : 'Workers'}
        </span>
      )}
      {chips.map(({ mission, status }) => {
        const isViewing = mission.id === viewingMissionId;
        const title = mission.title?.trim() || getMissionShortName(mission.id);
        return (
          <button
            key={mission.id}
            onClick={() => onSelectWorker(mission.id)}
            className={cn(
              'shrink-0 inline-flex items-center gap-1.5 rounded-full border px-2.5 py-1 text-xs transition-colors max-w-[280px]',
              status.bg,
              isViewing && 'ring-1 ring-indigo-400/60'
            )}
            title={status.activity ? `${title}: ${status.activity}` : title}
          >
            <span className={cn('shrink-0', status.color)}>{status.icon}</span>
            <span className="truncate text-foreground/85">{title}</span>
            {status.activity && (
              <span className="hidden md:inline truncate text-white/40 max-w-[120px]">
                · {status.activity}
              </span>
            )}
          </button>
        );
      })}
    </div>
  );
});
