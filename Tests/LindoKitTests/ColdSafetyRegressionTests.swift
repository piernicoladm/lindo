import Foundation
import Darwin
import Testing
@testable import LindoKit

private struct ColdSafetyFixture {
    let root: URL
    init() throws {
        root = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Developer/Lindo/test-fixtures/ColdSafety-" + UUID().uuidString)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
    }
    func write(_ path: String) throws -> URL {
        let url = root.appendingPathComponent(path)
        try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        try Data("identical data fork".utf8).write(to: url)
        return url
    }
    func resourceFork(_ url: URL, text: String) throws {
        let data = Data(text.utf8)
        let status = data.withUnsafeBytes { setxattr(url.path, "com.apple.ResourceFork", $0.baseAddress, $0.count, 0, XATTR_NOFOLLOW) }
        guard status == 0 else { throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO) }
    }
    func clean() { try? FileManager.default.removeItem(at: root) }
}

@Test func coldSafetyRejectsPreservedMtimeRewrite() async throws {
    let f = try ColdSafetyFixture(); defer { f.clean() }
    let url = try f.write("changed.bin")
    var before = stat()
    #expect(lstat(url.path, &before) == 0)
    let scanner = MacScanner(trash: { _ in Issue.record("Rewritten file was trashed") })
    let scan = await scanner.scan(category: .largeFiles, roots: [f.root], minimumBytes: 0)
    #expect(scan.items.count == 1)
    let fd = open(url.path, O_WRONLY | O_NOFOLLOW)
    guard fd >= 0 else { throw POSIXError(.EIO) }
    defer { close(fd) }
    let replacement = Data(repeating: 90, count: Int(before.st_size))
    #expect(replacement.withUnsafeBytes { Darwin.write(fd, $0.baseAddress, $0.count) } == replacement.count)
    var times = [before.st_atimespec, before.st_mtimespec]
    #expect(futimens(fd, &times) == 0)
    let result = await scanner.moveToTrash(ids: scan.items.map(\.id))
    #expect(result.trashedIDs.isEmpty && result.failures.count == 1)
}

@Test(arguments: [
    ("Google/Chrome/Default/Cache/index", "/Synthetic/Renamed.app/Contents/MacOS/Google Chrome"),
    ("Google/Chrome/Default/Cache/index", "/Synthetic/Renamed.app/Contents/Frameworks/Google Chrome Framework.framework/Helpers/Google Chrome Helper.app/Contents/MacOS/Google Chrome Helper"),
    ("Chromium/Default/Cache/index", "/Synthetic/Renamed.app/Contents/MacOS/Chromium"),
    ("Chromium/Default/Cache/index", "/Synthetic/Renamed.app/Contents/Frameworks/Chromium Framework.framework/Helpers/Chromium Helper.app/Contents/MacOS/Chromium Helper")
])
func coldSafetyRenamedBrowserFinalGate(cachePath: String, executablePath: String) async throws {
    let f = try ColdSafetyFixture(); defer { f.clean() }
    _ = try f.write(cachePath)
    let scanner = MacScanner(cacheRoot: f.root, processInventory: {
        ProcessSnapshot(paths: [executablePath], isComplete: true)
    }, trash: { _ in Issue.record("Renamed browser cache was trashed") })
    let scan = await scanner.scan(category: .caches, roots: [f.root])
    #expect(scan.items.count == 1)
    let result = await scanner.moveToTrash(ids: scan.items.map(\.id), liveRunningApplications: {
        RunningApplications(paths: [], bundleIdentifiers: [])
    })
    #expect(result.trashedIDs.isEmpty && result.failures.first?.contains("finale") == true)
}

@Test func coldSafetyResourceForkExcludesDuplicate() async throws {
    let f = try ColdSafetyFixture(); defer { f.clean() }
    let a = try f.write("a"), b = try f.write("b")
    try f.resourceFork(a, text: "unique first fork")
    try f.resourceFork(b, text: "unique second fork")
    let scanner = DuplicateScanner(trash: { _ in Issue.record("Forked file was trashed") })
    let scan = await scanner.scan(roots: [f.root])
    #expect(scan.groups.isEmpty)
    #expect(scan.isPartial && scan.warnings.contains { $0.contains("resource fork") })
}

@Test(arguments: [false, true])
func coldSafetyResourceForkAddedBeforeTrash(keeperChanges: Bool) async throws {
    let f = try ColdSafetyFixture(); defer { f.clean() }
    _ = try f.write("a"); _ = try f.write("b")
    let scanner = DuplicateScanner(trash: { _ in Issue.record("Fork added before trash was ignored") })
    let scan = await scanner.scan(roots: [f.root])
    let group = try #require(scan.groups.first)
    let keeper = try #require(group.files.first { $0.id == group.keeperID })
    let target = try #require(group.files.first { $0.id != group.keeperID })
    try f.resourceFork(keeperChanges ? keeper.url : target.url, text: "new unique resource")
    let result = await scanner.moveToTrash(ids: [target.id], keepers: [group.id: keeper.id])
    #expect(result.trashedIDs.isEmpty)
    #expect(result.failures.contains { $0.contains("resource fork") })
}

@Test func coldSafetyCancellationInFinalInventoryVetoesTrash() async throws {
    let f = try ColdSafetyFixture(); defer { f.clean() }
    _ = try f.write("Chromium/Default/Cache/index")
    let scanner = MacScanner(cacheRoot: f.root, processInventory: {
        // Cancel the operation deterministically after all tree checks have passed.
        withUnsafeCurrentTask { $0?.cancel() }
        return ProcessSnapshot(paths: [], isComplete: true)
    }, trash: { _ in Issue.record("Cancelled operation still trashed cache") })
    let scan = await scanner.scan(category: .caches, roots: [f.root])
    let task = Task {
        await scanner.moveToTrash(ids: scan.items.map(\.id), liveRunningApplications: {
            RunningApplications(paths: [], bundleIdentifiers: [])
        })
    }
    let result = await task.value
    #expect(result.trashedIDs.isEmpty && result.failures.count == 1)
}

@Test func coldSafetyRenamedBrowserMatcherRejectsUnrelatedNames() throws {
    let root = URL(fileURLWithPath: "/Synthetic/Caches")
    let chrome = try #require(CachePolicy.owner(of: root.appendingPathComponent("Google/Chrome/Default/Cache"), root: root))
    for path in ["/Synthetic/Renamed.app/Contents/MacOS/Google Chrome-notes", "/Synthetic/Renamed.app/Contents/Resources/Google Chrome", "Renamed.app/Contents/MacOS/Google Chrome", "/Synthetic/Renamed.app.backup/Contents/MacOS/Google Chrome"] {
        #expect(!CachePolicy.isRunning(chrome, paths: [path], identifiers: []))
    }
    #expect(CachePolicy.isRunning(chrome, paths: ["/Synthetic/Renamed.app/Contents/Frameworks/Helper.app/Contents/MacOS/Google Chrome Helper (Renderer)"], identifiers: []))
}
