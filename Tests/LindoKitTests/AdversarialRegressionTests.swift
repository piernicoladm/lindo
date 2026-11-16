import Foundation
import Darwin
import Testing
@testable import LindoKit

// Regressions from the 2026-11-12 adversarial review. Each test fails when the
// production guard it names is removed; several did not exist before.

private final class AdversarialTrash: @unchecked Sendable {
    private let lock = NSLock()
    private var stored: [URL] = []
    func add(_ url: URL) { lock.lock(); defer { lock.unlock() }; stored.append(url) }
    var urls: [URL] { lock.lock(); defer { lock.unlock() }; return stored }
}

private struct AdversarialFixture {
    let root: URL

    init() throws {
        root = FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent("Library/Developer/Lindo/test-fixtures")
            .appendingPathComponent("Adversarial-" + UUID().uuidString)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
    }

    @discardableResult
    func write(_ name: String, bytes: Int = 12) throws -> URL {
        let url = root.appendingPathComponent(name)
        try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        try Data(repeating: 65, count: bytes).write(to: url)
        return url
    }

    @discardableResult
    func app(_ name: String, identifier: String?) throws -> URL {
        let bundle = root.appendingPathComponent(name)
        let contents = bundle.appendingPathComponent("Contents")
        try FileManager.default.createDirectory(at: contents.appendingPathComponent("MacOS"), withIntermediateDirectories: true)
        try Data(repeating: 65, count: 20).write(to: contents.appendingPathComponent("MacOS/Main"))
        if let identifier {
            let metadata: [String: String] = ["CFBundleIdentifier": identifier, "CFBundleShortVersionString": "1.0"]
            try PropertyListSerialization.data(fromPropertyList: metadata, format: .xml, options: 0)
                .write(to: contents.appendingPathComponent("Info.plist"))
        }
        return bundle
    }

    func clean() { try? FileManager.default.removeItem(at: root) }
}

private let completeProcesses: @Sendable () async -> RunningApplications = {
    RunningApplications(paths: [], bundleIdentifiers: [], inspectionComplete: true)
}

@Test func libraryScopeRuleIgnoresLetterCase() {
    let home = "/Users/SyntheticLindoUser"
    // The default macOS volume is case-insensitive: these name the real Library.
    for path in [home + "/library", home + "/library/Application Support", home + "/LIBRARY/Containers/data",
                 home + "/Library/application support", "/library/Caches", "/SYSTEM/Library", "/Private/var"] {
        #expect(!MacScanner.allowsLargeFilePath(path, home: home), "Scope accepted \(path)")
    }
    for path in [home + "/Library/CloudStorage/Provider", home + "/library/cloudstorage/Provider", home + "/Documents/Reports"] {
        #expect(MacScanner.allowsLargeFilePath(path, home: home), "Scope rejected \(path)")
    }
}

@Test func folderChooserRejectsLibraryInAnyLetterCase() async throws {
    let f = try AdversarialFixture(); defer { f.clean() }
    try f.write("Library/Application Support/Data/file.bin")
    try f.write("Documents/Reports/file.bin")
    let reader = FolderDirectoryReader(home: f.root, executor: FolderReadExecutor())
    _ = try await reader.list(f.root.appendingPathComponent("Documents/Reports"))
    for path in ["Library/Application Support", "library/Application Support", "LIBRARY/Application Support/Data"] {
        await #expect(throws: (any Error).self, "Chooser accepted \(path)") {
            _ = try await reader.list(f.root.appendingPathComponent(path))
        }
    }
}

@Test func systemLocationsAreProtectedInAnyLetterCase() {
    for path in ["/Library/Apple/System", "/library/apple/x", "/Applications/Utilities", "/Applications/Utilities/Terminal.app",
                 "/applications/utilities/Terminal.app", "/APPLICATIONS/UTILITIES"] {
        #expect(ScanPathPolicy.isProtected(URL(fileURLWithPath: path)), "Unprotected: \(path)")
    }
    #expect(!ScanPathPolicy.isProtected(URL(fileURLWithPath: "/Applications/Example.app")))
}

