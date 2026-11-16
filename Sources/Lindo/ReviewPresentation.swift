import Foundation
import LindoKit

/// Value snapshots keep the comparison independent of later navigation or result replacement.
struct ReviewDuplicateGroup: Identifiable {
    let id: String
    let removed: [ScanItem]
    let preserved: [ScanItem]
    let protectedID: String
}

enum ReviewDuplicateRow: Identifiable {
    case group(id: String, number: Int)
    case section(id: String, preserved: Bool, count: Int, bytes: Int64)
    case file(ScanItem, protected: Bool)

    var id: String {
        switch self {
        case .group(let id, _): "group-" + id
        case .section(let id, _, _, _): "section-" + id
        case .file(let item, _): "file-" + item.id
        }
    }
}

func freezeDuplicateReview(groups: [DuplicateGroup], selectedIDs: Set<String>,
                           keepers: [String: String], order: DuplicateSortOrder) -> [ReviewDuplicateGroup] {
    orderedDuplicateGroups(groups, by: order).compactMap { group in
        let removed = group.files.filter { selectedIDs.contains($0.id) }
        guard !removed.isEmpty else { return nil }
        return ReviewDuplicateGroup(id: group.id, removed: removed,
            preserved: group.files.filter { !selectedIDs.contains($0.id) },
            protectedID: keepers[group.id] ?? group.keeperID)
    }
}

/// Flatten once at review time so a group with thousands of copies remains lazily rendered.
func duplicateReviewRows(_ groups: [ReviewDuplicateGroup]) -> [ReviewDuplicateRow] {
    var rows: [ReviewDuplicateRow] = []
    for (index, group) in groups.enumerated() {
        rows.append(.group(id: group.id, number: index + 1))
        rows.append(.section(id: group.id + "-remove", preserved: false,
                             count: group.removed.count, bytes: group.removed.reduce(0) { $0 + $1.bytes }))
        rows += group.removed.map { .file($0, protected: false) }
        rows.append(.section(id: group.id + "-preserve", preserved: true,
                             count: group.preserved.count, bytes: group.preserved.reduce(0) { $0 + $1.bytes }))
        rows += group.preserved.map { .file($0, protected: $0.id == group.protectedID) }
    }
    return rows
}
