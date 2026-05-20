//
//  ModelTests.swift
//  SandboxedDashboardTests
//
//  Unit tests for data models
//

import XCTest
@testable import sandboxed_sh

final class ModelTests: XCTestCase {

    // MARK: - Workspace Tests

    func testWorkspaceDecoding() throws {
        let json = """
        {
            "id": "workspace-id",
            "name": "test-workspace",
            "workspace_type": "container",
            "path": "/var/lib/workspace",
            "status": "ready",
            "error_message": null,
            "created_at": "2024-01-01T00:00:00Z"
        }
        """.data(using: .utf8)!

        let decoder = JSONDecoder()
        let workspace = try decoder.decode(Workspace.self, from: json)

        XCTAssertEqual(workspace.id, "workspace-id")
        XCTAssertEqual(workspace.name, "test-workspace")
        XCTAssertEqual(workspace.workspaceType, .container)
        XCTAssertEqual(workspace.status, .ready)
        XCTAssertNil(workspace.errorMessage)
    }

    func testWorkspaceTypeDisplayName() {
        XCTAssertEqual(WorkspaceType.host.displayName, "Host")
        XCTAssertEqual(WorkspaceType.container.displayName, "Container")
    }

    func testWorkspaceStatusProperties() {
        XCTAssertTrue(WorkspaceStatus.ready.isReady)
        XCTAssertFalse(WorkspaceStatus.pending.isReady)
        XCTAssertFalse(WorkspaceStatus.building.isReady)
        XCTAssertFalse(WorkspaceStatus.error.isReady)
    }

    func testWorkspaceIsDefault() {
        let defaultWorkspace = Workspace.defaultHost
        XCTAssertTrue(defaultWorkspace.isDefault)

        let customWorkspace = Workspace.previewContainer
        XCTAssertFalse(customWorkspace.isDefault)
    }

    // MARK: - Mission Tests

    func testMissionStatusDecoding() throws {
        let statuses = ["active", "completed", "failed", "interrupted", "blocked", "not_feasible"]
        let expectedStatuses: [MissionStatus] = [.active, .completed, .failed, .interrupted, .blocked, .notFeasible]

        for (json, expected) in zip(statuses, expectedStatuses) {
            let data = "\"\(json)\"".data(using: .utf8)!
            let status = try JSONDecoder().decode(MissionStatus.self, from: data)
            XCTAssertEqual(status, expected)
        }
    }

    func testMissionStatusDisplayLabel() {
        XCTAssertEqual(MissionStatus.active.displayLabel, "Active")
        XCTAssertEqual(MissionStatus.completed.displayLabel, "Completed")
        XCTAssertEqual(MissionStatus.failed.displayLabel, "Failed")
        XCTAssertEqual(MissionStatus.interrupted.displayLabel, "Interrupted")
        XCTAssertEqual(MissionStatus.blocked.displayLabel, "Blocked")
        XCTAssertEqual(MissionStatus.notFeasible.displayLabel, "Not Feasible")
    }

    func testMissionStatusCanResume() {
        // Active missions cannot be resumed (already active)
        XCTAssertFalse(MissionStatus.active.canResume)
        // Completed missions cannot be resumed
        XCTAssertFalse(MissionStatus.completed.canResume)
    }

    func testMissionDecodesGoalModeFields() throws {
        let json = """
        {
            "id": "mission-id",
            "status": "active",
            "title": "Goal mission",
            "history": [],
            "resumable": false,
            "agent": "codex",
            "backend": "codex",
            "goal_mode": true,
            "goal_objective": "Ship the feature",
            "created_at": "2024-01-01T00:00:00Z",
            "updated_at": "2024-01-01T00:00:00Z"
        }
        """.data(using: .utf8)!

        let mission = try JSONDecoder().decode(Mission.self, from: json)

        XCTAssertTrue(mission.goalMode)
        XCTAssertEqual(mission.goalObjective, "Ship the feature")
    }