@Test func productionScopeRejectsArbitraryApplicationAndLogRoots() async throws {
    let f = try AdversarialFixture(); defer { f.clean() }
    try f.app("Example.app", identifier: "org.example.scope")
    try f.write("old.log")
    let scanner = MacScanner(fixtureScopes: false, trash: { _ in Issue.record("Unexpected trash") })
    for category in [ScanCategory.applications, .logs] {
        let report = await scanner.scan(category: category, roots: [f.root], minimumBytes: 0)
        #expect(report.items.isEmpty && report.inspectedCount == 0 && report.isPartial, "Production scope accepted \(category)")
    }
}

@Test func applicationWithoutReadableIdentityIsNeverACandidate() async throws {
    let f = try AdversarialFixture(); defer { f.clean() }
    try f.app("Known.app", identifier: "org.example.known")
    try f.app("Unknown.app", identifier: nil)
    let empty = try f.app("EmptyIdentity.app", identifier: "")
    #expect(FileManager.default.fileExists(atPath: empty.path))
    let scanner = MacScanner(trash: { _ in Issue.record("Unexpected trash") })
    let report = await scanner.scan(category: .applications, roots: [f.root])
    #expect(report.items.map(\.title) == ["Known"])
    #expect(report.warnings.contains { $0.contains("Unknown.app") } && report.warnings.contains { $0.contains("EmptyIdentity.app") })
    #expect(report.isPartial)
}

@Test func runningApplicationIsRecognizedByBundleIdentifierAlone() async throws {
    let f = try AdversarialFixture(); defer { f.clean() }
    let app = try f.app("Renamed Copy.app", identifier: "org.example.running")
    let recorder = AdversarialTrash()
    let scanner = MacScanner(trash: { recorder.add($0) })
    let report = await scanner.scan(category: .applications, roots: [f.root])
    #expect(report.items.count == 1)
    // The running instance lives elsewhere: only the identifier matches.
    let blocked = await scanner.moveToTrash(ids: report.items.map(\.id), liveRunningApplications: {
        RunningApplications(paths: ["/Synthetic/Elsewhere.app"], bundleIdentifiers: ["org.example.running"], inspectionComplete: true)
    })
    #expect(blocked.trashedIDs.isEmpty && blocked.failures.count == 1 && recorder.urls.isEmpty)
    let allowed = await scanner.moveToTrash(ids: report.items.map(\.id), liveRunningApplications: completeProcesses)
    #expect(allowed.trashedIDs.count == 1 && recorder.urls.map(\.path) == [app.path])
}

@Test func itemInsideFolderNamedAfterRunningBundleIsBlocked() async throws {
    let f = try AdversarialFixture(); defer { f.clean() }
    let log = try f.write("org.example.busy/old.log")
    let recorder = AdversarialTrash()
    let scanner = MacScanner(trash: { recorder.add($0) })
    let report = await scanner.scan(category: .logs, roots: [f.root])
    #expect(report.items.count == 1)
    let early = await scanner.moveToTrash(ids: report.items.map(\.id), runningBundleIdentifiers: ["org.example.busy"])
    #expect(early.trashedIDs.isEmpty && recorder.urls.isEmpty)
    let late = await scanner.moveToTrash(ids: report.items.map(\.id), liveRunningApplications: {
        RunningApplications(paths: [], bundleIdentifiers: ["org.example.busy"], inspectionComplete: true)
    })
    #expect(late.trashedIDs.isEmpty && recorder.urls.isEmpty)
    let idle = await scanner.moveToTrash(ids: report.items.map(\.id))
    #expect(idle.trashedIDs.count == 1 && recorder.urls.map(\.path) == [log.path])
}

