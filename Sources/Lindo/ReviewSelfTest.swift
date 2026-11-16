import Foundation
import LindoKit

/// Regression checks use synthetic values only; no scan or cleanup is started.
enum ReviewSelfTest {
    static func run() -> [(String, Bool)] {
        let model = AppModel()
        guard model.isSample else { return [("Review checks require sample mode", false)] }
        var checks: [(String, Bool)] = []
        let files = (0..<10_000).map { index in
            ScanItem(id: "review-\(index)", url: URL(fileURLWithPath: "/synthetic-review/file-\(index)"),
                     title: "Synthetic \(index)", bytes: 128, modifiedAt: Date(timeIntervalSince1970: 0), category: .largeFiles)
        }
        let group = DuplicateGroup(id: "review-group", files: files, keeperID: files[0].id, digest: "synthetic")
        model.sidebar = .duplicates
        model.items[.duplicates] = files
        model.duplicateGroups = [group]
        model.duplicateKeepers = [group.id: group.keeperID]
        model.selected = []
        model.query = ""
        let initialBuilds = model.projectionBuildCount
        let initialRows = model.resultsProjection.rows.map(\.id)
        checks.append(("Large duplicate group flattens into 10001 unique rows", initialRows.count == 10_001 && Set(initialRows).count == 10_001))
        for _ in 0..<5 {
            _ = model.visibleItems
            _ = model.matchingDuplicateGroups
            _ = model.listedBytes
            _ = model.hiddenSelectedCount
        }
        checks.append(("Repeated large result reads reuse one projection", model.projectionBuildCount == initialBuilds + 1 && model.resultsProjection.rows.map(\.id) == initialRows))
        model.toggle(files[1])
        checks.append(("Selecting a copy refreshes metrics without refiltering", model.selectedItems.map(\.id) == [files[1].id] && model.selectedBytes == 128 && model.hiddenSelectedCount == 0 && model.projectionBuildCount == initialBuilds + 1))
        model.keep(files[1], in: group)
        checks.append(("Changing keeper refreshes selection without refiltering", model.isKeeper(files[1].id) && !model.isKeeper(files[0].id) && model.selectedItems.isEmpty && model.selectedBytes == 0 && model.hiddenSelectedCount == 0 && model.projectionBuildCount == initialBuilds + 1))
        model.toggle(files[0])
        model.query = "no-synthetic-match"
        let beforeQuery = model.projectionBuildCount
        checks.append(("Unmatched query retains selected copy as hidden", model.visibleItems.isEmpty && model.hiddenSelectedCount == 1 && model.selectedItems.map(\.id) == [files[0].id] && model.selectedBytes == 128))
        _ = model.resultsProjection
        checks.append(("Query change builds exactly one cached projection", model.projectionBuildCount == beforeQuery + 1))
        model.keep(files[0], in: group)
        checks.append(("A hidden copy becoming keeper leaves no removable selection", model.selectedItems.isEmpty && model.selectedBytes == 0 && model.hiddenSelectedCount == 0 && !model.selected.contains(files[0].id) && model.projectionBuildCount == beforeQuery + 1))
        model.query = files[9_999].title
        checks.append(("Matching one sibling exposes the entire group with stable row IDs", model.visibleItems.count == 10_000 && model.resultsProjection.rows.map(\.id) == initialRows))

        model.query = ""
        _ = model.resultsProjection
        let beforeGroupReplacement = model.projectionBuildCount
        let replacement = DuplicateGroup(id: group.id, files: Array(files.prefix(2)), keeperID: files[0].id, digest: group.digest)
        model.duplicateGroups = [replacement]
        checks.append(("Replacing groups invalidates cached flattened rows", model.resultsProjection.rows.count == 3 && model.projectionBuildCount == beforeGroupReplacement + 1))
        _ = model.resultsProjection
        checks.append(("Replaced group projection is reused", model.projectionBuildCount == beforeGroupReplacement + 1))

        model.sidebar = .largeFiles
        model.items[.largeFiles] = files
        model.selected = [files[1].id]
        _ = model.resultsProjection
        _ = model.selectedBytes
        let beforeItemReplacement = model.projectionBuildCount
        model.items[.largeFiles] = [files[2]]
        checks.append(("Replacing items refreshes visible results and selected metrics", model.visibleItems.map(\.id) == [files[2].id] && model.listedBytes == 128 && model.selectedItems.isEmpty && model.selectedBytes == 0 && model.projectionBuildCount == beforeItemReplacement + 1))
        _ = model.visibleItems
        checks.append(("Replaced item projection is reused", model.projectionBuildCount == beforeItemReplacement + 1))

        for page in [SidebarItem.cleanup, .largeFiles, .duplicates, .applications] {
            model.items[page] = []
            model.warnings[page] = []
            model.scanDates[page] = Date(timeIntervalSince1970: 0)
            model.scanOutcomes[page] = .partial
            let partialSummary = model.scanSummary(for: page)
            let partialEmpty = model.emptyScanMessage(for: page)
            model.scanOutcomes[page] = .interrupted
            let interruptedSummary = model.scanSummary(for: page)
            let interruptedEmpty = model.emptyScanMessage(for: page)
            model.scanOutcomes[page] = .complete
            let completeSummary = model.scanSummary(for: page)
            let completeEmpty = model.emptyScanMessage(for: page)
            checks.append(("\(page): overview distinguishes partial, interrupted and complete", partialSummary.contains("Parziale") && interruptedSummary.contains("Interrotta") && Set([partialSummary, interruptedSummary, completeSummary]).count == 3))
            checks.append(("\(page): zero results distinguish partial, interrupted and complete", partialEmpty.title.contains("parziale") && interruptedEmpty.title.contains("interrotta") && completeEmpty.title.contains(page == .duplicates ? "duplicato" : "candidato") && Set([partialEmpty.title, interruptedEmpty.title, completeEmpty.title]).count == 3 && !partialEmpty.detail.isEmpty && !interruptedEmpty.detail.isEmpty))
            model.clearResults(page)
            checks.append(("\(page): clearing results clears outcome and summary", model.scanOutcomes[page] == nil && model.scanOutcome(for: page) == nil && model.scanDates[page] == nil && model.scanSummary(for: page).isEmpty))
        }

        // These paths do not need to exist: review consumes the scan's cached classification.
        let syncModel = AppModel()
        syncModel.sidebar = .largeFiles
        let localItems = (0..<500).map { index in
            ScanItem(id: "sync-local-\(index)", url: URL(fileURLWithPath: "/synthetic-review/local-\(index)"),
                     title: "Local \(index)", bytes: 2, category: .largeFiles, syncStatus: .local)
        }
        let synced = ScanItem(id: "sync-last", url: URL(fileURLWithPath: "/synthetic-review/Documents/shared.pdf"),
                              title: "Shared", bytes: 1, category: .largeFiles, syncStatus: .synced)
        syncModel.items[.largeFiles] = localItems + [synced]
        syncModel.selected = Set(syncModel.currentItems.map(\.id))
        syncModel.reviewSelection()
        checks.append(("Review warns about a synced file after 500 local items", syncModel.reviewItems.count == 501 && syncModel.reviewSyncStatus == .synced))
        syncModel.items[.largeFiles] = localItems
        checks.append(("Review freezes sync classification with the confirmed items", syncModel.reviewItems.count == 501 && syncModel.reviewSyncStatus == .synced))
        syncModel.showConfirmation = false
        let unknown = ScanItem(id: "sync-unknown", url: URL(fileURLWithPath: "/synthetic-review/unavailable.pdf"),
                               title: "Unknown", bytes: 1, category: .largeFiles)
        syncModel.items[.largeFiles] = localItems + [unknown]
        syncModel.selected = Set(syncModel.currentItems.map(\.id))
        syncModel.reviewSelection()
        checks.append(("Review distinguishes unknown sync from confirmed synchronization", syncModel.reviewSyncStatus == .unknown))
        syncModel.showConfirmation = false
        syncModel.items[.largeFiles] = localItems
        syncModel.selected = Set(localItems.map(\.id))
        syncModel.reviewSelection()
        checks.append(("Review clears a previous sync warning for local items", syncModel.reviewSyncStatus == .local))

        let sortModel = AppModel()
        sortModel.sidebar = .duplicates
        func sortGroup(_ name: String, copies: Int, size: Int64) -> DuplicateGroup {
            let members = (0..<copies).map { index in
                ScanItem(id: "sort-\(name)-\(index)", url: URL(fileURLWithPath: "/synthetic-sort/\(name)/\(index)"),
                         title: "\(name) \(index)", bytes: size, category: .largeFiles, syncStatus: .local)
            }
            return DuplicateGroup(id: "sort-\(name)", files: members, keeperID: members[0].id, digest: name)
        }
        let alpha = sortGroup("A", copies: 2, size: 100)
        let beta = sortGroup("B", copies: 3, size: 150)
        let gamma = sortGroup("C", copies: 2, size: 300)
        let zeta = sortGroup("Z", copies: 2, size: 200)
        let unordered = [gamma, alpha, zeta, beta]
        sortModel.duplicateGroups = unordered
        sortModel.items[.duplicates] = unordered.flatMap(\.files)
        sortModel.duplicateKeepers = Dictionary(uniqueKeysWithValues: unordered.map { ($0.id, $0.keeperID) })
        sortModel.keep(beta.files[2], in: beta)
        sortModel.selected = [alpha.files[1].id, beta.files[1].id]
        let selectedBeforeSort = sortModel.selected
        let keepersBeforeSort = sortModel.duplicateKeepers
        let selectedBytesBeforeSort = sortModel.selectedBytes
        checks.append(("Duplicate groups default to extra bytes with deterministic path ties", sortModel.duplicateSortOrder == .extraBytes && sortModel.matchingDuplicateGroups.map(\.id) == [beta.id, gamma.id, zeta.id, alpha.id]))
        let beforeSortChange = sortModel.projectionBuildCount
        sortModel.duplicateSortOrder = .path
        checks.append(("Changing duplicate order invalidates the cached projection once", sortModel.matchingDuplicateGroups.map(\.id) == [alpha.id, beta.id, gamma.id, zeta.id] && sortModel.projectionBuildCount == beforeSortChange + 1))
        _ = sortModel.visibleItems
        _ = sortModel.resultsProjection.rows
        checks.append(("Repeated reads reuse the sorted duplicate projection", sortModel.projectionBuildCount == beforeSortChange + 1))
        checks.append(("Sorting preserves selections, protected copies, bytes and source file order", sortModel.selected == selectedBeforeSort && sortModel.duplicateKeepers == keepersBeforeSort && sortModel.selectedBytes == selectedBytesBeforeSort && sortModel.duplicateGroups.map { $0.files.map(\.id) } == unordered.map { $0.files.map(\.id) }))
        sortModel.query = "no-matching-group"
        sortModel.duplicateSortOrder = .extraBytes
        checks.append(("Sorting a filtered view retains hidden selected copies", sortModel.matchingDuplicateGroups.isEmpty && sortModel.hiddenSelectedCount == 2 && sortModel.selected == selectedBeforeSort && sortModel.duplicateKeepers == keepersBeforeSort))
        let samePathA = DuplicateGroup(id: "tie-A", files: alpha.files, keeperID: alpha.keeperID, digest: "tie-A")
        let samePathB = DuplicateGroup(id: "tie-B", files: alpha.files, keeperID: alpha.keeperID, digest: "tie-B")
        checks.append(("Equal duplicate sort keys use group IDs for a stable final tie", orderedDuplicateGroups([samePathB, samePathA], by: .extraBytes).map(\.id) == [samePathA.id, samePathB.id] && orderedDuplicateGroups([samePathB, samePathA], by: .path).map(\.id) == [samePathA.id, samePathB.id]))
        return checks
    }
}
