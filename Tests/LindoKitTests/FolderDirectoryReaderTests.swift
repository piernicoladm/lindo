import Foundation
import Darwin
import Testing
@testable import LindoKit

private struct FolderFixture {
    let home: URL
    init() throws {
        home = FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent("Library/Developer/Lindo/test-fixtures/FolderReader-" + UUID().uuidString)
        try FileManager.default.createDirectory(at: home, withIntermediateDirectories: true)
    }
    func folder(_ path: String) throws -> URL {
        let url = home.appendingPathComponent(path)
        try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
        return url
    }
    func clean() { try? FileManager.default.removeItem(at: home) }
}

@Test func folderReaderListsDirectoriesAndSortsNaturally() async throws {
    let f = try FolderFixture(); defer { f.clean() }
    _ = try f.folder("Folder 10")
    let first = try f.folder("Folder 2")
    try Data([1]).write(to: f.home.appendingPathComponent("file.txt"))
    let reader = FolderDirectoryReader(home: f.home, executor: FolderReadExecutor())
    let home = try await reader.list(f.home)
    #expect(home.children.map(\.name) == ["Folder 2", "Folder 10"])
    #expect(!home.canSelect && home.parent == nil)
    #expect(home.children.first?.id == first.path)
    let empty = try await reader.list(first)
    #expect(empty.children.isEmpty && empty.canSelect && empty.parent?.path == f.home.path)
}

@Test func folderReaderRejectsProtectedAndUnsafePaths() async throws {
    let f = try FolderFixture(); defer { f.clean() }
    let allowed = try f.folder("Allowed")
    _ = try f.folder("Allowed/Nested")
    var hidden = try f.folder("FinderHidden/Nested").deletingLastPathComponent()
    var hiddenValues = URLResourceValues()
    hiddenValues.isHidden = true
    try hidden.setResourceValues(hiddenValues)
    let rejected = ["_6_Archivio_Private/secret", "Piernicola De Maria/private", "_2_Work/CLIENTS/customer", ".hidden/nested", "Bundle.app/nested", "Mail/inbox", "com.apple.data/nested", "Library/Caches"]
    for path in rejected { _ = try f.folder(path) }
    let link = f.home.appendingPathComponent("Shortcut")
    try FileManager.default.createSymbolicLink(at: link, withDestinationURL: allowed)
    let reader = FolderDirectoryReader(home: f.home, executor: FolderReadExecutor())
    for path in rejected + ["Shortcut", "Shortcut/Nested", "FinderHidden/Nested", "missing"] {
        await #expect(throws: (any Error).self) { try await reader.list(f.home.appendingPathComponent(path)) }
    }
    await #expect(throws: (any Error).self) { try await reader.list(f.home.deletingLastPathComponent()) }
    await #expect(throws: (any Error).self) { try await reader.list(URL(string: "https://example.com")!) }
    let listing = try await reader.list(f.home)
    #expect(Set(listing.children.map(\.name)) == ["Allowed", "_2_Work"])
    let work = try await reader.list(f.home.appendingPathComponent("_2_Work"))
    #expect(listing.warnings.contains { $0.contains("Mail") })
    #expect(work.children.isEmpty)
    #expect(work.warnings.contains { $0.contains("CLIENTS") })
}

@Test func folderReaderAllowsOnlyCloudLibraryBranches() async throws {
    let f = try FolderFixture(); defer { f.clean() }
    let cloud = try f.folder("Library/CloudStorage/Provider/Documents")
    let mobile = try f.folder("Library/Mobile Documents/App/Documents")
    _ = try f.folder("Library/Application Support/Secret")
    let reader = FolderDirectoryReader(home: f.home, executor: FolderReadExecutor())
    #expect(try await reader.list(cloud).canSelect)
    #expect(try await reader.list(mobile).canSelect)
    let cloudRoot = try await reader.list(f.home.appendingPathComponent("Library/CloudStorage"))
    #expect(cloudRoot.parent == nil)
    await #expect(throws: (any Error).self) { try await reader.list(f.home.appendingPathComponent("Library")) }
}

@Test func folderReaderCapsEntriesAndCancels() async throws {
    let f = try FolderFixture(); defer { f.clean() }
    for i in 0..<100 { _ = try f.folder("Folder \(i)") }
    let reader = FolderDirectoryReader(home: f.home, entryLimit: 4, executor: FolderReadExecutor())
    let limited = try await reader.list(f.home)
    #expect(limited.children.count == 4 && limited.warnings.count == 1)
    let task = Task {
        while !Task.isCancelled { await Task.yield() }
        return try await reader.list(f.home)
    }
    task.cancel()
    await #expect(throws: CancellationError.self) { try await task.value }
}

@Test func folderReaderRejectsUnreadableDirectory() async throws {
    let f = try FolderFixture(); defer { f.clean() }
    let denied = try f.folder("Denied")
    try FileManager.default.setAttributes([.posixPermissions: 0], ofItemAtPath: denied.path)
    defer { try? FileManager.default.setAttributes([.posixPermissions: 0o700], ofItemAtPath: denied.path) }
    let reader = FolderDirectoryReader(home: f.home, executor: FolderReadExecutor())
    await #expect(throws: (any Error).self) { try await reader.list(denied) }
    #expect(try await reader.list(f.home).children.isEmpty)
}

private final class FolderReadCounter: @unchecked Sendable {
    private let lock = NSLock()
    private var count = 0
    func increment() { lock.lock(); count += 1; lock.unlock() }
    var value: Int { lock.lock(); defer { lock.unlock() }; return count }
}

