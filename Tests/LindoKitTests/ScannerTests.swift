import Foundation
import Testing
@testable import LindoKit

private final class TrashRecorder: @unchecked Sendable {
    private let lock = NSLock()
    private var storage: [URL] = []
    func append(_ url: URL) { lock.lock(); defer { lock.unlock() }; storage.append(url) }
    var urls: [URL] { lock.lock(); defer { lock.unlock() }; return storage }
}

private struct Fixture {
    let root: URL
    init() throws {
        root = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Developer/Lindo/test-fixtures").appendingPathComponent("LindoTests-" + UUID().uuidString)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
    }
    func write(_ name: String, bytes: Int = 12) throws -> URL {
        let url = root.appendingPathComponent(name)
        try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        try Data(repeating: 65, count: bytes).write(to: url)
        return url
    }
    // Inventory only admits bundles whose identity can be read.
    func identify(_ app: String, as identifier: String) throws {
        let url = root.appendingPathComponent(app).appendingPathComponent("Contents/Info.plist")
        try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        let metadata: [String: String] = ["CFBundleIdentifier": identifier, "CFBundleShortVersionString": "1.0"]
        try PropertyListSerialization.data(fromPropertyList: metadata, format: .xml, options: 0).write(to: url)
    }
    func clean() { try? FileManager.default.removeItem(at: root) }
}

@Test func scanFiltersAndDeduplicates() async throws {
    let f = try Fixture(); defer { f.clean() }
    let wanted = try f.write("nested/large.bin", bytes: 50)
    _ = try f.write("small.bin", bytes: 2)
    _ = try f.write("secret.pages/inside.bin", bytes: 100)
    _ = try f.write("_2_Work/CLIENTS/client.bin", bytes: 100)
    _ = try f.write("_6_Archivio_Private/secret.bin", bytes: 100)
    _ = try f.write(".ssh/key", bytes: 100)
    _ = try f.write(".git/objects/pack/file.bin", bytes: 100)
    _ = try f.write("photo.icloud", bytes: 100)
    try FileManager.default.createSymbolicLink(at: f.root.appendingPathComponent("shortcut"), withDestinationURL: wanted)
    let scanner = MacScanner(trash: { _ in Issue.record("Unexpected trash") })
    let report = await scanner.scan(category: .largeFiles, roots: [f.root, f.root.appendingPathComponent("nested"), f.root], minimumBytes: 10)
    #expect(report.items.map(\.url) == [wanted])
    #expect(report.isPartial)
    // Protected paths, symlinks and non-local files all explain partial coverage.
    for excluded in ["CLIENTS", "_6_Archivio_Private", ".ssh", "shortcut", "photo.icloud"] {
        #expect(report.warnings.contains { $0.contains(excluded) })
    }
}

@Test func trashRevalidatesAndConsumesIDs() async throws {
    let f = try Fixture(); defer { f.clean() }
    let changed = try f.write("changed.bin")
    let valid = try f.write("valid.bin")
    let recorder = TrashRecorder()
    let scanner = MacScanner(trash: { recorder.append($0) })
    let report = await scanner.scan(category: .largeFiles, roots: [f.root], minimumBytes: 0)
    try Data(repeating: 66, count: 30).write(to: changed)
    let result = await scanner.moveToTrash(ids: report.items.map(\.id) + ["invented"])
    #expect(recorder.urls == [valid])
    #expect(result.trashedIDs.count == 1)
    #expect(result.failures.count == 2)
    #expect(FileManager.default.fileExists(atPath: valid.path))
    let again = await scanner.moveToTrash(ids: report.items.map(\.id))
    #expect(again.trashedIDs.isEmpty)
}

@Test func replacedAncestorWithSymlinkCannotBeTrashed() async throws {
    let f = try Fixture(); defer { f.clean() }
    _ = try f.write("folder/item.bin")
    let scanner = MacScanner(trash: { _ in Issue.record("Symlink escaped scope") })
    let report = await scanner.scan(category: .largeFiles, roots: [f.root], minimumBytes: 0)
    let folder = f.root.appendingPathComponent("folder")
    let moved = f.root.appendingPathComponent("moved")
    try FileManager.default.moveItem(at: folder, to: moved)
    try FileManager.default.createSymbolicLink(at: folder, withDestinationURL: moved)
    let result = await scanner.moveToTrash(ids: report.items.map(\.id))
    #expect(result.trashedIDs.isEmpty)
    #expect(result.failures.count == 1)
}