@Test func scannerNeverTrashesTheApplicationItRunsFrom() async throws {
    let f = try AdversarialFixture(); defer { f.clean() }
    let own = try f.app("Host.app", identifier: "org.example.host")
    let other = try f.app("Other.app", identifier: "org.example.other")
    let recorder = AdversarialTrash()
    let scanner = MacScanner(ownBundlePath: own.path, trash: { recorder.add($0) })
    let report = await scanner.scan(category: .applications, roots: [f.root])
    #expect(report.items.count == 2)
    let result = await scanner.moveToTrash(ids: report.items.map(\.id), liveRunningApplications: completeProcesses)
    #expect(result.trashedIDs.count == 1 && result.failures.count == 1)
    #expect(recorder.urls.map(\.path) == [other.path])
}

@Test func unregisteredDocumentPackagesAreNotOpenedByCleanup() async throws {
    let f = try AdversarialFixture(); defer { f.clean() }
    try f.write("Windows.vmwarevm/disk-s001.vmdk", bytes: 64)
    try f.write("Linux.utm/Data/disk.qcow2", bytes: 64)
    try f.write("Edit.fcpbundle/Original Media/clip.mov", bytes: 64)
    try f.write("Build.xcarchive/Products/app.bin", bytes: 64)
    let plain = try f.write("Plain Folder/movie.mov", bytes: 64)
    let scanner = MacScanner(trash: { _ in Issue.record("Unexpected trash") })
    let report = await scanner.scan(category: .largeFiles, roots: [f.root], minimumBytes: 1)
    #expect(report.items.map(\.url.path) == [plain.path])
    #expect(report.warnings.contains { $0.contains("Pacchetti esclusi: 4") })
}

@Test func flatFilesWithPackageExtensionsAreOrdinaryFiles() async throws {
    let f = try AdversarialFixture(); defer { f.clean() }
    let installer = try f.write("Installer.pkg", bytes: 64)
    let slides = try f.write("Slides.key", bytes: 64)
    let recorder = AdversarialTrash()
    let scanner = MacScanner(trash: { recorder.add($0) })
    let report = await scanner.scan(category: .largeFiles, roots: [f.root], minimumBytes: 1)
    #expect(Set(report.items.map(\.url.path)) == [installer.path, slides.path])
    #expect(report.warnings.isEmpty && !report.isPartial)
    let result = await scanner.moveToTrash(ids: report.items.map(\.id))
    #expect(result.trashedIDs.count == 2 && result.failures.isEmpty && recorder.urls.count == 2)
}

@Test func oneFolderNamedTwiceIsScannedOnce() async throws {
    let f = try AdversarialFixture(); defer { f.clean() }
    let file = try f.write("Reports/summary.bin", bytes: 64)
    let root = f.root.appendingPathComponent("Reports")
    // On the default case-insensitive volume both spellings open the same folder.
    let respelled = f.root.appendingPathComponent("REPORTS")
    let scanner = MacScanner(trash: { _ in Issue.record("Unexpected trash") })
    let report = await scanner.scan(category: .largeFiles, roots: [root, respelled], minimumBytes: 1)
    #expect(report.items.map(\.url.path) == [file.path])
    // A single root in another spelling must not come back silently empty.
    let alone = await scanner.scan(category: .largeFiles, roots: [respelled], minimumBytes: 1)
    #expect(alone.items.map(\.url.path) == [file.path] && alone.warnings.isEmpty)
    let duplicates = await DuplicateScanner(trash: { _ in Issue.record("Unexpected trash") }).scan(roots: [respelled])
    #expect(duplicates.inspectedCount == 1 && !duplicates.isPartial)
}

@Test func retainedCopyTouchedBySyncServiceIsNotAFalseAlarm() async throws {
    let f = try AdversarialFixture(); defer { f.clean() }
    let keeperURL = try f.write("a-keeper.bin", bytes: 40)
    try f.write("b-copy.bin", bytes: 40)
    let recorder = AdversarialTrash()
    // A sync client rewrites an extended attribute on the retained copy during the move.
    let scanner = DuplicateScanner(trash: { url in
        recorder.add(url)
        _ = "synced".withCString { setxattr(keeperURL.path, "org.example.sync", $0, 6, 0, XATTR_NOFOLLOW) }
    })
    let report = await scanner.scan(roots: [f.root])
    let group = try #require(report.groups.first)
    let extras = group.files.filter { $0.id != group.keeperID }.map(\.id)
    let result = await scanner.moveToTrash(ids: extras, keepers: [group.id: group.keeperID])
    #expect(result.trashedIDs.count == 1 && result.failures.isEmpty && recorder.urls.count == 1)
}

