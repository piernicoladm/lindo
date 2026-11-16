import Foundation

enum UXSelfTest {
    static func run() -> [(String, Bool)] {
        let model = AppModel()
        guard model.isSample else { return [("UX checks require sample mode", false)] }
        var checks: [(String, Bool)] = []
        model.sidebar = .largeFiles
        if let first = model.currentItems.first {
            model.toggle(first)
            model.query = "no-matching-file"
            checks.append(("Filtered selections remain explicit", model.visibleItems.isEmpty && model.hiddenSelectedCount == 1 && model.selectedItems.count == 1))
            model.reviewSelection()
            checks.append(("Review includes selected files hidden by search", model.reviewItems.map(\.id) == [first.id]))
            model.showConfirmation = false
            model.clearSearch()
            checks.append(("Clearing search preserves selection", model.hiddenSelectedCount == 0 && model.selected.contains(first.id)))
        }
        model.sidebar = .duplicates
        model.pageChanged()
        if let group = model.duplicateGroups.first, let extra = group.files.first(where: { !model.isKeeper($0.id) }) {
            model.toggle(extra)
            model.query = group.files.first!.title
            checks.append(("Group search does not hide sibling selection", model.matchingDuplicateGroups.count == 1 && model.hiddenSelectedCount == 0))
            model.query = "Foto_evento"
            checks.append(("Group search reports hidden selected copy", model.hiddenSelectedCount == 1))
            let selectedBeforeClearing = model.selected
            let keepersBeforeClearing = model.duplicateKeepers
            model.clearSearch()
            checks.append(("Removing the duplicate filter preserves selections and protected copies", model.hiddenSelectedCount == 0 && model.selected == selectedBeforeClearing && model.duplicateKeepers == keepersBeforeClearing))
        }
        model.activePage = .duplicates
        model.scanning = true
        model.sidebar = .overview
        checks.append(("Background scan keeps its own context", model.activityPage == .duplicates && !model.isScanningCurrentPage && model.isBusy))
        model.cancelScan()
        checks.append(("Scan cancellation has visible pending state", model.scanCancelling && model.progressText.contains("Interruzione")))
        model.scanning = false
        model.activePage = nil
        model.cleaning = true
        model.reviewPage = .largeFiles
        checks.append(("Removal context survives navigation", model.activityPage == .largeFiles))
        checks.append(("Navigation shortcuts follow sidebar order", SidebarItem.allCases == [.overview, .spaceMap, .cleanup, .largeFiles, .duplicates, .applications]))
        checks += ReviewSelfTest.run()
        checks += ProductSelfTest.run()
        checks += CoverageSelfTest.run()
        return checks
    }
}
