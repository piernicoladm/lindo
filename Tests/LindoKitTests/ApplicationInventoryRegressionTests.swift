import Foundation
import Testing
@testable import LindoKit

private struct ApplicationInventoryFixture {
    let root: URL

    init() throws {
        root = FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent("Library/Developer/Lindo/test-fixtures")
            .appendingPathComponent("ApplicationInventory-" + UUID().uuidString)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
    }

    func app(_ relativePath: String, identifier: String = "org.example.inventory", payloadFiles: Int = 1, payloadDirectory: String = "Resources") throws -> URL {
        let bundle = root.appendingPathComponent(relativePath)
        let contents = bundle.appendingPathComponent("Contents")
        let payload = contents.appendingPathComponent(payloadDirectory)
        try FileManager.default.createDirectory(at: payload, withIntermediateDirectories: true)
        let metadata: [String: String] = ["CFBundleIdentifier": identifier, "CFBundleShortVersionString": "1.0"]
        let data = try PropertyListSerialization.data(fromPropertyList: metadata, format: .xml, options: 0)
        try data.write(to: contents.appendingPathComponent("Info.plist"))
        for index in 0..<payloadFiles {
            try Data(repeating: 65, count: 12).write(to: payload.appendingPathComponent("payload-\(index).bin"))
        }
        return bundle
    }

    func clean() { try? FileManager.default.removeItem(at: root) }
}

@Test func applicationInventoryGivesEachBundleItsOwnVerificationBudget() async throws {
    let fixture = try ApplicationInventoryFixture(); defer { fixture.clean() }
    // Each bundle has 14 entries, below 20; both bundles together exceed 20.
    let first = try fixture.app("First.app", payloadFiles: 10)
    let second = try fixture.app("Second.app", payloadFiles: 10)
    let scanner = MacScanner(entryLimit: 20, trash: { _ in Issue.record("Inventory must not trash files") })

    let report = await scanner.scan(category: .applications, roots: [fixture.root])

    #expect(Set(report.items.map { $0.url.path }) == Set([first.path, second.path]))
    #expect(report.warnings.isEmpty)
    #expect(!report.isPartial)
}

@Test func applicationInventorySkipsLargeAppleBundleBeforeVerificationBudget() async throws {
    let fixture = try ApplicationInventoryFixture(); defer { fixture.clean() }
    let appleRoot = fixture.root.appendingPathComponent("a")
    let thirdPartyRoot = fixture.root.appendingPathComponent("third-party")
    // Root ordering ensures the excluded Apple application is encountered first.
    _ = try fixture.app("a/Apple.app", identifier: "com.apple.inventoryfixture", payloadFiles: 50)
    let thirdParty = try fixture.app("third-party/Useful.app", payloadFiles: 1)
    let scanner = MacScanner(entryLimit: 20, trash: { _ in Issue.record("Inventory must not trash files") })

    let report = await scanner.scan(category: .applications, roots: [thirdPartyRoot, appleRoot])

    #expect(report.items.map { $0.url.path } == [thirdParty.path])
    #expect(report.warnings.isEmpty)
    #expect(!report.isPartial)
}

@Test func applicationInventoryFindsBundlesInsideOrdinarySubfolders() async throws {
    let fixture = try ApplicationInventoryFixture(); defer { fixture.clean() }
    let nested = try fixture.app("Utilities/Vendor/Nested.app")
    let scanner = MacScanner(entryLimit: 32, trash: { _ in Issue.record("Inventory must not trash files") })

    let report = await scanner.scan(category: .applications, roots: [fixture.root])

    #expect(report.items.map { $0.url.path } == [nested.path])
    #expect(report.warnings.isEmpty)
}

@Test func applicationInventoryAcceptsOrdinaryResourceNamesInsideBundles() async throws {
    let fixture = try ApplicationInventoryFixture(); defer { fixture.clean() }
    var bundles: Set<URL> = []
    for (index, name) in ["Photos", "Mail", "Messages", "System", "Apple", "com.apple.resources"].enumerated() {
        bundles.insert(try fixture.app("Resources\(index).app", payloadDirectory: name))
    }
    let scanner = MacScanner(entryLimit: 64, trash: { _ in Issue.record("Inventory must not trash files") })

    let report = await scanner.scan(category: .applications, roots: [fixture.root])

    #expect(Set(report.items.map { $0.url.path }) == Set(bundles.map(\.path)))
    #expect(report.warnings.isEmpty)
    #expect(!report.isPartial)
}