@Test func forbiddenAndMissingRootsArePartial() async throws {
    let f = try Fixture(); defer { f.clean() }
    _ = try f.write("Mail/inbox.bin")
    let scanner = MacScanner(trash: { _ in Issue.record("Unexpected trash") })
    let report = await scanner.scan(category: .largeFiles, roots: [f.root.appendingPathComponent("Mail"), f.root.appendingPathComponent("missing")], minimumBytes: 0)
    #expect(report.isPartial)
    #expect(report.items.isEmpty)
    #expect(report.inspectedCount == 0)
    let production = MacScanner(entryLimit: 100, fixtureScopes: false, trash: { _ in Issue.record("Unexpected trash") })
    let invalidScope = await production.scan(category: .caches, roots: [f.root])
    #expect(invalidScope.isPartial && invalidScope.items.isEmpty)
}

@Test func limitsAndCancellation() async throws {
    let f = try Fixture(); defer { f.clean() }
    for i in 0..<120 { _ = try f.write("\(i).bin") }
    let scanner = MacScanner(entryLimit: 4, trash: { _ in Issue.record("Unexpected trash") })
    let limited = await scanner.scan(category: .largeFiles, roots: [f.root], minimumBytes: 0)
    #expect(limited.inspectedCount == 4)
    #expect(limited.items.count == 4)
    #expect(limited.isPartial)
    let task = Task {
        while !Task.isCancelled { await Task.yield() }
        return await scanner.scan(category: .largeFiles, roots: [])
    }
    task.cancel()
    let cancelled = await task.value
    #expect(cancelled.cancelled && cancelled.isPartial)
}

@Test func rescansInvalidateOnlyMatchingScopesAndCategories() async throws {
    let f = try Fixture(); defer { f.clean() }
    _ = try f.write("one/file.bin")
    _ = try f.write("two/file.bin")
    let recorder = TrashRecorder()
    let scanner = MacScanner(trash: { recorder.append($0) })
    let first = await scanner.scan(category: .logs, roots: [f.root.appendingPathComponent("one")])
    let second = await scanner.scan(category: .logs, roots: [f.root.appendingPathComponent("two")])
    let newer = await scanner.scan(category: .logs, roots: [f.root.appendingPathComponent("one")])
    let result = await scanner.moveToTrash(ids: first.items.map(\.id) + second.items.map(\.id) + newer.items.map(\.id))
    #expect(result.trashedIDs.count == 2)
    #expect(result.failures.count == 1)
}

@Test func ageFilterAndRunningApps() async throws {
    let f = try Fixture(); defer { f.clean() }
    let old = try f.write("old.log")
    _ = try f.write("fresh.log")
    try FileManager.default.setAttributes([.modificationDate: Date(timeIntervalSinceNow: -900_000)], ofItemAtPath: old.path)
    let scanner = MacScanner(trash: { _ in Issue.record("Running app trashed") })
    let logs = await scanner.scan(category: .logs, roots: [f.root], olderThan: Date(timeIntervalSinceNow: -600_000))
    #expect(logs.items.map(\.url) == [old])
    _ = try f.write("Example.app/Contents/MacOS/Example", bytes: 20)
    _ = try f.write("Other.app/Contents/MacOS/Other", bytes: 20)
    try f.identify("Example.app", as: "org.example.first"); try f.identify("Other.app", as: "org.example.second")
    let apps = await scanner.scan(category: .applications, roots: [f.root])
    #expect(apps.items.count == 2)
    let result = await scanner.moveToTrash(ids: apps.items.map(\.id), runningApplicationPaths: Set(apps.items.map { $0.url.path }))
    #expect(result.trashedIDs.isEmpty)
    #expect(result.failures.count == 2)
}

