import Foundation
import Testing
@testable import LindoKit

private struct DuplicateFixture {
    let root: URL
    init() throws {
        root = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Developer/Lindo/test-fixtures/Duplicates-" + UUID().uuidString)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
    }
    func write(_ name: String, _ text: String = "identical bytes") throws -> URL {
        let url = root.appendingPathComponent(name)
        try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        try Data(text.utf8).write(to: url)
        return url
    }
    func clean() { try? FileManager.default.removeItem(at: root) }
}
private final class DuplicateTrash: @unchecked Sendable {
    private let lock = NSLock()
    private var stored: [URL] = []
    func add(_ url: URL) { lock.lock(); defer { lock.unlock() }; stored.append(url) }
    var urls: [URL] { lock.lock(); defer { lock.unlock() }; return stored }
}

@Test func duplicatesUseBytesAndExcludeUnsafeItems() async throws {
    let f = try DuplicateFixture(); defer { f.clean() }
    let a = try f.write("a.txt"), b = try f.write("different-name.bin")
    _ = try f.write("one/same.bin", "1111")
    _ = try f.write("two/same.bin", "2222")
    _ = try f.write("same-size.bin", "different bytes")
    _ = try f.write("zero", "")
    _ = try f.write("zero2", "")
    _ = try f.write("excluded.pages/inside")
    _ = try f.write("excluded.icloud")
    _ = try f.write("_6_Archivio_Private/inside")
    try FileManager.default.linkItem(at: a, to: f.root.appendingPathComponent("hardlink"))
    try FileManager.default.createSymbolicLink(at: f.root.appendingPathComponent("symlink"), withDestinationURL: a)
    let scanner = DuplicateScanner(trash: { _ in Issue.record("Unexpected trash") })
    let report = await scanner.scan(roots: [f.root])
    #expect(report.groups.count == 1)
    let group = try #require(report.groups.first)
    #expect(group.files.count == 2)
    #expect(group.files.contains { $0.url == b })
    #expect(group.files.contains { $0.url == a || $0.url.lastPathComponent == "hardlink" })
    #expect(group.digest.count == 64)
    #expect(report.isPartial)
    #expect(report.warnings.contains { $0.contains("Elemento escluso dalle protezioni:") })
}

@Test func duplicateTrashRejectsModifiedAndMissingKeeper() async throws {
    for mode in ["modifiedKeeper", "missingKeeper", "modifiedTarget"] {
        let f = try DuplicateFixture(); defer { f.clean() }
        _ = try f.write("a"); _ = try f.write("b")
        let recorder = DuplicateTrash(), scanner = DuplicateScanner(trash: { recorder.add($0) })
        let report = await scanner.scan(roots: [f.root])
        let group = try #require(report.groups.first)
        let keeper = try #require(group.files.first { $0.id == group.keeperID })
        let target = try #require(group.files.first { $0.id != group.keeperID })
        if mode == "missingKeeper" { try FileManager.default.removeItem(at: keeper.url) }
        else { try Data("xxxxxxxxxxxxxxx".utf8).prefix(Int(target.bytes)).write(to: mode == "modifiedTarget" ? target.url : keeper.url) }
        let result = await scanner.moveToTrash(ids: [target.id], keepers: [group.id: keeper.id])
        #expect(result.trashedIDs.isEmpty && !result.failures.isEmpty)
        #expect(recorder.urls.isEmpty)
    }
}

@Test func duplicateSelectionRequiresUnselectedReviewedKeeper() async throws {
    let f = try DuplicateFixture(); defer { f.clean() }
    _ = try f.write("a"); _ = try f.write("b"); _ = try f.write("c")
    let recorder = DuplicateTrash(), scanner = DuplicateScanner(trash: { recorder.add($0) })
    let scan = await scanner.scan(roots: [f.root]), group = try #require(scan.groups.first)
    let blocked = await scanner.moveToTrash(ids: group.files.map(\.id), keepers: [group.id: group.keeperID])
    #expect(blocked.trashedIDs.isEmpty)
    let wrong = await scanner.moveToTrash(ids: [group.files[1].id], keepers: [group.id: "invented"])
    #expect(wrong.trashedIDs.isEmpty)
    // Review may choose a different surviving copy.
    let result = await scanner.moveToTrash(ids: [group.files[0].id, group.files[1].id], keepers: [group.id: group.files[2].id])
    #expect(result.trashedIDs.count == 2 && result.failures.isEmpty)
    #expect(recorder.urls.count == 2)
}

@Test func duplicateRescanInvalidatesAllOldIDs() async throws {
    let f = try DuplicateFixture(); defer { f.clean() }
    _ = try f.write("a"); _ = try f.write("b")
    let scanner = DuplicateScanner(trash: { _ in Issue.record("Old ID accepted") })
    let first = await scanner.scan(roots: [f.root]), group = try #require(first.groups.first)
    _ = await scanner.scan(roots: [])
    let result = await scanner.moveToTrash(ids: [group.files[1].id], keepers: [group.id: group.keeperID])
    #expect(result.trashedIDs.isEmpty && result.failures.count == 1)
}