    func testMissionDefaultsGoalModeFields() throws {
        let json = """
        {
            "id": "mission-id",
            "status": "completed",
            "title": "Regular mission",
            "history": [],
            "resumable": false,
            "created_at": "2024-01-01T00:00:00Z",
            "updated_at": "2024-01-01T00:00:00Z"
        }
        """.data(using: .utf8)!

        let mission = try JSONDecoder().decode(Mission.self, from: json)

        XCTAssertFalse(mission.goalMode)
        XCTAssertNil(mission.goalObjective)
    }

    func testControlViewKeepsIOSValidationAnchors() throws {
        let source = try controlViewSource()

        XCTAssertTrue(source.contains("control-inline-thinking"))
        XCTAssertTrue(source.contains("thoughts-timeline"))
        XCTAssertTrue(source.contains("thought-latest"))
        XCTAssertTrue(source.contains("thoughts-bottom"))
        XCTAssertTrue(source.contains(".defaultScrollAnchor(.bottom)"))
        XCTAssertTrue(source.contains("ScrollAnchorState"))
        XCTAssertTrue(source.contains("case pinned"))
        XCTAssertTrue(source.contains("case detached"))
        XCTAssertTrue(source.contains("case programmatic"))
        XCTAssertTrue(source.contains("scrollAnchorState == .pinned"))
    }

    func testControlViewKeepsReconnectAndStreamingGates() throws {
        let source = try controlViewSource()

        XCTAssertTrue(source.contains("stream_lagged"))
        XCTAssertTrue(source.contains("resumeMissionAfterReconnect"))
        XCTAssertTrue(source.contains("sinceSeq"))
        XCTAssertTrue(source.contains("beforeSeq"))
        XCTAssertTrue(source.contains("missionMinSeq"))
        XCTAssertTrue(source.contains("mergeMissionEvents"))
        XCTAssertTrue(source.contains("Task.sleep(for: .milliseconds(16))"))
        XCTAssertTrue(source.contains("controlDroppedEvents"))
        XCTAssertTrue(source.contains("goal_role"))
        XCTAssertTrue(source.contains("goal-deliverable-"))
    }

    func testSharedControlReducerFixturesReplayOnIOS() throws {
        let fixtures = try sharedControlReducerFixtures()

        for fixtureCase in fixtures.cases {
            let messages = replayFixtureEvents(fixtureCase.events, mission: fixtures.mission)
            let normalized = messages.map(NormalizedFixtureMessage.init(message:))
            XCTAssertEqual(
                normalized,
                fixtureCase.expected,
                "Fixture failed: \(fixtureCase.name)"
            )
        }
    }

