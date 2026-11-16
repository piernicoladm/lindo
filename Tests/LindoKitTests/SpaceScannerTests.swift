import Foundation
import Darwin
import Testing
@testable import LindoKit

private struct SpaceFixture {
    let root: URL
    init() throws {
        root = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Developer/Lindo/test-fixtures/space-" + UUID().uuidString)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
    }
    func write(_ name: String, bytes: Int = 12) throws -> URL {
        let url = root.appendingPathComponent(name)
        try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        try Data(repeating: 65, count: bytes).write(to: url)
        return url
    }
    func info(_ url: URL) throws -> stat {
        var result = stat()
        guard lstat(url.path, &result) == 0 else { throw CocoaError(.fileReadNoSuchFile) }
        return result
    }
    func clean() { try? FileManager.default.removeItem(at: root) }
}

private final class SpaceBlockedRead: @unchecked Sendable {
    private let lock = NSLock()
    private let gate = DispatchSemaphore(value: 0)
    private var entered = false
    private var exited = false
    var didEnter: Bool { lock.withLock { entered } }
    var didExit: Bool { lock.withLock { exited } }
    func block() {
        lock.withLock { entered = true }
        // Watchdog only: successful cancellation must return before this read exits.
        _ = gate.wait(timeout: .now() + 10)
        lock.withLock { exited = true }
    }
    func release() { gate.signal() }
}

@Test func spaceScannerAggregatesImmediateChildrenFromMetadata() async throws {
    let fixture = try SpaceFixture(); defer { fixture.clean() }
    let first = try fixture.write("Folder/first.bin", bytes: 19)
    let second = try fixture.write("second.bin", bytes: 41)
    let folder = first.deletingLastPathComponent()
    let metadata = try [first, second, folder].map { try fixture.info($0) }
    let scanner = SpaceScanner(executor: ScanReadExecutor())

    let report = await scanner.scan(root: fixture.root)

    #expect(report.entries.count == 2)
    #expect(report.fileCount == 2)
    #expect(report.inspectedCount == 3)
    #expect(report.logicalBytes == metadata.reduce(Int64(0)) { $0 + Int64($1.st_size) })
    #expect(report.allocatedBytes == metadata.reduce(Int64(0)) { $0 + Int64($1.st_blocks) * 512 })
    #expect(report.entries.map(\.allocatedBytes) == report.entries.map(\.allocatedBytes).sorted(by: >))
    #expect(report.entries.allSatisfy { $0.id == $0.url.path })
    #expect(report.entries.first(where: { $0.url.lastPathComponent == "Folder" })?.isDirectory == true)
    #expect(!report.isPartial && !report.cancelled)
}

@Test func spaceScannerDeduplicatesHardlinksWithoutFollowingSymlinks() async throws {
    let fixture = try SpaceFixture(); defer { fixture.clean() }
    let original = try fixture.write("original.bin", bytes: 4_096)
    let linked = fixture.root.appendingPathComponent("hardlink.bin")
    try FileManager.default.linkItem(at: original, to: linked)
    try FileManager.default.createSymbolicLink(at: fixture.root.appendingPathComponent("shortcut"), withDestinationURL: original)
    let metadata = try fixture.info(original)
    let report = await SpaceScanner(executor: ScanReadExecutor()).scan(root: fixture.root)

    #expect(report.fileCount == 1)
    #expect(report.logicalBytes == 4_096)
    #expect(report.allocatedBytes == Int64(metadata.st_blocks) * 512)
    #expect(report.entries.count == 2)
    #expect(report.isPartial)
    #expect(report.entries.reduce(0) { $0 + $1.alreadyCountedLinks } == 1)
    #expect(report.warnings.contains { $0.contains("shortcut") })
}

@Test func spaceHardlinkAttributionIsExplicitAcrossFolders() async throws {
    let fixture = try SpaceFixture(); defer { fixture.clean() }
    let original = try fixture.write("First/data.bin", bytes: 4096)
    let second = fixture.root.appendingPathComponent("Second")
    try FileManager.default.createDirectory(at: second, withIntermediateDirectories: true)
    try FileManager.default.linkItem(at: original, to: second.appendingPathComponent("same.bin"))
    let scanner = SpaceScanner(executor: ScanReadExecutor())
    let report = await scanner.scan(root: fixture.root)
    #expect(report.fileCount == 1)
    #expect(report.entries.reduce(0) { $0 + $1.alreadyCountedLinks } == 1)
    let attributedElsewhere = try #require(report.entries.first { $0.alreadyCountedLinks == 1 })
    #expect(attributedElsewhere.fileCount == 0)
    let nested = await scanner.scan(root: attributedElsewhere.url)
    #expect(nested.fileCount == 1)
    #expect(nested.logicalBytes == 4096)
    #expect(nested.entries.allSatisfy { $0.alreadyCountedLinks == 0 })
}