private func folderReadEventually(_ predicate: @Sendable () -> Bool) async -> Bool {
    for _ in 0..<100 {
        if predicate() { return true }
        try? await Task.sleep(for: .milliseconds(10))
    }
    return predicate()
}

@Test func folderReaderCancellationDoesNotWaitForBlockedIOOrReleaseItsSlot() async throws {
    let f = try FolderFixture(); defer { f.clean() }
    let executor = FolderReadExecutor()
    let started = FolderReadCounter()
    let firstReturned = FolderReadCounter()
    let thirdReturned = FolderReadCounter()
    let unblock = DispatchSemaphore(value: 0)
    let reader = FolderDirectoryReader(home: f.home, executor: executor, beforeRead: {
        started.increment()
        unblock.wait()
    })
    let first = Task {
        defer { firstReturned.increment() }
        do { _ = try await reader.list(f.home); return false }
        catch is CancellationError { return true }
        catch { return false }
    }
    let second = Task { try? await reader.list(f.home) }
    #expect(await folderReadEventually { started.value == 2 })
    first.cancel()
    #expect(await folderReadEventually { firstReturned.value == 1 })
    // A cancelled caller must not free a slot while its filesystem worker is stuck.
    let third = Task {
        defer { thirdReturned.increment() }
        do { _ = try await reader.list(f.home); return false }
        catch { return error.localizedDescription.contains("Due letture") }
    }
    #expect(await folderReadEventually { thirdReturned.value == 1 })
    #expect(started.value == 2)
    // Always release all possible workers, including when a preceding assertion fails.
    for _ in 0..<3 { unblock.signal() }
    #expect(await first.value)
    _ = await second.value
    #expect(await third.value)
    // The cancelled worker now exits and makes the executor usable again.
    let plain = FolderDirectoryReader(home: f.home, executor: executor)
    let recovered = Task {
        for _ in 0..<100 {
            if let listing = try? await plain.list(f.home) { return listing.directory.path == f.home.path }
            try? await Task.sleep(for: .milliseconds(10))
        }
        return false
    }
    #expect(await recovered.value)
}

@Test(arguments: [false, true])
func folderReaderRejectsReplacementBeforeEnumerating(rootAncestor: Bool) async throws {
    let f = try FolderFixture(); defer { f.clean() }
    let outside = try FolderFixture(); defer { outside.clean() }
    let selected = try f.folder("Selected/Nested")
    _ = try f.folder("Selected/Nested/Original")
    _ = try outside.folder("Nested/OutsideSentinel")
    _ = try outside.folder("OutsideSentinel")
    let replaced = rootAncestor ? selected.deletingLastPathComponent() : selected
    let saved = replaced.deletingLastPathComponent().appendingPathComponent("Saved")
    let reached = FolderReadCounter()
    let entriesRead = FolderReadCounter()
    let resume = DispatchSemaphore(value: 0)
    let reader = FolderDirectoryReader(home: f.home, executor: FolderReadExecutor(), beforeEnumeration: {
        reached.increment()
        resume.wait()
    }, didReadEntry: { _ in entriesRead.increment() })
    let task = Task { try await reader.list(selected) }
    defer { resume.signal() }
    #expect(await folderReadEventually { reached.value == 1 })
    // Replace either the selected directory or its parent after descriptors are pinned.
    try FileManager.default.moveItem(at: replaced, to: saved)
    try FileManager.default.createSymbolicLink(at: replaced, withDestinationURL: outside.home)
    resume.signal()
    await #expect(throws: (any Error).self) { try await task.value }
    // Detect enumeration itself, not merely omission of the forbidden result.
    #expect(entriesRead.value == 0)
    #expect(FileManager.default.fileExists(atPath: outside.home.appendingPathComponent("OutsideSentinel").path))
}

@Test func folderReaderRejectsFinderPackagesHiddenAndCloudPlaceholders() async throws {
    let f = try FolderFixture(); defer { f.clean() }
    _ = try f.folder("Allowed")
    _ = try f.folder("Pending.icloud")
    for (name, flag) in [("FinderPackage", UInt8(0x20)), ("FinderInvisible", UInt8(0x40))] {
        let directory = try f.folder(name)
        var finderInfo = [UInt8](repeating: 0, count: 32)
        finderInfo[8] = flag
        let result = finderInfo.withUnsafeBytes {
            setxattr(directory.path, "com.apple.FinderInfo", $0.baseAddress, $0.count, 0, 0)
        }
        #expect(result == 0)
    }
    let reader = FolderDirectoryReader(home: f.home, executor: FolderReadExecutor())
    #expect(try await reader.list(f.home).children.map(\.name) == ["Allowed"])
    for name in ["FinderPackage", "FinderInvisible", "Pending.icloud"] {
        await #expect(throws: (any Error).self) { try await reader.list(f.home.appendingPathComponent(name)) }
    }
}

@Test func folderReaderCountsHiddenEntriesTowardBudget() async throws {
    let f = try FolderFixture(); defer { f.clean() }
    for name in [".one", ".two", ".three"] { _ = try f.folder(name) }
    let entriesRead = FolderReadCounter()
    let reader = FolderDirectoryReader(home: f.home, entryLimit: 2, executor: FolderReadExecutor(),
        didReadEntry: { name in
            if name != "." && name != ".." { entriesRead.increment() }
        })
    let listing = try await reader.list(f.home)
    #expect(listing.children.isEmpty)
    #expect(listing.warnings.count == 1)
    #expect(entriesRead.value == 2)
}