@Test func symbolicLinksInsideVerifiedFoldersStayExcluded() async throws {
    let f = try AdversarialFixture(); defer { f.clean() }
    let outside = try AdversarialFixture(); defer { outside.clean() }
    try outside.write("target/secret.bin", bytes: 64)
    let real = try f.write("folder/real.bin", bytes: 64)
    try FileManager.default.createSymbolicLink(at: f.root.appendingPathComponent("folder/linked-folder"), withDestinationURL: outside.root.appendingPathComponent("target"))
    try FileManager.default.createSymbolicLink(at: f.root.appendingPathComponent("folder/linked-file.bin"), withDestinationURL: outside.root.appendingPathComponent("target/secret.bin"))
    let scanner = MacScanner(trash: { _ in Issue.record("Unexpected trash") })
    let report = await scanner.scan(category: .largeFiles, roots: [f.root], minimumBytes: 1)
    #expect(report.items.map(\.url.path) == [real.path])
    #expect(report.warnings.count == 2 && report.isPartial)
}

@Test func duplicateBatchStopsWhenTheRetainedCopyVanishesDuringTrash() async throws {
    let f = try AdversarialFixture(); defer { f.clean() }
    let keeperURL = try f.write("a-keeper.bin", bytes: 40)
    try f.write("b-copy.bin", bytes: 40)
    try f.write("c-copy.bin", bytes: 40)
    let recorder = AdversarialTrash()
    // An outside actor removes the retained copy exactly while the first extra copy is trashed.
    let scanner = DuplicateScanner(trash: { url in
        recorder.add(url)
        try? FileManager.default.removeItem(at: keeperURL)
    })
    let report = await scanner.scan(roots: [f.root])
    let group = try #require(report.groups.first)
    #expect(group.files.count == 3 && group.files.first { $0.id == group.keeperID }?.url.path == keeperURL.path)
    let extras = group.files.filter { $0.id != group.keeperID }.map(\.id)
    let result = await scanner.moveToTrash(ids: extras, keepers: [group.id: group.keeperID])
    #expect(recorder.urls.count == 1, "A second copy was trashed after the retained copy vanished")
    #expect(result.trashedIDs.count == 1)
    #expect(result.failures.contains { $0.contains("copia da conservare") && $0.contains("Cestino") })
}

@Test func processInventorySeesTheCurrentProcess() {
    var buffer = [CChar](repeating: 0, count: 4 * Int(MAXPATHLEN))
    let length = buffer.withUnsafeMutableBytes { proc_pidpath(getpid(), $0.baseAddress, UInt32($0.count)) }
    #expect(length > 0)
    let own = String(decoding: buffer.prefix { $0 != 0 }.map { UInt8(bitPattern: $0) }, as: UTF8.self)
    let snapshot = ProcessInventory.capture()
    #expect(snapshot.isComplete)
    #expect(snapshot.paths.contains(own), "Native inventory missed the running test process")
}

@Test func publicScannersUseProductionScope() async throws {
    let f = try AdversarialFixture(); defer { f.clean() }
    try f.write("a.bin", bytes: 64); try f.write("b.bin", bytes: 64)
    // The fixture lives under ~/Library: only test-only scopes may read it.
    let report = await MacScanner().scan(category: .largeFiles, roots: [f.root], minimumBytes: 1)
    #expect(report.items.isEmpty && report.inspectedCount == 0 && report.isPartial)
    let duplicates = await DuplicateScanner().scan(roots: [f.root])
    #expect(duplicates.groups.isEmpty && duplicates.inspectedCount == 0 && duplicates.isPartial)
}
