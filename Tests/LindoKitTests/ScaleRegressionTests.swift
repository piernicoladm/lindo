import Foundation
import Testing
@testable import LindoKit

private struct ScaleFixture {
    let root: URL

    init() throws {
        root = FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent("Library/Developer/Lindo/test-fixtures")
            .appendingPathComponent("ScaleRegression-" + UUID().uuidString)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
    }

    func clean() { try? FileManager.default.removeItem(at: root) }
}

@Test(.enabled(if: ProcessInfo.processInfo.environment["LINDO_SCALE_TEST"] == "1"))
func scaleInventoryCompletesMoreThanOneHundredThousandEntries() async throws {
    let fixture = try ScaleFixture(); defer { fixture.clean() }
    let directoryCount = 120
    let filesPerDirectory = 1_000
    let fileCount = directoryCount * filesPerDirectory
    let fixtureStarted = Date()
    let payload = Data([65])
    for directoryIndex in 0..<directoryCount {
        let directory = fixture.root.appendingPathComponent("batch-\(directoryIndex)")
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: false)
        for fileIndex in 0..<filesPerDirectory {
            // Independent files ensure inode deduplication cannot disguise missing entries.
            try payload.write(to: directory.appendingPathComponent("entry-\(fileIndex).bin"))
        }
    }
    let preparationDuration = Date().timeIntervalSince(fixtureStarted)
    // Use the same uncapped entry budget as production, with fixture scope only.
    let scanner = MacScanner(fixtureScopes: true, trash: { _ in
        Issue.record("Scale inventory must not trash files")
    })
    let started = Date()

    let report = await scanner.scan(category: .largeFiles, roots: [fixture.root], minimumBytes: 1)

    let elapsed = Date().timeIntervalSince(started)
    print("LINDO_SCALE inventory: fixture_seconds=\(preparationDuration), scan_seconds=\(elapsed), reported_seconds=\(report.duration), files=\(report.items.count), inspected=\(report.inspectedCount), logical_bytes=\(report.items.reduce(Int64(0)) { $0 + $1.bytes })")
    #expect(report.items.count == fileCount)
    #expect(Set(report.items.map { $0.url.path }).count == fileCount)
    #expect(report.inspectedCount == fileCount + directoryCount)
    #expect(report.items.allSatisfy { $0.bytes == 1 })
    #expect(report.warnings.isEmpty)
    #expect(!report.isPartial && !report.cancelled)
}

@Test(.enabled(if: ProcessInfo.processInfo.environment["LINDO_SCALE_TEST"] == "1"))
func scaleDuplicatesPreserveThousandsOfFilesAndDistinctContentGroups() async throws {
    let fixture = try ScaleFixture(); defer { fixture.clean() }
    let groupCount = 1_000
    let copiesPerGroup = 3
    let uniqueFileCount = 100
    let bytesPerFile = 4_096
    let fixtureStarted = Date()
    var expectedGroups: Set<Set<String>> = []
    var uniquePaths: Set<String> = []
    for identity in 0..<(groupCount + uniqueFileCount) {
        // Every file has the same size; the first eight bytes distinguish content
        // groups, exercising hashing/comparison rather than size-only grouping.
        var payload = Data(repeating: 65, count: bytesPerFile)
        for byteIndex in 0..<8 {
            payload[byteIndex] = UInt8((UInt64(identity) >> (byteIndex * 8)) & 0xff)
        }
        let copies = identity < groupCount ? copiesPerGroup : 1
        var paths: Set<String> = []
        for copyIndex in 0..<copies {
            let url = fixture.root.appendingPathComponent("content-\(identity)-copy-\(copyIndex).bin")
            try payload.write(to: url)
            paths.insert(url.path)
        }
        if identity < groupCount { expectedGroups.insert(paths) }
        else { uniquePaths.formUnion(paths) }
    }
    let preparationDuration = Date().timeIntervalSince(fixtureStarted)
    let scanner = DuplicateScanner(fixtureScopes: true, trash: { _ in
        Issue.record("Scale duplicate inventory must not trash files")
    })
    let started = Date()

    let report = await scanner.scan(roots: [fixture.root], minimumBytes: 1)

    let elapsed = Date().timeIntervalSince(started)
    let actualGroups = Set(report.groups.map { Set($0.files.map { $0.url.path }) })
    let groupedPaths = Set(report.groups.flatMap { $0.files.map { $0.url.path } })
    print("LINDO_SCALE duplicates: fixture_seconds=\(preparationDuration), scan_seconds=\(elapsed), reported_seconds=\(report.duration), groups=\(report.groups.count), grouped_files=\(groupedPaths.count), inspected=\(report.inspectedCount), logical_bytes=\((groupCount * copiesPerGroup + uniqueFileCount) * bytesPerFile)")
    #expect(report.groups.count == groupCount)
    #expect(actualGroups == expectedGroups)
    #expect(report.groups.allSatisfy { $0.files.count == copiesPerGroup })
    #expect(groupedPaths.count == groupCount * copiesPerGroup)
    #expect(groupedPaths.isDisjoint(with: uniquePaths))
    #expect(Set(report.groups.map(\.digest)).count == groupCount)
    #expect(report.inspectedCount == groupCount * copiesPerGroup + uniqueFileCount)
    #expect(report.warnings.isEmpty)
    #expect(!report.isPartial && !report.cancelled)
}
