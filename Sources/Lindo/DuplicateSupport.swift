import Foundation
import LindoKit

extension AppModel {
    var matchingDuplicateGroups: [DuplicateGroup] { resultsProjection.groups }

    func isKeeper(_ id: String) -> Bool { duplicateKeeperIDs.contains(id) }

    func keeperID(for group: DuplicateGroup) -> String { duplicateKeepers[group.id] ?? group.keeperID }

    func keep(_ item: ScanItem, in group: DuplicateGroup) {
        guard !isBusy && !showConfirmation && group.files.contains(where: { $0.id == item.id }) else { return }
        duplicateKeepers[group.id] = item.id
        selected.remove(item.id)
    }

    func selectDuplicateCopies() {
        guard !isBusy && !showConfirmation else { return }
        let copies = matchingDuplicateGroups.flatMap { group in group.files.filter { $0.id != keeperID(for: group) }.map(\.id) }
        selected.formUnion(copies)
    }

    func clearResults(_ page: SidebarItem) {
        items[page] = nil
        warnings[page] = nil
        scanDates[page] = nil
        scannedScopes[page] = nil
        scanOutcomes[page] = nil
        coverageByPage[page] = nil
        notices[page] = nil
        failuresByPage[page] = nil
        resetInteraction(for: page)
        if page == .duplicates {
            duplicateGroups = []
            duplicateKeepers = [:]
        }
    }

    func removeTrashedDuplicates(_ moved: Set<String>) {
        duplicateGroups = duplicateGroups.compactMap { group in
            let remaining = group.files.filter { !moved.contains($0.id) }
            guard remaining.count > 1 else { return nil }
            return DuplicateGroup(id: group.id, files: remaining, keeperID: keeperID(for: group), digest: group.digest)
        }
        let groupIDs = Set(duplicateGroups.map(\.id))
        duplicateKeepers = duplicateKeepers.filter { groupIDs.contains($0.key) }
        items[.duplicates] = duplicateGroups.flatMap(\.files)
        setSelection(selection(for: .duplicates).intersection(items[.duplicates, default: []].map(\.id)), for: .duplicates)
    }

}