@Test func identityReplacementAndTrashFailure() async throws {
    let f = try Fixture(); defer { f.clean() }
    let url = try f.write("replace.bin", bytes: 12)
    let scanner = MacScanner(trash: { _ in throw CocoaError(.fileWriteNoPermission) })
    let first = await scanner.scan(category: .largeFiles, roots: [f.root], minimumBytes: 0)
    let replacement = try f.write("replacement.bin", bytes: 12)
    try FileManager.default.removeItem(at: url)
    try FileManager.default.moveItem(at: replacement, to: url)
    let rejected = await scanner.moveToTrash(ids: first.items.map(\.id))
    #expect(rejected.trashedIDs.isEmpty && rejected.failures.count == 1)
    let second = await scanner.scan(category: .largeFiles, roots: [f.root], minimumBytes: 0)
    let failed = await scanner.moveToTrash(ids: second.items.map(\.id))
    #expect(failed.trashedIDs.isEmpty && failed.failures.count == 1)
    #expect(FileManager.default.fileExists(atPath: url.path))
}

@Test func inaccessibleDirectoryIsPartial() async throws {
    let f = try Fixture(); defer { f.clean() }
    _ = try f.write("denied/item.bin")
    let denied = f.root.appendingPathComponent("denied")
    try FileManager.default.setAttributes([.posixPermissions: 0], ofItemAtPath: denied.path)
    defer { try? FileManager.default.setAttributes([.posixPermissions: 0o700], ofItemAtPath: denied.path) }
    let scanner = MacScanner(trash: { _ in Issue.record("Unexpected trash") })
    let report = await scanner.scan(category: .largeFiles, roots: [f.root], minimumBytes: 0)
    #expect(report.isPartial)
    #expect(report.items.isEmpty)
}

@Test func productionLargeFileScopesRejectSystemAndLibrary() async throws {
    let f = try Fixture(); defer { f.clean() }
    _ = try f.write("should-not-be-read.bin")
    let scanner = MacScanner(fixtureScopes: false, trash: { _ in Issue.record("Unexpected trash") })
    for root in [f.root, URL(fileURLWithPath: "/usr"), URL(fileURLWithPath: "/Library"), URL(fileURLWithPath: "/private"), FileManager.default.homeDirectoryForCurrentUser] {
        let report = await scanner.scan(category: .largeFiles, roots: [root], minimumBytes: 0)
        #expect(report.items.isEmpty && report.inspectedCount == 0 && report.isPartial)
    }
}

@Test func applicationTreeChangesAndPartialBundles() async throws {
    let f = try Fixture(); defer { f.clean() }
    let executable = try f.write("Valid.app/Contents/MacOS/Valid", bytes: 50)
    try f.identify("Valid.app", as: "org.example.valid")
    _ = try f.write("Blocked.app/Contents/.ssh/secret", bytes: 10)
    try f.identify("Blocked.app", as: "org.example.blocked")
    let identityBytes = try FileManager.default.attributesOfItem(atPath: f.root.appendingPathComponent("Valid.app/Contents/Info.plist").path)[.size] as? Int64 ?? 0
    let scanner = MacScanner(trash: { _ in Issue.record("Changed app trashed") })
    let apps = await scanner.scan(category: .applications, roots: [f.root])
    #expect(apps.items.count == 1)
    #expect(apps.items.first?.bytes == 50 + identityBytes)
    #expect(apps.warnings.contains { $0.contains("Blocked.app") && $0.contains("non verificabili") })
    #expect(apps.isPartial)
    try Data(repeating: 90, count: 20).write(to: executable)
    let result = await scanner.moveToTrash(ids: apps.items.map(\.id))
    #expect(result.trashedIDs.isEmpty && result.failures.count == 1)
}

@Test func liveProviderProtectsAppsStartedAfterSnapshot() async throws {
    let f = try Fixture(); defer { f.clean() }
    _ = try f.write("NewlyOpened.app/Contents/MacOS/Example", bytes: 20)
    try f.identify("NewlyOpened.app", as: "org.example.newlyopened.fixture")
    let scanner = MacScanner(trash: { _ in Issue.record("Newly active app or cache trashed") })
    let apps = await scanner.scan(category: .applications, roots: [f.root])
    let path = f.root.appendingPathComponent("NewlyOpened.app").path
    let result = await scanner.moveToTrash(ids: apps.items.map(\.id), liveRunningApplications: {
        await Task.yield()
        return RunningApplications(paths: [path], bundleIdentifiers: ["com.example.newlyopened"])
    })
    #expect(result.trashedIDs.isEmpty && result.failures.count == 1)
}

