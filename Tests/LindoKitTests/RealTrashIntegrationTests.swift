import Foundation
import Darwin
import Testing
@testable import LindoKit

// Opt in explicitly. Every source is a newly created UUID fixture; restore only
// the exact destination URL returned by this operation, without browsing Trash.
private final class RealTrashFixture: @unchecked Sendable {
    let root: URL
    private let lock = NSLock()
    private var captured: [(original: URL, trashed: URL)] = []
    init() throws {
        root = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Developer/Lindo/test-fixtures/" + UUID().uuidString)
        guard !FileManager.default.fileExists(atPath: root.path) else { throw CocoaError(.fileWriteFileExists) }
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
    }
    func write(_ path: String, data: Data) throws -> URL {
        let url = root.appendingPathComponent(path)
        try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        try data.write(to: url)
        return url
    }
    func trash(_ url: URL) throws {
        lock.lock(); defer { lock.unlock() }
        guard url.standardizedFileURL.path.hasPrefix(root.path + "/"),
              UUID(uuidString: root.lastPathComponent) != nil else { throw CocoaError(.fileWriteNoPermission) }
        var ancestor = url.standardizedFileURL
        while true {
            var info = stat()
            guard lstat(ancestor.path, &info) == 0, info.st_mode & S_IFMT != S_IFLNK else { throw CocoaError(.fileWriteNoPermission) }
            if ancestor.path == "/" { break }
            ancestor.deleteLastPathComponent()
        }
        var destination: NSURL?
        try FileManager.default.trashItem(at: url, resultingItemURL: &destination)
        guard let destination else {
            Issue.record("OS Trash returned no destination for synthetic fixture: \(url.path)")
            throw CocoaError(.fileWriteUnknown)
        }
        captured.append((url, destination as URL))
    }
    var destinations: [URL] {
        lock.lock(); defer { lock.unlock() }
        return captured.map(\.trashed)
    }
    @discardableResult func restoreCaptured() -> Bool {
        lock.lock(); defer { lock.unlock() }
        var pending: [(original: URL, trashed: URL)] = []
        for pair in captured {
            do {
                // moveItem refuses to overwrite an existing path.
                try FileManager.default.moveItem(at: pair.trashed, to: pair.original)
            } catch {
                pending.append(pair)
                Issue.record("Synthetic fixture remains in Trash at exact captured path: \(pair.trashed.path). Restore to: \(pair.original.path). Error: \(error)")
            }
        }
        captured = pending
        return pending.isEmpty
    }
    func finish() {
        // Never remove the fixture root when a captured item still needs restoration.
        if restoreCaptured() { try? FileManager.default.removeItem(at: root) }
    }
}

@Test(.enabled(if: ProcessInfo.processInfo.environment["LINDO_REAL_TRASH_TEST"] == "1"))
func realTrashDuplicateRoundTrip() async throws {
    let fixture = try RealTrashFixture(); defer { fixture.finish() }
    let content = Data("synthetic duplicate payload".utf8)
    let keeperURL = try fixture.write("keeper.bin", data: content)
    let extraURL = try fixture.write("extra.bin", data: content)
    let different = try fixture.write("different.bin", data: Data(repeating: 42, count: content.count))
    let base = MacScanner(fixtureScopes: true, trash: { try fixture.trash($0) })
    let scanner = DuplicateScanner(scanner: base)
    let scan = await scanner.scan(roots: [fixture.root])
    #expect(!scan.isPartial)
    #expect(scan.groups.count == 1)
    let group = try #require(scan.groups.first)
    let keeper = try #require(group.files.first { $0.url == keeperURL })
    let extra = try #require(group.files.first { $0.url == extraURL })
    #expect(group.files.count == 2)
    let moved = await scanner.moveToTrash(ids: [extra.id], keepers: [group.id: keeper.id])
    #expect(moved.trashedIDs == [extra.id])
    #expect(moved.failures.isEmpty)
    #expect(try Data(contentsOf: keeperURL) == content)
    #expect(try Data(contentsOf: different) == Data(repeating: 42, count: content.count))
    #expect(!FileManager.default.fileExists(atPath: extraURL.path))
    let captured = try #require(fixture.destinations.first)
    #expect(fixture.destinations.count == 1)
    #expect(FileManager.default.fileExists(atPath: captured.path))
    #expect(fixture.restoreCaptured())
    #expect(try Data(contentsOf: extraURL) == content)
    #expect(try Data(contentsOf: keeperURL) == content)
    #expect(!FileManager.default.fileExists(atPath: captured.path))
}

@Test(.enabled(if: ProcessInfo.processInfo.environment["LINDO_REAL_TRASH_TEST"] == "1"))
func realTrashCacheRoundTrip() async throws {
    let fixture = try RealTrashFixture(); defer { fixture.finish() }
    let indexData = Data("synthetic Firefox index".utf8), entryData = Data("synthetic Firefox cache entry".utf8)
    let cache = fixture.root.appendingPathComponent("Firefox/Profiles/fixture.work/cache2")
    let index = try fixture.write("Firefox/Profiles/fixture.work/cache2/index", data: indexData)
    let entry = try fixture.write("Firefox/Profiles/fixture.work/cache2/entries/entry", data: entryData)
    let sentinel = try fixture.write("sentinel.txt", data: Data("root stays".utf8))
    let rootInode = try FileManager.default.attributesOfItem(atPath: fixture.root.path)[.systemFileNumber] as? NSNumber
    let scanner = MacScanner(fixtureScopes: true, cacheRoot: fixture.root, trash: { try fixture.trash($0) })
    let scan = await scanner.scan(category: .caches, roots: [fixture.root])
    #expect(!scan.isPartial)
    let unit = try #require(scan.items.first)
    #expect(scan.items.count == 1 && unit.url.path == cache.path)
    #expect(unit.ownerBundleIdentifier == "org.mozilla.firefox")
    let moved = await scanner.moveToTrash(ids: [unit.id], liveRunningApplications: {
        RunningApplications(paths: [], bundleIdentifiers: [], inspectionComplete: true)
    })
    #expect(moved.trashedIDs == [unit.id] && moved.failures.isEmpty)
    #expect(!FileManager.default.fileExists(atPath: cache.path))
    let captured = try #require(fixture.destinations.first)
    #expect(fixture.destinations.count == 1)
    #expect(try Data(contentsOf: captured.appendingPathComponent("index")) == indexData)
    #expect(try Data(contentsOf: captured.appendingPathComponent("entries/entry")) == entryData)
    #expect(FileManager.default.fileExists(atPath: cache.deletingLastPathComponent().path))
    #expect(try Data(contentsOf: sentinel) == Data("root stays".utf8))
    #expect(try FileManager.default.attributesOfItem(atPath: fixture.root.path)[.systemFileNumber] as? NSNumber == rootInode)
    #expect(fixture.restoreCaptured())
    #expect(try Data(contentsOf: index) == indexData)
    #expect(try Data(contentsOf: entry) == entryData)
    #expect(!FileManager.default.fileExists(atPath: captured.path))
}
