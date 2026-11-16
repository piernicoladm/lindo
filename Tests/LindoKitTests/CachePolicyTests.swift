import Foundation
import Testing
@testable import LindoKit

private struct CacheFixture {
    let root: URL
    init() throws {
        root = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Developer/Lindo/test-fixtures/" + UUID().uuidString)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
    }
    func write(_ path: String) throws -> URL {
        let url = root.appendingPathComponent(path)
        try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        try Data([1, 2, 3]).write(to: url)
        return url
    }
    func clean() { try? FileManager.default.removeItem(at: root) }
}

@Test func cacheAllowlistIsExact() {
    let root = URL(fileURLWithPath: "/Synthetic/Caches")
    for path in ["Google/Chrome/Default/Cache", "Google/Chrome/Profile 12/Cache", "Chromium/Default/Cache", "Firefox/Profiles/random.work/cache2"] {
        #expect(CachePolicy.owner(of: root.appendingPathComponent(path), root: root) != nil)
    }
    for path in ["com.example/Cache", "Google/Chrome/Default/Service Worker/CacheStorage", "Google/Chrome/Profile nope/Cache", "Chromium/Default/Cache/Cache_Data", "Firefox/Profiles/.hidden/cache2"] {
        #expect(CachePolicy.owner(of: root.appendingPathComponent(path), root: root) == nil)
        #expect(!CachePolicy.mayTraverse(root.appendingPathComponent(path), root: root))
    }
}

@Test func cacheUnitsIgnoreBlockAgeAndUnknownTrees() async throws {
    let f = try CacheFixture(); defer { f.clean() }
    _ = try f.write("Google/Chrome/Default/Cache/Cache_Data/index")
    _ = try f.write("Google/Chrome/Default/Cache/Cache_Data/block")
    _ = try f.write("unknown/deep/data")
    let scanner = MacScanner(cacheRoot: f.root, trash: { _ in })
    let report = await scanner.scan(category: .caches, roots: [f.root], olderThan: .distantPast)
    #expect(report.items.count == 1)
    #expect(report.items.first?.bytes == 6)
    #expect(report.items.first?.ownerBundleIdentifier == "com.google.Chrome")
    #expect(report.items.first?.detail == "Intera cache HTTP. Chiudi il browser prima di pulire")
    let deleted = await scanner.moveToTrash(ids: report.items.map(\.id), liveRunningApplications: { RunningApplications(paths: [], bundleIdentifiers: []) })
    #expect(deleted.trashedIDs.count == 1)
}

@Test func cacheRejectsUnsafeTreeAndChangesDuringProvider() async throws {
    let f = try CacheFixture(); defer { f.clean() }
    let file = try f.write("Chromium/Default/Cache/index")
    let scanner = MacScanner(cacheRoot: f.root, trash: { _ in Issue.record("Modified cache trashed") })
    let report = await scanner.scan(category: .caches, roots: [f.root])
    let deleted = await scanner.moveToTrash(ids: report.items.map(\.id), liveRunningApplications: {
        _ = try? f.write("Chromium/Default/Cache/new-entry")
        return RunningApplications(paths: [], bundleIdentifiers: [])
    })
    #expect(deleted.trashedIDs.isEmpty && deleted.failures.count == 1)
    try FileManager.default.createSymbolicLink(at: file.deletingLastPathComponent().appendingPathComponent("link"), withDestinationURL: file)
    let unsafe = await scanner.scan(category: .caches, roots: [f.root])
    #expect(unsafe.items.isEmpty && unsafe.isPartial)
}

@Test func cacheBlocksHelpersAndIncompleteInventory() async throws {
    let f = try CacheFixture(); defer { f.clean() }
    _ = try f.write("Firefox/Profiles/example.work/cache2/entries/file")
    let scanner = MacScanner(cacheRoot: f.root, trash: { _ in Issue.record("Active cache trashed") })
    for live in [RunningApplications(paths: [], bundleIdentifiers: ["org.mozilla.firefox.helper"]), RunningApplications(paths: ["/Applications/Firefox.app/Contents/MacOS/plugin-container"], bundleIdentifiers: []), RunningApplications(paths: [], bundleIdentifiers: [], inspectionComplete: false)] {
        let report = await scanner.scan(category: .caches, roots: [f.root])
        let deleted = await scanner.moveToTrash(ids: report.items.map(\.id), liveRunningApplications: { live })
        #expect(deleted.trashedIDs.isEmpty && deleted.failures.first?.contains("Chiudi") == true)
    }
}