@Test func applicationInventoryExcludesSmallAppleAndLindoBundles() async throws {
    let fixture = try ApplicationInventoryFixture(); defer { fixture.clean() }
    _ = try fixture.app("AppleFixture.app", identifier: "com.apple.inventoryfixture")
    _ = try fixture.app("LindoFixture.app", identifier: "com.piernicolademaria.lindo")
    let wanted = try fixture.app("Useful.app")
    let scanner = MacScanner(entryLimit: 20, trash: { _ in Issue.record("Inventory must not trash files") })

    let report = await scanner.scan(category: .applications, roots: [fixture.root])

    #expect(report.items.map { $0.url.path } == [wanted.path])
    #expect(report.warnings.isEmpty)
}

@Test func applicationInventoryContinuesAfterOversizedThirdPartyBundle() async throws {
    let fixture = try ApplicationInventoryFixture(); defer { fixture.clean() }
    _ = try fixture.app("a/Oversized.app", payloadFiles: 50)
    let wanted = try fixture.app("later/Useful.app")
    let scanner = MacScanner(entryLimit: 20, trash: { _ in Issue.record("Inventory must not trash files") })

    let report = await scanner.scan(category: .applications, roots: [fixture.root.appendingPathComponent("a"), fixture.root.appendingPathComponent("later")])

    #expect(report.items.map { $0.url.path } == [wanted.path])
    #expect(report.isPartial)
    #expect(report.warnings.contains { $0.contains("Oversized.app") })
}

@Test func applicationInventoryReportsDirectoryDepthCoverageLimit() async throws {
    let fixture = try ApplicationInventoryFixture(); defer { fixture.clean() }
    _ = try fixture.app("one/two/three/four/TooDeep.app")
    let scanner = MacScanner(entryLimit: 32, trash: { _ in Issue.record("Inventory must not trash files") })

    let report = await scanner.scan(category: .applications, roots: [fixture.root])

    #expect(report.items.isEmpty)
    #expect(report.isPartial)
    #expect(report.warnings.contains { $0.contains("Profondità massima") })
}

@Test func applicationInventoryRecordsDanglingSymlinksWithoutFollowingTargets() async throws {
    let fixture = try ApplicationInventoryFixture(); defer { fixture.clean() }
    let bundle = try fixture.app("Linked.app")
    try FileManager.default.createSymbolicLink(atPath: bundle.appendingPathComponent("Contents/Resources/alias").path,
                                              withDestinationPath: "missing-target")
    let scanner = MacScanner(trash: { _ in })
    let report = await scanner.scan(category: .applications, roots: [fixture.root])
    #expect(report.items.count == 1)
    #expect(!report.isPartial)
    let item = try #require(report.items.first)
    let result = await scanner.moveToTrash(ids: [item.id], liveRunningApplications: {
        RunningApplications(paths: [], bundleIdentifiers: [], inspectionComplete: true)
    })
    #expect(result.trashedIDs == [item.id])
}

@Test func applicationResourceExceptionDoesNotAllowUserMessagesOrCredentials() async throws {
    let fixture = try ApplicationInventoryFixture(); defer { fixture.clean() }
    _ = try fixture.app("Messages/Blocked.app")
    _ = try fixture.app("CredentialFixture.app", payloadDirectory: ".ssh")
    let allowed = try fixture.app("Good.app", payloadDirectory: "Resources/plugin/Messages")
    let scanner = MacScanner(trash: { _ in })
    let report = await scanner.scan(category: .applications, roots: [fixture.root])
    #expect(report.items.map { $0.url.path } == [allowed.path])
    #expect(report.isPartial && report.warnings.count == 2)
    let result = await scanner.moveToTrash(ids: report.items.map(\.id), liveRunningApplications: {
        RunningApplications(paths: [], bundleIdentifiers: [], inspectionComplete: true)
    })
    #expect(result.trashedIDs.count == 1)
}