@Test func providerSuspensionRequiresFinalIdentityCheck() async throws {
    let f = try Fixture(); defer { f.clean() }
    let url = try f.write("item.bin")
    let scanner = MacScanner(trash: { _ in Issue.record("File modified during provider await was trashed") })
    let scan = await scanner.scan(category: .largeFiles, roots: [f.root], minimumBytes: 0)
    let result = await scanner.moveToTrash(ids: scan.items.map(\.id), liveRunningApplications: {
        try? Data(repeating: 90, count: 100).write(to: url)
        return RunningApplications(paths: [], bundleIdentifiers: [])
    })
    #expect(result.trashedIDs.isEmpty && result.failures.count == 1)
}

@Test func cloudDocumentScopePolicyDoesNotTouchProviders() {
    let home = "/Users/SyntheticLindoUser"
    for path in [home + "/Library/CloudStorage", home + "/Library/CloudStorage/ExampleProvider/Projects", home + "/Library/Mobile Documents", home + "/Library/Mobile Documents/com~apple~CloudDocs/Reports", home + "/Downloads"] {
        #expect(MacScanner.allowsLargeFilePath(path, home: home))
    }
    for path in [home, home + "/Library", home + "/Library/Caches", home + "/Library/CloudStorageBackup", home + "/Library/Mobile Documents-old", home + "/Library/CloudStorage/.secret", "/Library/CloudStorage", "/private/Library/CloudStorage"] {
        #expect(!MacScanner.allowsLargeFilePath(path, home: home))
    }
}

@Test func applicationsRequireCompleteLiveAndFinalProcessInventories() async throws {
    let f = try Fixture(); defer { f.clean() }
    let executable = try f.write("Example.app/Contents/MacOS/Example")
    try f.identify("Example.app", as: "org.example.inventory")
    let app = f.root.appendingPathComponent("Example.app")
    let recorder = TrashRecorder()
    let scanner = MacScanner(processInventory: { ProcessSnapshot(paths: [], isComplete: true) }, trash: { recorder.append($0) })
    let report = await scanner.scan(category: .applications, roots: [f.root])
    #expect(report.items.count == 1)
    let missing = await scanner.moveToTrash(ids: report.items.map(\.id))
    #expect(missing.trashedIDs.isEmpty && missing.failures.first?.contains("non completa") == true)
    let incomplete = await scanner.moveToTrash(ids: report.items.map(\.id), liveRunningApplications: {
        RunningApplications(paths: [], bundleIdentifiers: [], inspectionComplete: false)
    })
    #expect(incomplete.trashedIDs.isEmpty && incomplete.failures.first?.contains("non completa") == true)
    #expect(recorder.urls.isEmpty)
    for snapshot in [ProcessSnapshot(paths: [], isComplete: false), ProcessSnapshot(paths: [executable.path], isComplete: true)] {
        let blockedScanner = MacScanner(processInventory: { snapshot }, trash: { _ in Issue.record("Incomplete or active final inventory permitted trash") })
        let blockedReport = await blockedScanner.scan(category: .applications, roots: [f.root])
        let blocked = await blockedScanner.moveToTrash(ids: blockedReport.items.map(\.id), liveRunningApplications: {
            RunningApplications(paths: [], bundleIdentifiers: [], inspectionComplete: true)
        })
        #expect(blocked.trashedIDs.isEmpty && blocked.failures.first?.contains("finale") == true)
    }
    let allowed = await scanner.moveToTrash(ids: report.items.map(\.id), liveRunningApplications: {
        RunningApplications(paths: [], bundleIdentifiers: [], inspectionComplete: true)
    })
    #expect(allowed.trashedIDs.count == 1 && allowed.failures.isEmpty)
    #expect(recorder.urls == [app])
    #expect(FileManager.default.fileExists(atPath: executable.path))
}
