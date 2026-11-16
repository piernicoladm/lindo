import Foundation
import LindoKit

/// Product-flow regression checks use only in-memory fixtures, never real scans or removals.
enum ProductSelfTest {
    static func run() -> [(String, Bool)] {
        let model = AppModel()
        guard model.isSample else { return [("Product checks require sample mode", false)] }
        var checks: [(String, Bool)] = []
        let pages: [SidebarItem] = [.cleanup, .largeFiles, .duplicates, .applications]
        var expected: [SidebarItem: Set<String>] = [:]
        for page in pages {
            model.sidebar = page
            let candidate = model.currentItems.first { page != .duplicates || !model.isKeeper($0.id) }!
            model.toggle(candidate)
            model.query = "filter-\(page.rawValue)"
            expected[page] = [candidate.id]
        }
        for page in pages.reversed() {
            model.sidebar = .overview
            model.sidebar = page
            model.pageChanged()
            checks.append(("\(page): navigation restores selection and its own hidden filter",
                model.selected == expected[page] && model.query == "filter-\(page.rawValue)"
                && model.selectedItems.count == 1 && model.hiddenSelectedCount == 1))
        }
        model.sidebar = .largeFiles
        let largeSelection = model.selected
        model.clearResults(.duplicates)
        model.sidebar = .duplicates
        let duplicatesReset = model.selected.isEmpty && model.query.isEmpty && model.duplicateKeepers.isEmpty
        model.sidebar = .largeFiles
        checks.append(("Invalidating a background page leaves the current selection and query intact",
            duplicatesReset && model.selected == largeSelection && model.query == "filter-largeFiles"))
        model.minimumMB = 500
        checks.append(("Threshold changes invalidate that page's results, selection and query",
            model.currentItems.isEmpty && model.scanDates[.largeFiles] == nil
            && model.selected.isEmpty && model.query.isEmpty && model.selection(for: .applications) == expected[.applications]))
        model.sidebar = .applications
        model.resetInteraction(for: .cleanup)
        checks.append(("Preparing another page for rescan keeps the visible page's interaction",
            model.selected == expected[.applications] && model.query == "filter-applications"
            && model.selection(for: .cleanup).isEmpty))

        let scopeModel = AppModel()
        scopeModel.sidebar = .largeFiles
        scopeModel.selected = [scopeModel.currentItems[0].id]
        scopeModel.query = "Workshop"
        let originalRoot = scopeModel.largeRoot
        scopeModel.largeRoot = originalRoot
        checks.append(("Choosing the existing scope preserves results and interaction",
            scopeModel.selectedItems.count == 1 && scopeModel.query == "Workshop" && scopeModel.currentItems.count == 3))
        scopeModel.scannedScopes[.largeFiles] = "Recorded scope"
        checks.append(("Overview reports the scope stored with a completed scan",
            scopeModel.scopeDescription(for: .largeFiles) == "Recorded scope"))
        scopeModel.largeRoot = URL(fileURLWithPath: "/synthetic-product/new-root")
        checks.append(("A new root clears the old scope, results, selection and query",
            scopeModel.scanDates[.largeFiles] == nil && scopeModel.currentItems.isEmpty
            && scopeModel.selected.isEmpty && scopeModel.query.isEmpty
            && scopeModel.scannedScopes[.largeFiles] == nil
            && scopeModel.scopeDescription(for: .largeFiles).contains("/synthetic-product/new-root")))
        checks.append(("Overview uses actual map and application roots",
            scopeModel.scopeDescription(for: .spaceMap) == scopeModel.spaceReport?.root.path
            && scopeModel.scopeDescription(for: .applications).contains("/Applications")
            && scopeModel.scopeDescription(for: .applications).contains("/Users/demo/Applications")))

        let removalModel = AppModel()
        removalModel.sidebar = .duplicates
        let originalGroup = removalModel.duplicateGroups[0]
        let extras = originalGroup.files.filter { !removalModel.isKeeper($0.id) }
        removalModel.selected = Set(extras.map(\.id))
        removalModel.sidebar = .largeFiles
        removalModel.selected = [removalModel.currentItems[0].id]
        let otherPageSelection = removalModel.selected
        removalModel.removeTrashedDuplicates([extras[0].id])
        checks.append(("Background duplicate removal updates only duplicate selection",
            removalModel.selected == otherPageSelection
            && removalModel.selection(for: .duplicates) == [extras[1].id]
            && !removalModel.items[.duplicates, default: []].contains(where: { $0.id == extras[0].id })))

        let reviewModel = AppModel()
        reviewModel.sidebar = .duplicates
        let groups = reviewModel.duplicateGroups
        reviewModel.keep(groups[0].files[2], in: groups[0])
        let ids: Set<String> = [groups[0].files[1].id, groups[1].files[1].id]
        reviewModel.selected = ids
        reviewModel.query = "Foto_evento"
        reviewModel.reviewSelection()
        let frozen = reviewModel.reviewDuplicateGroups
        checks.append(("Grouped review includes selected copies hidden by the filter",
            frozen.count == 2 && Set(frozen.flatMap(\.removed).map(\.id)) == ids
            && Set(reviewModel.reviewItems.map(\.id)) == ids))
        checks.append(("Each reviewed group lists all unselected copies and the chosen keeper",
            frozen[0].protectedID == groups[0].files[2].id
            && Set(frozen[0].preserved.map(\.id)) == [groups[0].files[0].id, groups[0].files[2].id]
            && frozen.allSatisfy { group in group.preserved.contains { $0.id == group.protectedID } }))
        let protectedRows = reviewModel.reviewDuplicateRows.compactMap { row -> String? in
            if case .file(let item, protected: true) = row { return item.id }
            return nil
        }
        checks.append(("Grouped review distinguishes the protected copy from other preserved copies",
            Set(protectedRows) == Set(frozen.map(\.protectedID)) && protectedRows.count == 2))
        let rowIDs = reviewModel.reviewDuplicateRows.map(\.id)
        reviewModel.selected = []
        reviewModel.duplicateGroups = []
        reviewModel.duplicateKeepers = [:]
        reviewModel.items[.duplicates] = []
        reviewModel.sidebar = .applications
        reviewModel.selected = [reviewModel.currentItems[0].id]
        reviewModel.reviewSelection()
        checks.append(("An open grouped review stays frozen across result replacement and navigation",
            reviewModel.reviewPage == .duplicates && Set(reviewModel.reviewItems.map(\.id)) == ids
            && reviewModel.reviewDuplicateRows.map(\.id) == rowIDs
            && reviewModel.reviewDuplicateGroups.map(\.protectedID) == frozen.map(\.protectedID)))
        reviewModel.showConfirmation = false
        reviewModel.reviewSelection()
        checks.append(("A later nonduplicate review clears previous group presentation",
            reviewModel.reviewPage == .applications && reviewModel.reviewDuplicateGroups.isEmpty
            && reviewModel.reviewDuplicateRows.isEmpty && reviewModel.preservedReviewFiles.isEmpty))

        let manyFiles = (0..<10_000).map { index in
            ScanItem(id: "group-review-\(index)", url: URL(fileURLWithPath: "/synthetic-product/\(index)"),
                     title: "Copy \(index)", bytes: 1, category: .largeFiles, syncStatus: .local)
        }
        let manyGroup = DuplicateGroup(id: "many", files: manyFiles, keeperID: manyFiles[0].id, digest: "synthetic")
        let manySnapshot = freezeDuplicateReview(groups: [manyGroup], selectedIDs: Set(manyFiles.dropFirst().map(\.id)),
                                                 keepers: [:], order: .extraBytes)
        let manyRows = duplicateReviewRows(manySnapshot)
        checks.append(("Ten thousand copies produce complete unique rows for lazy grouped review",
            manySnapshot[0].removed.count == 9_999 && manySnapshot[0].preserved.map(\.id) == [manyFiles[0].id]
            && manyRows.count == 10_003 && Set(manyRows.map(\.id)).count == manyRows.count))
        return checks
    }
}
