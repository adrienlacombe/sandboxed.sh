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
  text: string;
  border: string;
  /** Tinted fill that makes live workers "light up" against the flat
   * neutral surface of idle/terminal ones — hierarchy via surface, not a
   * blunt opacity dim that would also crush idle text contrast. */
  surface: string;
  activity: string | null;
  isActive: boolean;
};

// Spinner that holds still under prefers-reduced-motion (the glyph alone
// still reads as "working").
const Spinner = (
  <Loader2 className="h-3 w-3 animate-spin motion-reduce:animate-none" />
);

const FLAT_SURFACE = 'bg-white/[0.02]';

function chipStatusFor(mission: Mission, info?: RunningMissionInfo): ChipStatus {
  if (info) {
    if (info.state === 'running') {
      return {
        icon: Spinner,
        text: 'text-indigo-400',
        border: 'border-indigo-500/30',
        surface: 'bg-indigo-500/[0.08]',
        activity: info.current_activity || null,
        isActive: true,
      };
    }
    if (info.state === 'waiting_for_tool') {
      return {
        icon: <Clock className="h-3 w-3" />,
        text: 'text-amber-400',
        border: 'border-amber-500/30',
        surface: 'bg-amber-500/[0.08]',
        activity: info.current_activity || 'Waiting for tool',
        isActive: true,
      };
    }
    if (info.state === 'queued') {
      return {
        icon: <Clock className="h-3 w-3" />,
        text: 'text-white/60',
        border: 'border-white/10',
        surface: FLAT_SURFACE,
        activity: 'Queued',
        isActive: false,
      };
    }
  }

  switch (mission.status) {
    case 'completed':
      return {
        icon: <CheckCircle className="h-3 w-3" />,
        text: 'text-emerald-400',
        border: 'border-emerald-500/25',
        surface: FLAT_SURFACE,
        activity: null,
        isActive: false,
      };
    case 'failed':
      return {
        icon: <XCircle className="h-3 w-3" />,
        text: 'text-red-400',
        border: 'border-red-500/25',
        surface: FLAT_SURFACE,
        activity: null,
        isActive: false,
      };
    case 'interrupted':
      return {
        icon: <AlertTriangle className="h-3 w-3" />,
        text: 'text-amber-400',
        border: 'border-amber-500/25',
        surface: FLAT_SURFACE,
        activity: null,
        isActive: false,
      };
    case 'not_feasible':
      return {
        icon: <Ban className="h-3 w-3" />,
        text: 'text-rose-400',
        border: 'border-rose-500/25',
        surface: FLAT_SURFACE,
        activity: null,
        isActive: false,
      };
    case 'active':
      return {
        icon: Spinner,
        text: 'text-indigo-400',
        border: 'border-indigo-500/30',
        surface: 'bg-indigo-500/[0.08]',
        activity: null,
        isActive: true,
      };
    default:
      return {
        icon: <Clock className="h-3 w-3" />,
        text: 'text-white/50',
        border: 'border-white/[0.08]',
        surface: FLAT_SURFACE,
        activity: null,
        isActive: false,
      };
  }
}

/**
 * Horizontal, sticky strip of worker chips. Sits at the top of the chat
 * container so the boss can see active workers without opening a side
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
  const activeCount = chips.filter((c) => c.status.isActive).length;
  // Index where the active → idle transition happens (chips are sorted active-first).
  const firstIdleIndex = chips.findIndex((c) => !c.status.isActive);

  return (
    <div
      className={cn(
        'flex items-center gap-1.5 px-3 py-1.5 border-b border-white/[0.06] overflow-x-auto',
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
              'shrink-0 inline-flex h-6 items-center gap-1 rounded-md border border-violet-500/30',
              'bg-violet-500/10 hover:bg-violet-500/20 text-violet-400',
              'px-2 text-[11px] font-medium max-w-[280px]',
              'transition-[background-color,transform] duration-150 active:translate-y-px'
            )}
            title={`Back to boss: ${parentTitle}`}
            aria-label={`Back to boss mission ${parentTitle}`}
          >
            <ArrowLeft className="h-3 w-3 shrink-0" />
            <Crown className="h-3 w-3 shrink-0" />
            <span className="truncate">{parentTitle}</span>
          </button>
          {chips.length > 0 && (
            <span aria-hidden className="shrink-0 h-3.5 w-px bg-white/10" />
          )}
        </>
      )}
      {chips.length > 0 && (
        <span
          className="shrink-0 inline-flex items-center gap-1 text-[10px] uppercase tracking-wider text-white/45 mr-0.5"
          title={`${activeCount} active of ${chips.length} ${onWorkerView ? 'siblings' : 'workers'}`}
        >
          <span>{onWorkerView ? 'Siblings' : 'Workers'}</span>
          <span className="tabular-nums">
            <span className={activeCount > 0 ? 'text-indigo-300' : 'text-white/55'}>
              {activeCount}
            </span>
            <span className="text-white/30">/{chips.length}</span>
          </span>
        </span>
      )}
      {chips.map(({ mission, status }, index) => {
        const isViewing = mission.id === viewingMissionId;
        const title = mission.title?.trim() || getMissionShortName(mission.id);
        const showDivider =
          index !== 0 && index === firstIdleIndex && activeCount > 0;
        return (
          <span key={mission.id} className="contents">
            {showDivider && (
              <span
                aria-hidden
                className="shrink-0 h-3.5 w-px bg-white/10 mx-0.5"
                title="Idle workers"
              />
            )}
            <button
              onClick={() => onSelectWorker(mission.id)}
              aria-current={isViewing ? 'true' : undefined}
              className={cn(
                'shrink-0 inline-flex h-6 items-center gap-1.5 rounded-md border px-2 text-[11px] max-w-[260px]',
                'transition-[background-color,box-shadow,transform] duration-150 active:translate-y-px',
                status.border,
                status.surface,
                'hover:bg-white/[0.06]',
                // "Viewing" is a structural cue (you-are-here): a neutral inset
                // ring, so it reads on top of the chip's semantic status hue
                // instead of replacing it.
                isViewing && 'ring-1 ring-inset ring-white/30'
              )}
              title={status.activity ? `${title}: ${status.activity}` : title}
            >
              <span className={cn('shrink-0', status.text)}>{status.icon}</span>
              <span className="truncate text-foreground/85 font-medium">
                {title}
              </span>
              {status.activity && (
                <span className="hidden lg:inline truncate text-white/45 max-w-[120px]">
                  {status.activity}
                </span>
              )}
            </button>
          </span>
        );
      })}
    </div>
  );
});