@Test func cacheNeedsFreshProviderAndRejectsPackages() async throws {
    let f = try CacheFixture(); defer { f.clean() }
    _ = try f.write("Google/Chrome/Profile 1/Cache/index")
    let scanner = MacScanner(cacheRoot: f.root, trash: { _ in Issue.record("Unverified cache trashed") })
    let report = await scanner.scan(category: .caches, roots: [f.root])
    let deleted = await scanner.moveToTrash(ids: report.items.map(\.id))
    #expect(deleted.trashedIDs.isEmpty && deleted.failures.count == 1)
    _ = try f.write("Google/Chrome/Profile 1/Cache/payload.app/Contents/file")
    let package = await scanner.scan(category: .caches, roots: [f.root])
    #expect(package.items.isEmpty && package.isPartial)
}

@Test func finalValidationCanVetoImmediatelyBeforeTrash() async throws {
    let f = try CacheFixture(); defer { f.clean() }
    _ = try f.write("file.bin")
    let scanner = MacScanner(trash: { _ in Issue.record("Final veto ignored") })
    let report = await scanner.scan(category: .largeFiles, roots: [f.root], minimumBytes: 0)
    let deleted = await scanner.moveToTrash(ids: report.items.map(\.id), finalValidation: { false })
    #expect(deleted.trashedIDs.isEmpty && deleted.failures.count == 1)
}

@Test func cacheFinalInventoryRunsAfterTreeRevalidation() async throws {
    let f = try CacheFixture(); defer { f.clean() }
    _ = try f.write("Google/Chrome/Default/Cache/index")
    let scanner = MacScanner(cacheRoot: f.root, processInventory: {
        ProcessSnapshot(paths: ["/Applications/Google Chrome.app/Contents/Frameworks/Google Chrome Framework.framework/Helpers/Google Chrome Helper.app/Contents/MacOS/Google Chrome Helper"], isComplete: true)
    }, trash: { _ in Issue.record("Browser appeared after early inventory") })
    let report = await scanner.scan(category: .caches, roots: [f.root])
    let deleted = await scanner.moveToTrash(ids: report.items.map(\.id), liveRunningApplications: {
        RunningApplications(paths: [], bundleIdentifiers: [], inspectionComplete: true)
    })
    #expect(deleted.trashedIDs.isEmpty && deleted.failures.first?.contains("finale") == true)
}

@Test func cacheFinalIncompleteInventoryFailsClosed() async throws {
    let f = try CacheFixture(); defer { f.clean() }
    _ = try f.write("Chromium/Default/Cache/index")
    let scanner = MacScanner(cacheRoot: f.root, processInventory: {
        ProcessSnapshot(paths: [], isComplete: false)
    }, trash: { _ in Issue.record("Incomplete final inventory ignored") })
    let report = await scanner.scan(category: .caches, roots: [f.root])
    let deleted = await scanner.moveToTrash(ids: report.items.map(\.id), liveRunningApplications: {
        RunningApplications(paths: [], bundleIdentifiers: [])
    })
    #expect(deleted.trashedIDs.isEmpty && deleted.failures.count == 1)
}

