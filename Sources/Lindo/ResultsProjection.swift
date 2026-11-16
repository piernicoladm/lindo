import Foundation
import LindoKit

enum DuplicateListRow: Identifiable {
    case header(DuplicateGroup)
    case file(ScanItem, DuplicateGroup)
    var id: String {
        switch self {
        case .header(let group): "header-" + group.id
        case .file(let item, _): "file-" + item.id
        }
    }
}

struct ResultsProjection {
    let page: SidebarItem
    let query: String
    let revision: Int
    let duplicateSortOrder: DuplicateSortOrder
    let visibleItems: [ScanItem]
    let groups: [DuplicateGroup]
    let rows: [DuplicateListRow]
    let visibleIDs: Set<String>
    let listedBytes: Int64
}

struct SelectionProjection {
    let page: SidebarItem
    let resultsRevision: Int
    let selectionRevision: Int
    let items: [ScanItem]
    let bytes: Int64
}

/// Sort groups for presentation only; file order, protected copies and selection stay untouched.
func orderedDuplicateGroups(_ groups: [DuplicateGroup], by order: DuplicateSortOrder) -> [DuplicateGroup] {
    // Compute path keys once per projection instead of walking the files inside each comparison.
    let keyed: [(group: DuplicateGroup, extraBytes: Int64, path: String)] = groups.map { group in
        let extraBytes = Int64(max(0, group.files.count - 1)) * (group.files.first?.bytes ?? 0)
        let path = group.files.map { $0.url.path }.min() ?? ""
        return (group, extraBytes, path)
    }
    return keyed.sorted { left, right in
        if order == .extraBytes && left.extraBytes != right.extraBytes {
            return left.extraBytes > right.extraBytes
        }
        if left.path != right.path { return left.path < right.path }
        return left.group.id < right.group.id
    }.map { $0.group }
}

extension AppModel {
    // Revision reads remain observable even on a cache hit. Selection changes
    // never invalidate the text filter or the flat, lazily rendered row list.
    var resultsProjection: ResultsProjection {
        if let cached = resultsCache, cached.page == sidebar,
           cached.query == query, cached.revision == resultsRevision,
           cached.duplicateSortOrder == duplicateSortOrder { return cached }
        let current = currentItems
        let matches: (ScanItem) -> Bool = { [query] in
            query.isEmpty || $0.title.localizedCaseInsensitiveContains(query)
                || $0.url.path.localizedCaseInsensitiveContains(query)
        }
        let matchingGroups = sidebar == .duplicates ? duplicateGroups.filter { query.isEmpty || $0.files.contains(where: matches) } : []
        let groups = orderedDuplicateGroups(matchingGroups, by: duplicateSortOrder)
        let visible = sidebar == .duplicates ? groups.flatMap(\.files) : current.filter(matches)
        let rows = groups.flatMap { group in
            [DuplicateListRow.header(group)] + group.files.map { .file($0, group) }
        }
        let projection = ResultsProjection(page: sidebar, query: query, revision: resultsRevision,
            duplicateSortOrder: duplicateSortOrder,
            visibleItems: visible, groups: groups, rows: rows, visibleIDs: Set(visible.map(\.id)),
            listedBytes: current.reduce(0) { $0 + $1.bytes })
        projectionBuildCount += 1
        resultsCache = projection
        return projection
    }

    var selectionProjection: SelectionProjection {
        if let cached = selectionCache, cached.page == sidebar,
           cached.resultsRevision == resultsRevision,
           cached.selectionRevision == selectionRevision { return cached }
        let selectedItems = currentItems.filter { selected.contains($0.id) && (sidebar != .duplicates || !isKeeper($0.id)) }
        let projection = SelectionProjection(page: sidebar, resultsRevision: resultsRevision,
            selectionRevision: selectionRevision, items: selectedItems, bytes: selectedItems.reduce(0) { $0 + $1.bytes })
        selectionCache = projection
        return projection
    }
}
