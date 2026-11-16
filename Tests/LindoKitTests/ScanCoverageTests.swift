import Darwin
import Foundation
import Testing
@testable import LindoKit

private func coverageFixture() throws -> URL {
    let root = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Developer/Lindo/test-fixtures/coverage-" + UUID().uuidString)
    try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
    return root
}

private func coverageWrite(_ root: URL, _ path: String) throws {
    let url = root.appendingPathComponent(path)
    try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
    try Data([1, 2, 3, 4]).write(to: url)
}

@Test func plannedExclusionsHaveStructuredCoverageWithoutReadErrors() async throws {
    let root = try coverageFixture(); defer { try? FileManager.default.removeItem(at: root) }
    for name in ["visible.bin", ".DS_Store", "Document.pages/inside.bin", "remote.icloud"] { try coverageWrite(root, name) }
    let report = await MacScanner(trash: { _ in Issue.record("Read-only coverage test") }).scan(category: .largeFiles, roots: [root], minimumBytes: 1)
    #expect(report.items.map(\.title) == ["visible.bin"])
    #expect(report.coverage.excludedCount == 3 && !report.coverage.hasUnplannedOmissions)
    #expect(report.isPartial && report.warnings.count == 3)
}

@Test func deniedDirectoryCoverageRemainsIncomplete() async throws {
    let root = try coverageFixture(); defer { try? FileManager.default.removeItem(at: root) }
    try coverageWrite(root, "Denied/inside.bin")
    let denied = root.appendingPathComponent("Denied")
    #expect(chmod(denied.path, 0) == 0)
    defer { _ = chmod(denied.path, 0o700) }
    let report = await MacScanner(trash: { _ in Issue.record("Read-only coverage test") }).scan(category: .largeFiles, roots: [root], minimumBytes: 1)
    #expect(report.items.isEmpty && report.coverage.hasUnplannedOmissions)
    #expect(report.coverage.excludedCount == 0 && !report.warnings.isEmpty)
    #expect(report.inspectedCount == 1 && report.warnings.contains { $0.contains("Denied") })
}

@Test func mixedCoveragePreservesExclusionsAndAccessFailure() async throws {
    let root = try coverageFixture(); defer { try? FileManager.default.removeItem(at: root) }
    for name in ["visible.bin", ".DS_Store", "Denied/inside.bin"] { try coverageWrite(root, name) }
    let denied = root.appendingPathComponent("Denied")
    #expect(chmod(denied.path, 0) == 0)
    defer { _ = chmod(denied.path, 0o700) }
    let report = await MacScanner(trash: { _ in Issue.record("Read-only coverage test") }).scan(category: .largeFiles, roots: [root], minimumBytes: 1)
    #expect(report.items.map(\.title) == ["visible.bin"])
    #expect(report.coverage.excludedCount == 1 && report.coverage.hasUnplannedOmissions)
    #expect(report.warnings.count >= 2)
}

@Test func scanLimitsAreUnplannedOmissions() async throws {
    let root = try coverageFixture(); defer { try? FileManager.default.removeItem(at: root) }
    for name in ["a.bin", "b.bin"] { try coverageWrite(root, name) }
    let report = await MacScanner(entryLimit: 1, trash: { _ in Issue.record("Read-only coverage test") }).scan(category: .largeFiles, roots: [root], minimumBytes: 1)
    #expect(report.coverage.hasUnplannedOmissions && report.coverage.excludedCount == 0)
    #expect(report.isPartial && report.items.count == 1)
}

@Test func duplicateCoveragePropagatesPlannedExclusions() async throws {
    let root = try coverageFixture(); defer { try? FileManager.default.removeItem(at: root) }
    for name in ["first.bin", "second.bin", ".DS_Store"] { try coverageWrite(root, name) }
    let report = await DuplicateScanner(trash: { _ in Issue.record("Read-only coverage test") }).scan(roots: [root])
    #expect(report.groups.count == 1 && report.groups[0].files.count == 2)
    #expect(report.coverage.excludedCount == 1 && !report.coverage.hasUnplannedOmissions)
    #expect(report.isPartial && !report.warnings.isEmpty)
}

@Test func spaceCoverageDistinguishesPolicyFromDeniedChildren() async throws {
    let root = try coverageFixture(); defer { try? FileManager.default.removeItem(at: root) }
    for name in ["Folder/.ssh/key", "visible.bin"] { try coverageWrite(root, name) }
    let scanner = SpaceScanner(executor: ScanReadExecutor())
    let policy = await scanner.scan(root: root)
    #expect(policy.coverage.excludedCount == 1 && !policy.coverage.hasUnplannedOmissions)
    let folder = policy.entries.first { $0.url.lastPathComponent == "Folder" }
    #expect(folder?.coverage.excludedCount == 1 && folder?.coverage.hasUnplannedOmissions == false)
    try coverageWrite(root, "Denied/inside.bin")
    let denied = root.appendingPathComponent("Denied")
    #expect(chmod(denied.path, 0) == 0)
    defer { _ = chmod(denied.path, 0o700) }
    let mixed = await scanner.scan(root: root)
    #expect(mixed.coverage.excludedCount == 1 && mixed.coverage.hasUnplannedOmissions)
    #expect(mixed.entries.first { $0.url.lastPathComponent == "Denied" }?.coverage.hasUnplannedOmissions == true)
}

@Test func unknownCoverageAndMixedAggregationStayConservative() async throws {
    let legacy = ScanReport(items: [], warnings: ["opaque"], inspectedCount: 0, isPartial: true, cancelled: false, duration: 0)
    #expect(legacy.coverage.hasUnplannedOmissions && legacy.coverage.excludedCount == 0)
    let combined = ScanCoverage.combined([ScanCoverage(excludedCount: 3), legacy.coverage, ScanCoverage(excludedCount: 2)])
    #expect(combined.excludedCount == 5 && combined.hasUnplannedOmissions)
    #expect(ScanCoverage.combined([]) == ScanCoverage())
}
