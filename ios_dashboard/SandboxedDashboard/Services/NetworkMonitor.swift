//
//  NetworkMonitor.swift
//  SandboxedDashboard
//
//  Decouples the "is the network reachable" signal from the SSE stream.
//  The previous design only flipped the connection banner from SSE state,
//  which silently lied about reachability whenever the SSE socket happened
//  to be held open by an upstream proxy / NAT while every HTTP call hung.
//
//  This monitor combines two signals:
//
//  1. `NWPathMonitor` — kernel-level "is there an IP path".
//  2. A lightweight `/api/health` probe, fired every `healthInterval` while
//     the SSE has been silent for `staleAfter` seconds. A probe failure is
//     additional evidence we're not actually online.
//
//  The aggregated `state` is published as a `ConnectionState` so callers can
//  bind their banner directly to it (or merge it with their SSE state).
//

import Foundation
import Network
import Observation

@MainActor
@Observable
final class NetworkMonitor {
    /// Time since the last byte from the SSE stream after which we consider
    /// the stream "stale" and start proactive health probes.
    static let staleAfter: TimeInterval = 12

    /// Cadence for the `/api/health` probe while the SSE is stale.
    static let healthInterval: TimeInterval = 10

    /// Number of consecutive health failures before we flip from `degraded`
    /// to `disconnected`. One bad probe could be a tiny blip.
    static let failuresBeforeOffline = 3

    /// Aggregated state — bind your UI to this rather than to the SSE state
    /// directly. This already incorporates the SSE state via
    /// `noteStreamReconnecting` / `noteStreamConnected`, so callers don't
    /// need to do their own merging in the View body (which historically
    /// broke the SwiftUI ViewBuilder type-checker on the large ControlView).
    private(set) var state: ConnectionState = .connected

    /// Last time the SSE stream delivered any byte to us. Updated by callers
    /// via `noteStreamActivity`. Initialised to `.now` so a freshly-launched
    /// app doesn't immediately think the stream is stale.
    private(set) var lastStreamActivity: Date = Date()

    /// Set to true while NWPathMonitor reports `.satisfied`.
    private(set) var pathSatisfied: Bool = true

    /// Latest SSE state reported by the caller. Merged with `pathSatisfied`
    /// and `healthFailures` to produce `state`.
    private var sseState: ConnectionState = .connected

    private let pathMonitor = NWPathMonitor()
    private let pathQueue = DispatchQueue(label: "md.thomas.openagent.netpath")
    private var healthTask: Task<Void, Never>?
    private var healthFailures = 0

    nonisolated init() {}

    func start() {
        pathMonitor.pathUpdateHandler = { [weak self] path in
            guard let self else { return }
            let satisfied = path.status == .satisfied
            Task { @MainActor in
                self.pathSatisfied = satisfied
                self.recomputeState()
            }
        }
        pathMonitor.start(queue: pathQueue)
        startHealthLoop()
    }

    func stop() {
        pathMonitor.cancel()
        healthTask?.cancel()
        healthTask = nil
    }

    /// Call this from the SSE event handler on every received byte / event.
    /// Resets the staleness timer and the health-probe failure counter.
    func noteStreamActivity() {
        lastStreamActivity = Date()
        healthFailures = 0
        recomputeState()
    }

    /// Signal that the SSE socket itself is in a transitional state. Used by
    /// the stream loop so the banner doesn't flicker between `degraded` and
    /// `reconnecting`.
    func noteStreamReconnecting(attempt: Int) {
        sseState = .reconnecting(attempt: attempt)
        recomputeState()
    }

    /// The SSE has just connected and emitted (or replayed) at least one
    /// real event. Clears any stale/offline state.
    func noteStreamConnected() {
        sseState = .connected
        healthFailures = 0
        lastStreamActivity = Date()
        recomputeState()
    }

    /// The SSE is intentionally torn down (mission switch, view disappear).
    /// Banner stays clean — disconnect is expected, not an error.
    func noteStreamIdle() {
        sseState = .connected
        recomputeState()
    }

    private func recomputeState() {
        // Path down beats everything else.
        if !pathSatisfied {
            state = .disconnected
            return
        }
        if healthFailures >= Self.failuresBeforeOffline {
            state = .disconnected
            return
        }
        // SSE explicitly reconnecting outranks degraded.
        if case .reconnecting = sseState {
            state = sseState
            return
        }
        let staleness = Date().timeIntervalSince(lastStreamActivity)
        if staleness > Self.staleAfter || healthFailures > 0 {
            state = .degraded
            return
        }
        state = .connected
    }

    private func startHealthLoop() {
        healthTask?.cancel()
        healthTask = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(Self.healthInterval))
                guard !Task.isCancelled else { return }
                await self?.runHealthProbeIfNeeded()
            }
        }
    }

    private func runHealthProbeIfNeeded() async {
        // Only probe when the SSE has been silent long enough to be worth
        // burning a request on. On a busy stream this loop is a no-op.
        let staleness = Date().timeIntervalSince(lastStreamActivity)
        guard staleness > Self.staleAfter else { return }
        guard pathSatisfied else {
            // No path → no point probing; state is already `.disconnected`.
            return
        }

        do {
            let ok = try await APIService.shared.checkHealth()
            if ok {
                healthFailures = 0
            } else {
                healthFailures += 1
            }
        } catch {
            healthFailures += 1
        }
        recomputeState()
    }
}