@Test func firefoxCacheBlocksRelatedChannelsAndRenamedBundles() throws {
    let root = URL(fileURLWithPath: "/Synthetic/Caches")
    let owner = try #require(CachePolicy.owner(of: root.appendingPathComponent("Firefox/Profiles/example.work/cache2"), root: root))
    for identifier in ["org.mozilla.firefoxdeveloperedition", "org.mozilla.nightly", "org.mozilla.nightly.helper", "org.mozilla.plugincontainer", "org.mozilla.firefox-developer-edition-gpu-helper"] {
        #expect(CachePolicy.isRunning(owner, paths: [], identifiers: [identifier]))
    }
    for path in [
        "/Applications/Firefox Developer Edition.app/Contents/MacOS/firefox",
        "/Applications/Firefox Nightly.app/Contents/MacOS/firefox",
        "/Applications/Nightly.app/Contents/MacOS/firefox",
        "/Synthetic/Renamed.app/Contents/MacOS/firefox",
        "/Synthetic/Renamed.app/Contents/MacOS/plugin-container",
        "/Synthetic/Renamed.app/Contents/Frameworks/plugin-container.app/Contents/MacOS/plugin-container",
        "/Synthetic/Renamed.app/Contents/MacOS/gpu-helper",
        "/Synthetic/Renamed.app/Contents/MacOS/media-plugin-helper",
        "/Synthetic/Renamed.app/Contents/MacOS/security-module-helper"
    ] {
        #expect(CachePolicy.isRunning(owner, paths: [path], identifiers: []))
    }
    for identifier in ["org.mozilla.nightlynotes", "org.mozilla.firefoxdevelopereditionnotes", "org.mozilla.plugincontainernotes"] {
        #expect(!CachePolicy.isRunning(owner, paths: [], identifiers: [identifier]))
    }
    for path in [
        "/Synthetic/Contents/MacOS/firefox",
        "/Synthetic/Renamed.app/MacOS/firefox",
        "/Synthetic/Renamed.app/Contents/Resources/firefox",
        "/Synthetic/Renamed.app/Contents/MacOS/firefox-notes",
        "/Synthetic/Renamed.app/Contents/MacOS/plugin-container-notes",
        "/Synthetic/Renamed.app.backup/Contents/MacOS/firefox",
        "Renamed.app/Contents/MacOS/firefox"
    ] {
        #expect(!CachePolicy.isRunning(owner, paths: [path], identifiers: []))
    }
    let chrome = try #require(CachePolicy.owner(of: root.appendingPathComponent("Google/Chrome/Default/Cache"), root: root))
    #expect(!CachePolicy.isRunning(chrome, paths: ["/Synthetic/Renamed.app/Contents/MacOS/firefox"], identifiers: ["org.mozilla.nightly"]))
}

@Test func firefoxNightlyInFinalInventoryVetoesTrash() async throws {
    let f = try CacheFixture(); defer { f.clean() }
    _ = try f.write("Firefox/Profiles/example.work/cache2/entries/file")
    let scanner = MacScanner(cacheRoot: f.root, processInventory: {
        ProcessSnapshot(paths: ["/Synthetic/Firefox Nightly.app/Contents/MacOS/firefox"], isComplete: true)
    }, trash: { _ in Issue.record("Nightly cache trashed") })
    let report = await scanner.scan(category: .caches, roots: [f.root])
    #expect(report.items.count == 1)
    let deleted = await scanner.moveToTrash(ids: report.items.map(\.id), liveRunningApplications: {
        RunningApplications(paths: [], bundleIdentifiers: [], inspectionComplete: true)
    })
    #expect(deleted.trashedIDs.isEmpty && deleted.failures.first?.contains("finale") == true)
}

@Test func queuedApplicationChildrenRejectReplacedSymlinkAncestor() throws {
    let f = try CacheFixture(); defer { f.clean() }
    let child = try f.write("Synthetic.app/Contents/Frameworks/Nested.framework/Versions/A/file")
    let root = f.root.appendingPathComponent("Synthetic.app")
    #expect(MacScanner.applicationAncestorsAreDirectories(child, root: root))
    let directory = root.appendingPathComponent("Contents/Frameworks/Nested.framework/Versions")
    let moved = root.appendingPathComponent("Contents/Frameworks/Nested.framework/OldVersions")
    try FileManager.default.moveItem(at: directory, to: moved)
    try FileManager.default.createSymbolicLink(at: directory, withDestinationURL: moved)
    #expect(!MacScanner.applicationAncestorsAreDirectories(child, root: root))
}