    private func controlViewSource() throws -> String {
        let testFile = URL(fileURLWithPath: #filePath)
        let controlView = testFile
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .appendingPathComponent("SandboxedDashboard/Views/Control/ControlView.swift")
        return try String(contentsOf: controlView, encoding: .utf8)
    }

    private func sharedControlReducerFixtures() throws -> SharedControlReducerFixtures {
        let testFile = URL(fileURLWithPath: #filePath)
        let fixturesURL = testFile
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .appendingPathComponent("shared/control-reducer-fixtures.json")
        let data = try Data(contentsOf: fixturesURL)
        return try JSONDecoder().decode(SharedControlReducerFixtures.self, from: data)
    }

    private func replayFixtureEvents(_ events: [StoredEvent], mission: Mission) -> [ChatMessage] {
        var messages: [ChatMessage] = []
        var textOpBuffers: [String: String] = [:]
        let orderedEvents = events.sorted { lhs, rhs in
            if lhs.sequence != rhs.sequence { return lhs.sequence < rhs.sequence }
            return lhs.id < rhs.id
        }

        for event in orderedEvents {
            var data = event.metadata.mapValues(\.value)
            data["mission_id"] = event.missionId
            data["content"] = event.content
            if let eventId = event.eventId { data["id"] = eventId }
            if event.eventType == "text_op",
               let jsonData = event.content.data(using: .utf8),
               let ops = try? JSONSerialization.jsonObject(with: jsonData) {
                data["ops"] = ops
            }

            switch event.eventType {
            case "assistant_message", "assistant_message_canonical":
                guard let id = data["id"] as? String,
                      !messages.contains(where: { $0.id == id }) else { continue }
                messages.append(
                    ChatMessage(
                        id: id,
                        type: .assistant(success: data["success"] as? Bool ?? true, costCents: 0, costSource: .unknown, model: nil, sharedFiles: nil),
                        content: data["content"] as? String ?? ""
                    )
                )
            case "thinking":
                let content = data["content"] as? String ?? ""
                let done = data["done"] as? Bool ?? false
                if done, data["goal_role"] as? String == "deliverable", mission.goalMode {
                    let baseId = data["id"] as? String ?? String(event.id)
                    let id = "goal-deliverable-\(baseId)"
                    guard !messages.contains(where: { $0.id == id }) else { continue }
                    messages.append(
                        ChatMessage(
                            id: id,
                            type: .assistant(success: true, costCents: 0, costSource: .unknown, model: nil, sharedFiles: nil),
                            content: content
                        )
                    )
                    continue
                }
                if let id = data["id"] as? String,
                   messages.contains(where: { $0.id == id }) {
                    continue
                }
                if let index = messages.lastIndex(where: { $0.isThinking && !$0.thinkingDone }) {
                    let existing = messages[index]
                    messages[index] = ChatMessage(
                        id: existing.id,
                        type: .thinking(done: done, startTime: existing.thinkingStartTime ?? existing.timestamp),
                        content: content,
                        timestamp: existing.timestamp
                    )
                } else {
                    messages.append(
                        ChatMessage(
                            id: data["id"] as? String ?? "thinking-\(event.id)",
                            type: .thinking(done: done, startTime: Date()),
                            content: content
                        )
                    )
                }
            case "text_op":
                let bubbleId = data["bubble_id"] as? String ?? "text-op-latest"
                let ops = data["ops"] as? [[String: Any]] ?? []
                var content = textOpBuffers[bubbleId] ?? ""
                var finalized = false
                for op in ops {
                    switch op["type"] as? String {
                    case "insert":
                        let pos = min(max(op["pos"] as? Int ?? content.count, 0), content.count)
                        let index = content.index(content.startIndex, offsetBy: pos)
                        content.insert(contentsOf: op["text"] as? String ?? "", at: index)
                    case "replace":
                        let range = op["range"] as? [Int] ?? []
                        let start = min(max(range.first ?? 0, 0), content.count)
                        let end = min(max(range.dropFirst().first ?? content.count, start), content.count)
                        let startIndex = content.index(content.startIndex, offsetBy: start)
                        let endIndex = content.index(content.startIndex, offsetBy: end)
                        content.replaceSubrange(startIndex..<endIndex, with: op["text"] as? String ?? "")
                    case "finalize":
                        finalized = true
                    default:
                        continue
                    }
                }
                textOpBuffers[bubbleId] = finalized ? nil : content
                if let index = messages.lastIndex(where: { $0.isThinking && !$0.thinkingDone && $0.id.hasPrefix("stream-thinking-") }) {
                    messages[index] = ChatMessage(
                        id: messages[index].id,
                        type: .thinking(done: finalized, startTime: messages[index].thinkingStartTime ?? messages[index].timestamp),
                        content: content,
                        timestamp: messages[index].timestamp
                    )
                } else {
                    messages.append(
                        ChatMessage(
                            id: "stream-thinking-\(bubbleId)",
                            type: .thinking(done: finalized, startTime: Date()),
                            content: content
                        )
                    )
                }
            default:
                continue
            }
        }

        return messages
    }

    // MARK: - FileEntry Tests

    func testFileEntryDecoding() throws {
        let json = """
        {
            "name": "test.txt",
            "path": "/home/user/test.txt",
            "kind": "file",
            "size": 1024,
            "mtime": 1704067200
        }
        """.data(using: .utf8)!

        let decoder = JSONDecoder()
        let entry = try decoder.decode(FileEntry.self, from: json)

        XCTAssertEqual(entry.name, "test.txt")
        XCTAssertEqual(entry.path, "/home/user/test.txt")
        XCTAssertTrue(entry.isFile)
        XCTAssertFalse(entry.isDirectory)
        XCTAssertEqual(entry.size, 1024)
    }

    func testFileEntryDirectoryDecoding() throws {
        let json = """
        {
            "name": "docs",
            "path": "/home/user/docs",
            "kind": "dir",
            "size": 0,
            "mtime": 1704067200
        }
        """.data(using: .utf8)!

        let decoder = JSONDecoder()
        let entry = try decoder.decode(FileEntry.self, from: json)

        XCTAssertEqual(entry.name, "docs")
        XCTAssertTrue(entry.isDirectory)
        XCTAssertFalse(entry.isFile)
    }

    func testFileEntryFormattedSize() throws {
        let json = """
        {
            "name": "large.bin",
            "path": "/tmp/large.bin",
            "kind": "file",
            "size": 1048576,
            "mtime": 1704067200
        }
        """.data(using: .utf8)!

        let entry = try JSONDecoder().decode(FileEntry.self, from: json)
        // 1MB = 1024 KB = 1 MB
        XCTAssertTrue(entry.formattedSize.contains("MB") || entry.formattedSize.contains("KB"))
    }

    func testFileEntryIcon() throws {
        // Test Swift file icon
        let swiftJson = """
        {"name": "test.swift", "path": "/tmp/test.swift", "kind": "file", "size": 100, "mtime": 0}
        """.data(using: .utf8)!
        let swiftEntry = try JSONDecoder().decode(FileEntry.self, from: swiftJson)
        XCTAssertEqual(swiftEntry.icon, "doc.text.fill")

        // Test directory icon
        let dirJson = """
        {"name": "folder", "path": "/tmp/folder", "kind": "dir", "size": 0, "mtime": 0}
        """.data(using: .utf8)!
        let dirEntry = try JSONDecoder().decode(FileEntry.self, from: dirJson)
        XCTAssertEqual(dirEntry.icon, "folder.fill")
    }
}

private struct SharedControlReducerFixtures: Decodable {
    let mission: Mission
    let cases: [SharedControlReducerFixtureCase]
}

private struct SharedControlReducerFixtureCase: Decodable {
    let name: String
    let events: [StoredEvent]
    let expected: [NormalizedFixtureMessage]
}

private struct NormalizedFixtureMessage: Decodable, Equatable {
    let kind: String
    let id: String?
    let content: String
    let done: Bool?
    let success: Bool?

    init(kind: String, id: String? = nil, content: String, done: Bool? = nil, success: Bool? = nil) {
        self.kind = kind
        self.id = id
        self.content = content
        self.done = done
        self.success = success
    }

    init(message: ChatMessage) {
        switch message.type {
        case .user:
            self.init(kind: "user", id: message.id, content: message.content)
        case .assistant(let success, _, _, _, _):
            self.init(kind: "assistant", id: message.id, content: message.content, success: success)
        case .thinking(let done, _):
            let kind = message.id.hasPrefix("stream-thinking-") ? "stream" : "thinking"
            let id = kind == "stream"
                ? String(message.id.dropFirst("stream-thinking-".count))
                : message.id
            self.init(kind: kind, id: id, content: message.content, done: done)
        default:
            self.init(kind: "other", id: message.id, content: message.content)
        }
    }

    static func == (lhs: NormalizedFixtureMessage, rhs: NormalizedFixtureMessage) -> Bool {
        lhs.kind == rhs.kind
            && (rhs.id == nil || lhs.id == rhs.id)
            && lhs.content == rhs.content
            && (rhs.done == nil || lhs.done == rhs.done)
            && (rhs.success == nil || lhs.success == rhs.success)
    }
}