@Test func readOnlyMapMeasuresPackageMetadata() async throws {
    let fixture = try SpaceFixture(); defer { fixture.clean() }
    _ = try fixture.write("Example.app/Contents/payload.bin", bytes: 1024)
    _ = try fixture.write("Document.pages/Data/payload.bin", bytes: 512)
    let report = await SpaceScanner(executor: ScanReadExecutor()).scan(root: fixture.root)
    #expect(report.fileCount == 2)
    #expect(report.entries.count == 2)
    #expect(!report.isPartial)
}

@Test func spaceScannerIncludesLibraryHiddenAndOrdinaryAppleResourceNames() async throws {
    let fixture = try SpaceFixture(); defer { fixture.clean() }
    for name in ["Library/Caches", ".hidden", "Photos", "System", "Apple", "com.apple.resources"] {
        _ = try fixture.write(name + "/data.bin")
    }
    let report = await SpaceScanner(executor: ScanReadExecutor()).scan(root: fixture.root)

    #expect(report.fileCount == 6)
    #expect(Set(report.entries.map { $0.url.lastPathComponent }) == Set(["Library", ".hidden", "Photos", "System", "Apple", "com.apple.resources"]))
    #expect(!report.isPartial)
}

@Test func spaceScannerExcludesPrivateCredentialAndCloudPlaceholderPaths() async throws {
    let fixture = try SpaceFixture(); defer { fixture.clean() }
    for name in ["_6_Archivio_Private", "Piernicola De Maria", "_2_Work/CLIENTS", ".ssh", ".gnupg", ".aws", ".codex", "Keychains", "Mail", "Messages", "Trash", ".Trash"] {
        _ = try fixture.write(name + "/secret.bin")
    }
    _ = try fixture.write("pending.icloud")
    let visible = try fixture.write("visible.bin", bytes: 5)
    let report = await SpaceScanner(executor: ScanReadExecutor()).scan(root: fixture.root)

    #expect(report.fileCount == 1)
    #expect(report.entries.contains { $0.url.path == visible.path })
    #expect(report.entries.filter { !$0.isDirectory }.count == 1)
    #expect(report.entries.first(where: { $0.url.lastPathComponent == "_2_Work" })?.isPartial == true)
    #expect(report.isPartial)
    #expect(report.warnings.count == 13)
}

@Test func spaceScannerRejectsForbiddenRootsAndSymlinkRootComponents() async throws {
    let fixture = try SpaceFixture(); defer { fixture.clean() }
    _ = try fixture.write("Mail/secret.bin")
    _ = try fixture.write("actual/child/file.bin")
    try FileManager.default.createSymbolicLink(at: fixture.root.appendingPathComponent("alias"), withDestinationURL: fixture.root.appendingPathComponent("actual"))
    let scanner = SpaceScanner(executor: ScanReadExecutor())
    for root in [fixture.root.appendingPathComponent("Mail"), fixture.root.appendingPathComponent("alias/child")] {
        let report = await scanner.scan(root: root)
        #expect(report.entries.isEmpty && report.inspectedCount == 0 && report.isPartial)
    }
    let production = await SpaceScanner().scan(root: URL(fileURLWithPath: "/"))
    #expect(production.entries.isEmpty && production.inspectedCount == 0 && production.isPartial)
}

@Test func spaceScannerCancellationReturnsWhileFilesystemWorkerIsBlocked() async throws {
    let blocked = SpaceBlockedRead()
    defer { blocked.release() }
    let fixture = try SpaceFixture(); defer { fixture.clean() }
    let scanner = SpaceScanner(executor: ScanReadExecutor(limit: 1), beforeRead: { blocked.block() })
    let task = Task { await scanner.scan(root: fixture.root) }
    defer { task.cancel() }
    let deadline = ContinuousClock.now.advanced(by: .seconds(5))
    while !blocked.didEnter && ContinuousClock.now < deadline { try await Task.sleep(for: .milliseconds(1)) }
    try #require(blocked.didEnter)
    task.cancel()
    let report = await task.value
    #expect(!blocked.didExit, "Cancellation waited for the blocked filesystem worker")
    #expect(report.cancelled && report.isPartial && report.entries.isEmpty)
}
