import Foundation
import Testing
@testable import LindoKit

private func reportFixture() throws -> URL {
    let root = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Developer/Lindo/test-fixtures/report-" + UUID().uuidString)
    try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
    return root
}

@Test func applicationSizeCountsHardlinkedPayloadOnce() async throws {
    let root = try reportFixture()
    defer { try? FileManager.default.removeItem(at: root) }
    let app = root.appendingPathComponent("Linked.app/Contents")
    try FileManager.default.createDirectory(at: app, withIntermediateDirectories: true)
    let plist = try PropertyListSerialization.data(fromPropertyList: ["CFBundleIdentifier": "org.example.linked"], format: .xml, options: 0)
    try plist.write(to: app.appendingPathComponent("Info.plist"))
    let payload = app.appendingPathComponent("payload")
    try Data(repeating: 3, count: 1024).write(to: payload)
    try FileManager.default.linkItem(at: payload, to: app.appendingPathComponent("alias"))
    let scanner = MacScanner(trash: { _ in Issue.record("Read-only test") })
    let report = await scanner.scan(category: .applications, roots: [root])
    #expect(report.items.count == 1)
    #expect(report.items.first?.bytes == Int64(plist.count + 1024))
}

@Test func cacheSizeCountsHardlinkedPayloadOnce() async throws {
    let root = try reportFixture()
    defer { try? FileManager.default.removeItem(at: root) }
    let cache = root.appendingPathComponent("Google/Chrome/Default/Cache")
    try FileManager.default.createDirectory(at: cache, withIntermediateDirectories: true)
    let payload = cache.appendingPathComponent("payload")
    try Data(repeating: 3, count: 1024).write(to: payload)
    try FileManager.default.linkItem(at: payload, to: cache.appendingPathComponent("alias"))
    let scanner = MacScanner(cacheRoot: root, trash: { _ in Issue.record("Read-only test") })
    let report = await scanner.scan(category: .caches, roots: [root])
    #expect(report.items.count == 1)
    #expect(report.items.first?.bytes == 1024)
}

@Test func preliminaryExclusionsReportPartialCoverage() async throws {
    let root = try reportFixture()
    defer { try? FileManager.default.removeItem(at: root) }
    let file = root.appendingPathComponent("local.bin")
    try Data([1]).write(to: file)
    try Data([2]).write(to: root.appendingPathComponent("remote.icloud"))
    try FileManager.default.createSymbolicLink(at: root.appendingPathComponent("alias"), withDestinationURL: file)
    let scanner = MacScanner(trash: { _ in Issue.record("Read-only test") })
    let report = await scanner.scan(category: .largeFiles, roots: [root], minimumBytes: 1)
    #expect(report.items.map(\.url) == [file])
    #expect(report.isPartial)
    #expect(report.warnings.contains { $0.contains("alias") })
    #expect(report.warnings.contains { $0.contains("remote.icloud") })
}

@Test func hiddenExclusionsHaveBoundedCoverageWarningsWithoutDescent() async throws {
    for category in [ScanCategory.largeFiles, .logs] {
        let root = try reportFixture()
        defer { try? FileManager.default.removeItem(at: root) }
        for index in 0..<30 {
            let directory = root.appendingPathComponent(".hidden-\(index)")
            try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
            try Data([1]).write(to: directory.appendingPathComponent("unvisited.bin"))
        }
        try Data([1]).write(to: root.appendingPathComponent(".hidden-file"))
        let scanner = MacScanner(trash: { _ in Issue.record("Read-only test") })
        let report = await scanner.scan(category: category, roots: [root], minimumBytes: 0)
        #expect(report.items.isEmpty && report.isPartial)
        #expect(report.inspectedCount == 31)
        #expect(report.warnings.count == 1)
        #expect(report.warnings.first?.contains("nascosti esclusi: 31") == true)
    }
}

@Test func packageExclusionsHaveBoundedCoverageWarningsWithoutDescent() async throws {
    for category in [ScanCategory.largeFiles, .logs] {
        let root = try reportFixture()
        defer { try? FileManager.default.removeItem(at: root) }
        for index in 0..<30 {
            let directory = root.appendingPathComponent("Document-\(index).pages")
            try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
            try Data([1]).write(to: directory.appendingPathComponent("unvisited.bin"))
        }
        let scanner = MacScanner(trash: { _ in Issue.record("Read-only test") })
        let report = await scanner.scan(category: category, roots: [root], minimumBytes: 0)
        #expect(report.items.isEmpty && report.isPartial)
        #expect(report.inspectedCount == 30)
        #expect(report.warnings.count == 1)
        #expect(report.warnings.first?.contains("Pacchetti esclusi: 30") == true)
    }
}

@Test func sizeAndAgeFiltersDoNotClaimPartialCoverage() async throws {
    let root = try reportFixture()
    defer { try? FileManager.default.removeItem(at: root) }
    try Data([1]).write(to: root.appendingPathComponent("small-new.bin"))
    let scanner = MacScanner(trash: { _ in Issue.record("Read-only test") })
    let large = await scanner.scan(category: .largeFiles, roots: [root], minimumBytes: 100)
    let logs = await scanner.scan(category: .logs, roots: [root], olderThan: .distantPast)
    #expect(large.items.isEmpty && !large.isPartial && large.warnings.isEmpty)
    #expect(logs.items.isEmpty && !logs.isPartial && logs.warnings.isEmpty)
}