@Test func duplicateTrashFailureAndKeeperLossLeaveOthersSafe() async throws {
    let f = try DuplicateFixture(); defer { f.clean() }
    _ = try f.write("a"); _ = try f.write("b"); _ = try f.write("c")
    let recorder = DuplicateTrash()
    let scanner = DuplicateScanner(trash: { url in
        recorder.add(url)
        throw CocoaError(.fileWriteNoPermission)
    })
    let scan = await scanner.scan(roots: [f.root]), group = try #require(scan.groups.first)
    let result = await scanner.moveToTrash(ids: group.files.filter { $0.id != group.keeperID }.map(\.id), keepers: [group.id: group.keeperID])
    #expect(result.trashedIDs.isEmpty && result.failures.count == 2)
    #expect(recorder.urls.count == 2)
    #expect(group.files.allSatisfy { FileManager.default.fileExists(atPath: $0.url.path) })
}

@Test func duplicateTrashCanRetryWithoutRescanAfterFailure() async throws {
    let f = try DuplicateFixture(); defer { f.clean() }
    _ = try f.write("a"); _ = try f.write("b")
    let recorder = DuplicateTrash()
    let scanner = DuplicateScanner(trash: { url in
        recorder.add(url)
        if recorder.urls.count == 1 { throw CocoaError(.fileWriteNoPermission) }
        try FileManager.default.removeItem(at: url)
    })
    let report = await scanner.scan(roots: [f.root])
    let group = try #require(report.groups.first)
    let target = try #require(group.files.first { $0.id != group.keeperID })
    let failed = await scanner.moveToTrash(ids: [target.id], keepers: [group.id: group.keeperID])
    #expect(failed.trashedIDs.isEmpty && failed.failures.count == 1)
    #expect(FileManager.default.fileExists(atPath: target.url.path))
    let retried = await scanner.moveToTrash(ids: [target.id], keepers: [group.id: group.keeperID])
    #expect(retried.trashedIDs == [target.id] && retried.failures.isEmpty)
    #expect(recorder.urls.count == 2)
    #expect(!FileManager.default.fileExists(atPath: target.url.path))
    let consumed = await scanner.moveToTrash(ids: [target.id], keepers: [group.id: group.keeperID])
    #expect(consumed.trashedIDs.isEmpty && consumed.failures.count == 1)
    #expect(recorder.urls.count == 2)
}

@Test func duplicateRetryRevalidatesKeeperAfterTrashFailure() async throws {
    let f = try DuplicateFixture(); defer { f.clean() }
    _ = try f.write("a"); _ = try f.write("b")
    let recorder = DuplicateTrash()
    let scanner = DuplicateScanner(trash: { url in
        recorder.add(url)
        throw CocoaError(.fileWriteNoPermission)
    })
    let report = await scanner.scan(roots: [f.root])
    let group = try #require(report.groups.first)
    let target = try #require(group.files.first { $0.id != group.keeperID })
    let keeper = try #require(group.files.first { $0.id == group.keeperID })
    let failed = await scanner.moveToTrash(ids: [target.id], keepers: [group.id: keeper.id])
    #expect(failed.trashedIDs.isEmpty && failed.failures.count == 1)
    try FileManager.default.removeItem(at: keeper.url)
    let retried = await scanner.moveToTrash(ids: [target.id], keepers: [group.id: keeper.id])
    #expect(retried.trashedIDs.isEmpty && retried.failures.count == 1)
    #expect(recorder.urls.count == 1)
    #expect(FileManager.default.fileExists(atPath: target.url.path))
}

@Test func duplicateLimitsCancellationAndSupersessionArePartial() async throws {
    let f = try DuplicateFixture(); defer { f.clean() }
    for i in 0..<8 { _ = try f.write("\(i)") }
    let scanner = DuplicateScanner(entryLimit: 2, trash: { _ in Issue.record("Unexpected trash") })
    let report = await scanner.scan(roots: [f.root])
    #expect(report.isPartial && report.inspectedCount == 2)
    let task = Task {
        while !Task.isCancelled { await Task.yield() }
        return await scanner.scan(roots: [f.root])
    }
    task.cancel()
    let cancelled = await task.value
    #expect(cancelled.cancelled && cancelled.isPartial && cancelled.groups.isEmpty)
    let full = DuplicateScanner(trash: { _ in Issue.record("Superseded ID accepted") })
    let superseded = await full.scan(roots: [f.root], progress: { _ in _ = await full.scan(roots: []) })
    #expect(superseded.cancelled && superseded.groups.isEmpty)
}

@Test func duplicateKeeperLossDuringBatchStopsFollowingTrash() async throws {
    let f = try DuplicateFixture(); defer { f.clean() }
    let keeperURL = try f.write("a"); _ = try f.write("b"); _ = try f.write("c")
    let recorder = DuplicateTrash()
    let scanner = DuplicateScanner(trash: { url in
        recorder.add(url)
        try FileManager.default.removeItem(at: url)
        try FileManager.default.removeItem(at: keeperURL)
    })
    let scan = await scanner.scan(roots: [f.root]), group = try #require(scan.groups.first)
    let result = await scanner.moveToTrash(ids: group.files.filter { $0.id != group.keeperID }.map(\.id), keepers: [group.id: group.keeperID])
    #expect(result.trashedIDs.count == 1 && result.failures.count == 1)
    #expect(recorder.urls.count == 1)
    #expect(group.files.filter { FileManager.default.fileExists(atPath: $0.url.path) }.count == 1)
}
