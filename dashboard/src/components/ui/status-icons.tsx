import {
  Clock,
  Loader,
  Bell,
  CheckCircle,
  XCircle,
  Ban,
  AlertTriangle,
  HelpCircle,
  type LucideIcon,
} from 'lucide-react';
import type { MissionStatus } from '@/lib/api';

/**
 * Unified icon mapping for mission statuses. Single source of truth — the
 * string-typed twin in `@/lib/mission-status` has been removed.
 */
export const STATUS_ICONS: Record<string, LucideIcon> = {
  pending: Clock,
  active: Loader,
  running: Loader,
  awaiting_user: Bell,
  acknowledged: CheckCircle,
  completed: CheckCircle,
  failed: XCircle,
  cancelled: Ban,
  interrupted: Ban,
  blocked: AlertTriangle,
  not_feasible: XCircle,
  unknown: HelpCircle,
};

/**
 * Get the icon component for a mission status.
 * @param status - The mission status
 * @param fallback - Fallback icon (default: Clock)
 */
export function getStatusIcon(status: MissionStatus | string, fallback: LucideIcon = Clock): LucideIcon {
  return STATUS_ICONS[status] || fallback;
}
