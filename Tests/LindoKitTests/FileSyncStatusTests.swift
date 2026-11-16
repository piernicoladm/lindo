import Foundation
import Testing
@testable import LindoKit

@Test func syncPolicyRecognizesProviderRootsWithoutReadingMetadata() {
    for path in [
        "/Users/demo/Library/CloudStorage/Provider/Shared/video.mov",
        "/Users/demo/library/mobile documents/com~apple~CloudDocs/a.pdf",
        "/Users/demo/Dropbox/Shared/video.mov",
        "/Users/demo/OneDrive",
        "/Users/demo/Google Drive/project/file.txt"
    ] {
        let status = FileSyncPolicy.read(from: URL(fileURLWithPath: path), home: "/Users/demo") { _ in
            Issue.record("Known provider paths must not trigger a metadata read")
            return nil
        }
        #expect(status == .synced)
    }
}

@Test func syncPolicyDoesNotConfuseSimilarFolderNames() {
    for path in [
        "/Users/demo/Downloads/video.mov",
        "/Users/demo/Dropbox-old/video.mov",
        "/Users/demo/Library/CloudStorageBackup/a",
        "/Users/demolition/Dropbox/a"
    ] {
        #expect(FileSyncPolicy.classify(path: path, home: "/Users/demo", isUbiquitous: false) == .local)
        #expect(FileSyncPolicy.classify(path: path, home: "/Users/demo", isUbiquitous: nil) == .unknown)
    }
}

@Test func syncPolicyUsesICloudMetadataOutsideProviderPaths() {
    let item = URL(fileURLWithPath: "/Users/demo/Documents/shared.pdf")
    #expect(FileSyncPolicy.read(from: item, home: "/Users/demo", metadata: { _ in true }) == .synced)
    #expect(FileSyncPolicy.read(from: item, home: "/Users/demo", metadata: { _ in false }) == .local)
}

@Test func syncPolicyPreservesMissingAndFailedMetadataAsUnknown() {
    let item = URL(fileURLWithPath: "/Users/demo/Documents/unavailable.pdf")
    #expect(FileSyncPolicy.read(from: item, home: "/Users/demo", metadata: { _ in nil }) == .unknown)
    #expect(FileSyncPolicy.read(from: item, home: "/Users/demo", metadata: { _ in
        throw CocoaError(.fileReadNoPermission)
    }) == .unknown)
    #expect(ScanItem(url: item, title: "Unclassified", bytes: 1, category: .largeFiles).syncStatus == .unknown)
}

@Test func syncSummaryIncludesItemsBeyondTheOldReviewLimit() {
    let local = Array(repeating: FileSyncStatus.local, count: 500)
    #expect(FileSyncStatus.combined(local + [.synced]) == .synced)
    #expect(FileSyncStatus.combined(local + [.unknown]) == .unknown)
    #expect(FileSyncStatus.combined([.unknown] + local + [.synced]) == .synced)
    #expect(FileSyncStatus.combined(local) == .local)
    #expect(FileSyncStatus.combined([FileSyncStatus]()) == .local)
}
