import Foundation

extension AppModel {
    func scopeDescription(for page: SidebarItem) -> String {
        if scanDates[page] != nil, let scanned = scannedScopes[page] { return scanned }
        if page == .spaceMap, let report = spaceReport { return report.root.path }
        return configuredScopeDescription(for: page)
    }

    /// Capture this description with the report, so displayed totals retain their original scope.
    func configuredScopeDescription(for page: SidebarItem) -> String {
        let home = isSample ? "/Users/demo" : FileManager.default.homeDirectoryForCurrentUser.path
        switch page {
        case .overview: return "Disco di avvio"
        case .spaceMap: return spaceRoot.path
        case .cleanup: return "\(home)/Library/Caches · \(home)/Library/Logs"
        case .applications: return "/Applications · \(home)/Applications"
        case .largeFiles:
            return "\(largeRoot?.path ?? "Cartella da scegliere") · \(minimumMB) MB+"
        case .duplicates:
            let threshold = duplicateMinimumKB == 0 ? "File non vuoti" : "\(bytes(Int64(duplicateMinimumKB) * 1_000))+"
            return "\(duplicateRoot?.path ?? "Cartella da scegliere") · \(threshold)"
        }
    }
}
