import Foundation
import Testing
@testable import LindoKit

@Test(arguments: [0, 1, 2])
func queuedScanChildrenRejectReplacedAncestors(depth: Int) async throws {
    let fm = FileManager.default
    // Foundation may standardize /private/tmp back to the symlink /tmp.
    // Use the same no-symlink synthetic fixture root as the other scanner tests.
    let fixture = fm.homeDirectoryForCurrentUser
        .appendingPathComponent("Library/Developer/Lindo/test-fixtures")
        .appendingPathComponent("Lindo-AncestorReplacement-" + UUID().uuidString)
    let root = fixture.appendingPathComponent("Chosen")
    let protectedTarget = fixture.appendingPathComponent("_6_Archivio_Private")
    let original = fixture.appendingPathComponent("Original")
    let relative = depth == 0 ? "" : depth == 1 ? "Folder" : "Folder/Inner"
    let enumerated = relative.isEmpty ? root : root.appendingPathComponent(relative)
    // The deepest case replaces a grandparent, not merely the direct parent.
    let replaced = depth == 0 ? root : root.appendingPathComponent("Folder")
    let targetDirectory = depth == 2 ? protectedTarget.appendingPathComponent("Inner") : protectedTarget
    try fm.createDirectory(at: enumerated, withIntermediateDirectories: true)
    defer { try? fm.removeItem(at: fixture) }
    try fm.createDirectory(at: targetDirectory, withIntermediateDirectories: true)
    try Data([65]).write(to: enumerated.appendingPathComponent("same-name.bin"))
    try Data([66, 66]).write(to: targetDirectory.appendingPathComponent("same-name.bin"))

    let scanner = MacScanner(afterDirectoryEnumeration: { directory in
        guard directory.path == enumerated.path else { return }
        do {
            try FileManager.default.moveItem(at: replaced, to: original)
            try FileManager.default.createSymbolicLink(at: replaced, withDestinationURL: protectedTarget)
        } catch {
            Issue.record("Could not replace the fixture ancestor: \(error)")
        }
    }, trash: { _ in Issue.record("Unexpected trash during a read-only regression") })

    let report = await scanner.scan(category: .largeFiles, roots: [root], minimumBytes: 1)
    let destination = try fm.destinationOfSymbolicLink(atPath: replaced.path)
    #expect(destination == protectedTarget.path, "The test must actually replace the enumerated ancestor")
    #expect(report.items.isEmpty, "A queued child was accepted through a replaced ancestor")
    #expect(report.isPartial && !report.cancelled)
    #expect(report.warnings.contains { $0.contains("Collegamento, contenuto non locale o percorso cambiato") })
}
